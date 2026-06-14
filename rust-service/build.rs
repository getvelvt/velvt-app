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
}
