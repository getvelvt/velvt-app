use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

/// The file the app build reads its version from. Debug.xcconfig and
/// Release.xcconfig both include it, and the Makefile reads it, so a bare
/// `cargo build` reports the same version the app next to it would.
const VERSION_XCCONFIG: &str = "../swift-client/Configs/Version.xcconfig";

fn main() {
    println!("cargo:rerun-if-changed=migrations");
    let mut migrations = fs::read_dir("migrations")
        .expect("migrations directory must exist")
        .map(|entry| entry.expect("migration entry must be readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect::<Vec<_>>();
    migrations.sort();

    let mut generated = String::from("const EMBEDDED_MIGRATIONS: &[Migration] = &[\n");
    // Two branches each adding "the next migration" produce two files with the
    // same numeric prefix. run_migrations keys idempotence on that integer, so
    // the second file is skipped with no execute, no insert, and no error --
    // the UNIQUE constraint on schema_migration.version is never reached. The
    // build is the only place that collision is cheap to catch.
    let mut versions: HashMap<i64, String> = HashMap::new();
    for path in migrations {
        let name = path
            .file_name()
            .expect("migration must have a file name")
            .to_string_lossy();
        let version = name
            .split('_')
            .next()
            .expect("migration must start with a version")
            .parse::<i64>()
            .expect("migration version must be an integer");
        if let Some(previous) = versions.insert(version, name.to_string()) {
            panic!("duplicate migration version {version}: {previous} and {name}");
        }
        let absolute = fs::canonicalize(&path).expect("migration path must resolve");
        generated.push_str(&format!(
            "Migration {{ version: {version}, name: {name:?}, sql: include_str!({path:?}) }},\n",
            path = absolute.to_string_lossy()
        ));
    }
    generated.push_str("];\n");

    let output = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set"))
        .join("embedded_migrations.rs");
    fs::write(output, generated).expect("generated migrations must be writable");

    // Compile-time API config. Set these in the environment when building via
    // the Xcode Run Script phase; bare `cargo build` uses staging defaults so
    // local development needs no env setup.
    let api_base_url =
        env::var("VELVT_API_BASE_URL").unwrap_or_else(|_| "https://api.getvelvt.com".into());
    let apns_env = env::var("VELVT_APNS_ENV").unwrap_or_else(|_| "development".into());
    println!("cargo:rustc-env=VELVT_API_BASE_URL_COMPILED={api_base_url}");
    println!("cargo:rustc-env=VELVT_APNS_ENV_COMPILED={apns_env}");
    println!("cargo:rerun-if-env-changed=VELVT_API_BASE_URL");
    println!("cargo:rerun-if-env-changed=VELVT_APNS_ENV");

    // The version the helper reports: `--version`, every upload batch, and
    // device registration. Not CARGO_PKG_VERSION: Cargo.toml said 1.0.0 through
    // eleven releases, so the backend could not tell 1.0.3 from 1.0.11. The app
    // build passes the CFBundleShortVersionString it stamps (build_rust_helper.sh
    // via the Xcode phase); anything else reads Version.xcconfig; a crate
    // copied out of this repository falls back to Cargo's version.
    println!("cargo:rerun-if-env-changed=VELVT_SERVICE_VERSION");
    let version_file = Path::new(VERSION_XCCONFIG);
    if version_file.exists() {
        println!("cargo:rerun-if-changed={VERSION_XCCONFIG}");
    }
    let service_version = non_empty_env("VELVT_SERVICE_VERSION")
        .or_else(|| marketing_version(version_file))
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION must be set"));
    assert!(
        is_release_version(&service_version),
        "the service version must be dotted digits such as 1.0.11, got {service_version:?}"
    );
    println!("cargo:rustc-env=VELVT_SERVICE_VERSION_COMPILED={service_version}");

    // The commit the binary was built from. The release targets pass it (see
    // scripts/release_provenance.sh); a bare `cargo build` says "unknown"
    // rather than guess.
    println!("cargo:rerun-if-env-changed=VELVT_SOURCE_COMMIT");
    let source_commit = non_empty_env("VELVT_SOURCE_COMMIT").unwrap_or_else(|| "unknown".into());
    assert!(
        is_source_commit(&source_commit),
        "VELVT_SOURCE_COMMIT must be a hex commit id, optionally ending in -dirty, got {source_commit:?}"
    );
    println!("cargo:rustc-env=VELVT_SOURCE_COMMIT_COMPILED={source_commit}");
}

fn non_empty_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn marketing_version(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    contents.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "MARKETING_VERSION").then(|| value.trim().to_owned())
    })
}

fn is_release_version(version: &str) -> bool {
    let parts = version.split('.').collect::<Vec<_>>();
    (1..=4).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn is_source_commit(commit: &str) -> bool {
    if commit == "unknown" {
        return true;
    }
    let hash = commit.strip_suffix("-dirty").unwrap_or(commit);
    (7..=40).contains(&hash.len())
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
