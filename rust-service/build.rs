use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=migrations");
    let mut migrations = fs::read_dir("migrations")
        .expect("migrations directory must exist")
        .map(|entry| entry.expect("migration entry must be readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect::<Vec<_>>();
    migrations.sort();

    let mut generated = String::from("const EMBEDDED_MIGRATIONS: &[Migration] = &[\n");
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
}
