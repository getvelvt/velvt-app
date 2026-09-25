//! What this binary is, stamped at compile time by `build.rs`.

/// The release version, such as `1.0.11`: the app's `CFBundleShortVersionString`
/// when the app build compiled this helper, and otherwise the version in
/// `swift-client/Configs/Version.xcconfig`. It is what `--version` prints, what
/// every upload batch carries as `client_version`, and what device registration
/// reports.
pub const SERVICE_VERSION: &str = env!("VELVT_SERVICE_VERSION_COMPILED");

/// The commit this binary was built from, suffixed `-dirty` when the tree had
/// uncommitted changes, or `unknown` for a build that was not told. The app's
/// Info.plist carries the same value as `VelvtSourceCommit`.
pub const SOURCE_COMMIT: &str = env!("VELVT_SOURCE_COMMIT_COMPILED");

#[cfg(test)]
mod tests {
    use super::{SERVICE_VERSION, SOURCE_COMMIT};

    fn version_xcconfig_marketing_version() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../swift-client/Configs/Version.xcconfig"
        );
        let contents = std::fs::read_to_string(path).expect("Version.xcconfig is readable");
        contents
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "MARKETING_VERSION").then(|| value.trim().to_owned())
            })
            .expect("Version.xcconfig declares MARKETING_VERSION")
    }

    #[test]
    fn service_version_is_the_app_version_not_cargos() {
        match option_env!("VELVT_SERVICE_VERSION").filter(|value| !value.trim().is_empty()) {
            // The app build passed the version it stamps into Info.plist.
            Some(passed) => assert_eq!(SERVICE_VERSION, passed.trim()),
            None => assert_eq!(
                SERVICE_VERSION,
                version_xcconfig_marketing_version(),
                "a build that was not passed a version must report Version.xcconfig's"
            ),
        }
    }

    #[test]
    fn service_version_is_dotted_digits() {
        assert!(SERVICE_VERSION
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())));
    }

    #[test]
    fn source_commit_is_a_commit_id_or_says_it_is_unknown() {
        let hash = SOURCE_COMMIT
            .strip_suffix("-dirty")
            .unwrap_or(SOURCE_COMMIT);
        assert!(
            SOURCE_COMMIT == "unknown"
                || ((7..=40).contains(&hash.len())
                    && hash.bytes().all(|byte| byte.is_ascii_hexdigit())),
            "unexpected source commit {SOURCE_COMMIT:?}"
        );
    }
}
