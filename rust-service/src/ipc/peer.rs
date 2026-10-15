//! The code-signature check the helper makes on the process at the other end
//! of the socket before it serves the connection (design:
//! `docs/architecture/personal-vault-and-devices.md` §9.3, PR A0).
//!
//! The socket's directory is owner-only (`0700`). That keeps other users out,
//! but not another process running as the same person, which could connect and
//! receive what the helper pushes, sessions included. In a Release build the
//! helper now asks the kernel which process connected (its audit token, from
//! `LOCAL_PEERTOKEN`) and serves it only when that process's code signature
//! satisfies a requirement built from the helper's own: the same Apple-issued
//! team, and the app's identifier. The app makes the mirror check on the helper
//! (`swift-client/Sources/VelvtMac/IPC/SocketPeerCheck.swift`).
//!
//! Security.framework and CoreFoundation are called through the small FFI in
//! [`security`], so the check adds no crate.

use std::os::fd::AsRawFd;

/// The signing identifier of the Velvt app, which the helper requires of its
/// peer.
pub const APP_IDENTIFIER: &str = "com.velvt.mac";

/// The signing identifier of the helper (`scripts/sign_release.sh`), which the
/// app requires of its peer.
pub const SERVICE_IDENTIFIER: &str = "com.velvt.service";

/// Whether, and against what, the helper checks the process that connects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerPolicy {
    /// A helper built for a Debug app, or by a bare `cargo build`: every
    /// connection is served, as before the check existed.
    Skip,
    /// A helper built for a Release app: only a peer whose signature satisfies
    /// this requirement (code-signing requirement language) is served.
    Require(String),
    /// A helper built for a Release app that could not read its own signature,
    /// and so cannot say whom to trust. It serves nobody.
    Unavailable(PeerCheckError),
}

/// Why a peer was refused, or why the helper could not build its requirement.
///
/// Each variant carries only a numeric status, so a refusal is logged as an
/// error code and a number, never anything about the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerCheckError {
    /// The kernel did not report the peer's audit token (`errno`).
    #[error("the IPC peer's audit token is unavailable")]
    PeerTokenUnavailable { errno: i32 },
    /// Security.framework could not find the peer's code (`OSStatus`).
    #[error("the IPC peer's code is unavailable")]
    PeerCodeUnavailable { status: i32 },
    /// The peer's signature does not satisfy the requirement (`OSStatus`).
    #[error("the IPC peer's code signature was refused")]
    SignatureRejected { status: i32 },
    /// The helper could not read its own signature (`OSStatus`).
    #[error("this helper's own code signature is unavailable")]
    OwnSignatureUnavailable { status: i32 },
    /// The requirement could not be built or compiled (`OSStatus`, or 0 when
    /// the team or identifier was not something the language can quote).
    #[error("the IPC peer requirement is invalid")]
    RequirementInvalid { status: i32 },
    /// This platform has no code signatures to check.
    #[error("the IPC peer check is unsupported on this platform")]
    Unsupported,
}

impl PeerCheckError {
    /// The `error_code` this refusal is logged under.
    pub fn code(&self) -> &'static str {
        match self {
            Self::PeerTokenUnavailable { .. } => "ipc_peer_token_unavailable",
            Self::PeerCodeUnavailable { .. } => "ipc_peer_code_unavailable",
            Self::SignatureRejected { .. } => "ipc_peer_signature_rejected",
            Self::OwnSignatureUnavailable { .. } => "ipc_own_signature_unavailable",
            Self::RequirementInvalid { .. } => "ipc_peer_requirement_invalid",
            Self::Unsupported => "ipc_peer_check_unsupported",
        }
    }

    /// The `errno` or `OSStatus` behind the refusal, 0 when there is none.
    pub fn status(&self) -> i32 {
        match self {
            Self::PeerTokenUnavailable { errno } => *errno,
            Self::PeerCodeUnavailable { status }
            | Self::SignatureRejected { status }
            | Self::OwnSignatureUnavailable { status }
            | Self::RequirementInvalid { status } => *status,
            Self::Unsupported => 0,
        }
    }
}

/// The identity this process is signed with, as its own signature states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningIdentity {
    /// The signing identifier (`codesign -i`), absent for unsigned code.
    pub identifier: Option<String>,
    /// The Apple Developer team, absent for ad-hoc or unsigned code.
    pub team_identifier: Option<String>,
}

impl PeerPolicy {
    /// The policy of this build: [`PeerPolicy::Skip`] unless the helper was
    /// built for a Release app (`build_info::PEER_SIGNATURE_CHECK`), and then a
    /// requirement for the app built from this helper's own signature.
    pub fn for_this_build() -> Self {
        if crate::build_info::PEER_SIGNATURE_CHECK {
            Self::same_signer_as_this_process(APP_IDENTIFIER)
        } else {
            Self::Skip
        }
    }

    /// Requires a peer signed by this process's team with `peer_identifier`.
    pub fn same_signer_as_this_process(peer_identifier: &str) -> Self {
        let identity = match signing_identity_of_this_process() {
            Ok(identity) => identity,
            Err(error) => return Self::Unavailable(error),
        };
        match requirement_for(peer_identifier, identity.team_identifier.as_deref()) {
            Ok(requirement) => Self::Require(requirement),
            Err(error) => Self::Unavailable(error),
        }
    }

    /// What `velvt-service --peer-requirement` prints: `none` when nothing is
    /// checked, otherwise the requirement. `scripts/verify_release.sh` refuses
    /// a Release app whose helper answers `none`, and checks the app against
    /// the requirement with `codesign -R`.
    pub fn describe(&self) -> Result<&str, PeerCheckError> {
        match self {
            Self::Skip => Ok("none"),
            Self::Require(requirement) => Ok(requirement),
            Self::Unavailable(error) => Err(*error),
        }
    }

    /// Checks the process connected on `socket`. `Ok` means serve it.
    pub fn verify(&self, socket: &impl AsRawFd) -> Result<(), PeerCheckError> {
        match self {
            Self::Skip => Ok(()),
            Self::Unavailable(error) => Err(*error),
            Self::Require(requirement) => verify_peer(socket.as_raw_fd(), requirement),
        }
    }
}

/// The requirement for a peer signed with `peer_identifier` by the same team.
///
/// With a team (every Developer ID build, which `verify_release.sh --mode
/// production` insists on), the peer must chain to Apple, carry that team in
/// its leaf certificate, and have the identifier. Without one (an ad-hoc
/// `make package-release` build, which never leaves the machine that built it)
/// there is no team to pin, so the identifier alone is required.
pub fn requirement_for(
    peer_identifier: &str,
    team_identifier: Option<&str>,
) -> Result<String, PeerCheckError> {
    let quotable = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
    };
    if !quotable(peer_identifier) {
        return Err(PeerCheckError::RequirementInvalid { status: 0 });
    }
    match team_identifier {
        None => Ok(format!("identifier \"{peer_identifier}\"")),
        Some(team)
            if team.len() == 10
                && team
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()) =>
        {
            Ok(format!(
                "anchor apple generic and certificate leaf[subject.OU] = \"{team}\" \
                 and identifier \"{peer_identifier}\""
            ))
        }
        Some(_) => Err(PeerCheckError::RequirementInvalid { status: 0 }),
    }
}

/// This process's signing identifier and team, from its own signature.
pub fn signing_identity_of_this_process() -> Result<SigningIdentity, PeerCheckError> {
    #[cfg(target_os = "macos")]
    {
        security::signing_identity_of_this_process()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(PeerCheckError::Unsupported)
    }
}

fn verify_peer(descriptor: i32, requirement: &str) -> Result<(), PeerCheckError> {
    #[cfg(target_os = "macos")]
    {
        let token = security::peer_audit_token(descriptor)?;
        security::check_code_with_audit_token(&token, requirement)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (descriptor, requirement);
        Err(PeerCheckError::Unsupported)
    }
}

#[cfg(target_os = "macos")]
#[allow(non_upper_case_globals, non_snake_case)]
mod security {
    use std::ffi::{c_char, c_int, c_void, CStr};

    use super::{PeerCheckError, SigningIdentity};

    type CFTypeRef = *const c_void;
    type CFIndex = isize;
    type CFTypeID = usize;
    type OSStatus = i32;
    type SecCSFlags = u32;

    const errSecSuccess: OSStatus = 0;
    const kSecCSDefaultFlags: SecCSFlags = 0;
    const kSecCSSigningInformation: SecCSFlags = 1 << 1;
    const kCFStringEncodingUTF8: u32 = 0x0800_0100;
    // <sys/un.h>
    const SOL_LOCAL: c_int = 0;
    const LOCAL_PEERTOKEN: c_int = 0x006;
    /// `audit_token_t` is eight `unsigned int`s.
    const AUDIT_TOKEN_SIZE: usize = 32;

    /// Only the address of these CoreFoundation callback tables is used.
    #[repr(C)]
    struct OpaqueCallBacks {
        _private: [u8; 0],
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFTypeDictionaryKeyCallBacks: OpaqueCallBacks;
        static kCFTypeDictionaryValueCallBacks: OpaqueCallBacks;
        fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: CFIndex) -> CFTypeRef;
        fn CFDictionaryCreate(
            allocator: CFTypeRef,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            count: CFIndex,
            key_callbacks: *const OpaqueCallBacks,
            value_callbacks: *const OpaqueCallBacks,
        ) -> CFTypeRef;
        fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFStringCreateWithBytes(
            allocator: CFTypeRef,
            bytes: *const u8,
            length: CFIndex,
            encoding: u32,
            is_external_representation: u8,
        ) -> CFTypeRef;
        fn CFStringGetCString(
            string: CFTypeRef,
            buffer: *mut c_char,
            buffer_size: CFIndex,
            encoding: u32,
        ) -> u8;
        fn CFStringGetTypeID() -> CFTypeID;
        fn CFGetTypeID(object: CFTypeRef) -> CFTypeID;
        fn CFRelease(object: CFTypeRef);
    }

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecGuestAttributeAudit: CFTypeRef;
        static kSecCodeInfoIdentifier: CFTypeRef;
        static kSecCodeInfoTeamIdentifier: CFTypeRef;
        fn SecCodeCopySelf(flags: SecCSFlags, code: *mut CFTypeRef) -> OSStatus;
        fn SecCodeCopyStaticCode(
            code: CFTypeRef,
            flags: SecCSFlags,
            static_code: *mut CFTypeRef,
        ) -> OSStatus;
        fn SecCodeCopySigningInformation(
            code: CFTypeRef,
            flags: SecCSFlags,
            information: *mut CFTypeRef,
        ) -> OSStatus;
        fn SecCodeCopyGuestWithAttributes(
            host: CFTypeRef,
            attributes: CFTypeRef,
            flags: SecCSFlags,
            guest: *mut CFTypeRef,
        ) -> OSStatus;
        fn SecRequirementCreateWithString(
            text: CFTypeRef,
            flags: SecCSFlags,
            requirement: *mut CFTypeRef,
        ) -> OSStatus;
        fn SecCodeCheckValidity(
            code: CFTypeRef,
            flags: SecCSFlags,
            requirement: CFTypeRef,
        ) -> OSStatus;
    }

    extern "C" {
        fn getsockopt(
            socket: c_int,
            level: c_int,
            option: c_int,
            value: *mut c_void,
            length: *mut u32,
        ) -> c_int;
    }

    /// A CoreFoundation object this module created or copied, released once.
    struct Owned(CFTypeRef);

    impl Owned {
        /// Takes ownership of what a `Create` or `Copy` call returned.
        fn new(object: CFTypeRef) -> Option<Self> {
            (!object.is_null()).then_some(Self(object))
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a non-null object this module owns one
            // reference to (checked in `Owned::new`), released exactly once.
            unsafe { CFRelease(self.0) }
        }
    }

    pub(super) fn peer_audit_token(
        descriptor: c_int,
    ) -> Result<[u8; AUDIT_TOKEN_SIZE], PeerCheckError> {
        let mut token = [0_u8; AUDIT_TOKEN_SIZE];
        let mut length = AUDIT_TOKEN_SIZE as u32;
        // SAFETY: `token` is writable for `length` bytes, and the kernel writes
        // at most that many.
        let result = unsafe {
            getsockopt(
                descriptor,
                SOL_LOCAL,
                LOCAL_PEERTOKEN,
                token.as_mut_ptr().cast(),
                &mut length,
            )
        };
        if result != 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return Err(PeerCheckError::PeerTokenUnavailable { errno });
        }
        if length as usize != AUDIT_TOKEN_SIZE {
            return Err(PeerCheckError::PeerTokenUnavailable { errno: 0 });
        }
        Ok(token)
    }

    pub(super) fn check_code_with_audit_token(
        token: &[u8; AUDIT_TOKEN_SIZE],
        requirement: &str,
    ) -> Result<(), PeerCheckError> {
        let requirement = compile_requirement(requirement)?;
        // SAFETY: every pointer passed below is either null where the API
        // allows it or a live object owned by an `Owned` for the whole call.
        unsafe {
            let token_data = Owned::new(CFDataCreate(
                std::ptr::null(),
                token.as_ptr(),
                AUDIT_TOKEN_SIZE as CFIndex,
            ))
            .ok_or(PeerCheckError::PeerCodeUnavailable { status: 0 })?;
            let keys = [kSecGuestAttributeAudit];
            let values = [token_data.0];
            let attributes = Owned::new(CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
                std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
            ))
            .ok_or(PeerCheckError::PeerCodeUnavailable { status: 0 })?;

            let mut guest: CFTypeRef = std::ptr::null();
            let status = SecCodeCopyGuestWithAttributes(
                std::ptr::null(),
                attributes.0,
                kSecCSDefaultFlags,
                &mut guest,
            );
            if status != errSecSuccess {
                return Err(PeerCheckError::PeerCodeUnavailable { status });
            }
            let guest =
                Owned::new(guest).ok_or(PeerCheckError::PeerCodeUnavailable { status: 0 })?;

            let status = SecCodeCheckValidity(guest.0, kSecCSDefaultFlags, requirement.0);
            if status != errSecSuccess {
                return Err(PeerCheckError::SignatureRejected { status });
            }
        }
        Ok(())
    }

    fn compile_requirement(text: &str) -> Result<Owned, PeerCheckError> {
        // SAFETY: `text` outlives the call, and the string and requirement
        // returned are owned by `Owned`.
        unsafe {
            let string = Owned::new(CFStringCreateWithBytes(
                std::ptr::null(),
                text.as_ptr(),
                text.len() as CFIndex,
                kCFStringEncodingUTF8,
                0,
            ))
            .ok_or(PeerCheckError::RequirementInvalid { status: 0 })?;
            let mut requirement: CFTypeRef = std::ptr::null();
            let status =
                SecRequirementCreateWithString(string.0, kSecCSDefaultFlags, &mut requirement);
            if status != errSecSuccess {
                return Err(PeerCheckError::RequirementInvalid { status });
            }
            Owned::new(requirement).ok_or(PeerCheckError::RequirementInvalid { status: 0 })
        }
    }

    pub(super) fn signing_identity_of_this_process() -> Result<SigningIdentity, PeerCheckError> {
        // SAFETY: each out-pointer is a local the call fills, and each object
        // returned is owned by an `Owned` before the next call.
        unsafe {
            let mut code: CFTypeRef = std::ptr::null();
            let status = SecCodeCopySelf(kSecCSDefaultFlags, &mut code);
            if status != errSecSuccess {
                return Err(PeerCheckError::OwnSignatureUnavailable { status });
            }
            let code =
                Owned::new(code).ok_or(PeerCheckError::OwnSignatureUnavailable { status: 0 })?;

            let mut static_code: CFTypeRef = std::ptr::null();
            let status = SecCodeCopyStaticCode(code.0, kSecCSDefaultFlags, &mut static_code);
            if status != errSecSuccess {
                return Err(PeerCheckError::OwnSignatureUnavailable { status });
            }
            let static_code = Owned::new(static_code)
                .ok_or(PeerCheckError::OwnSignatureUnavailable { status: 0 })?;

            let mut information: CFTypeRef = std::ptr::null();
            let status = SecCodeCopySigningInformation(
                static_code.0,
                kSecCSSigningInformation,
                &mut information,
            );
            if status != errSecSuccess {
                return Err(PeerCheckError::OwnSignatureUnavailable { status });
            }
            let information = Owned::new(information)
                .ok_or(PeerCheckError::OwnSignatureUnavailable { status: 0 })?;

            Ok(SigningIdentity {
                identifier: string_value(information.0, kSecCodeInfoIdentifier),
                team_identifier: string_value(information.0, kSecCodeInfoTeamIdentifier),
            })
        }
    }

    /// The string under `key` in `dictionary`, if there is one.
    ///
    /// # Safety
    /// `dictionary` must be a live CFDictionary and `key` a live CFString.
    unsafe fn string_value(dictionary: CFTypeRef, key: CFTypeRef) -> Option<String> {
        let value = CFDictionaryGetValue(dictionary, key);
        if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
            return None;
        }
        // Team and signing identifiers are short; anything that does not fit
        // is not something a requirement could quote anyway.
        let mut buffer = [0 as c_char; 256];
        if CFStringGetCString(
            value,
            buffer.as_mut_ptr(),
            buffer.len() as CFIndex,
            kCFStringEncodingUTF8,
        ) == 0
        {
            return None;
        }
        CStr::from_ptr(buffer.as_ptr())
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::{requirement_for, PeerCheckError, PeerPolicy, APP_IDENTIFIER, SERVICE_IDENTIFIER};

    #[test]
    fn a_team_signed_build_pins_the_team_and_the_identifier() {
        assert_eq!(
            requirement_for(APP_IDENTIFIER, Some("ABCDE12345")).unwrap(),
            "anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\" \
             and identifier \"com.velvt.mac\""
        );
        assert_eq!(
            requirement_for(SERVICE_IDENTIFIER, Some("ABCDE12345")).unwrap(),
            "anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\" \
             and identifier \"com.velvt.service\""
        );
    }

    #[test]
    fn an_ad_hoc_build_has_no_team_to_pin_and_requires_the_identifier() {
        assert_eq!(
            requirement_for(APP_IDENTIFIER, None).unwrap(),
            "identifier \"com.velvt.mac\""
        );
    }

    /// The team is read from this process's own signature and spliced into
    /// requirement language, so anything that is not a team identifier's shape
    /// is refused rather than quoted.
    #[test]
    fn a_team_or_identifier_that_cannot_be_quoted_is_refused() {
        for team in [
            "",
            "abcde12345",
            "ABCDE1234",
            "ABCDE123456",
            "ABCDE\"1234",
            "ABCDE 1234",
        ] {
            assert_eq!(
                requirement_for(APP_IDENTIFIER, Some(team)),
                Err(PeerCheckError::RequirementInvalid { status: 0 }),
                "team {team:?}"
            );
        }
        for identifier in ["", "com.velvt.mac\" or anchor apple", "com velvt"] {
            assert_eq!(
                requirement_for(identifier, None),
                Err(PeerCheckError::RequirementInvalid { status: 0 }),
                "identifier {identifier:?}"
            );
        }
    }

    /// `cargo test` builds without `VELVT_PEER_SIGNATURE_CHECK=enforce`, as a
    /// Debug app's helper does, and then nothing is checked. Built with it, the
    /// helper always checks: it never falls back to serving everyone.
    #[test]
    fn only_a_helper_built_for_a_release_app_checks_its_peer() {
        if crate::build_info::PEER_SIGNATURE_CHECK {
            assert!(matches!(
                PeerPolicy::for_this_build(),
                PeerPolicy::Require(_) | PeerPolicy::Unavailable(_)
            ));
        } else {
            assert_eq!(PeerPolicy::for_this_build(), PeerPolicy::Skip);
        }
        assert_eq!(PeerPolicy::Skip.describe(), Ok("none"));
    }

    #[test]
    fn a_helper_that_cannot_read_its_own_signature_serves_nobody() {
        let error = PeerCheckError::OwnSignatureUnavailable { status: -67062 };
        let policy = PeerPolicy::Unavailable(error);
        let (_listener, client, _server) = socket_pair();
        assert_eq!(policy.verify(&client), Err(error));
        assert_eq!(policy.describe(), Err(error));
        assert_eq!(error.code(), "ipc_own_signature_unavailable");
        assert_eq!(error.status(), -67062);
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use super::super::{signing_identity_of_this_process, PeerCheckError, PeerPolicy};
        use super::socket_pair;

        /// errSecCSReqFailed: the code is validly signed but does not satisfy
        /// the requirement.
        const ERR_SEC_CS_REQ_FAILED: i32 = -67050;
        /// <sys/errno.h>
        const ENOTCONN: i32 = 57;

        /// What a Release helper checks: the app's identifier, and this
        /// process's own team when it has one.
        #[test]
        fn the_release_check_is_built_from_this_processs_own_signature() {
            let identity = signing_identity_of_this_process().unwrap();
            assert_eq!(
                PeerPolicy::same_signer_as_this_process("com.velvt.mac"),
                PeerPolicy::Require(
                    super::super::requirement_for(
                        "com.velvt.mac",
                        identity.team_identifier.as_deref()
                    )
                    .unwrap()
                )
            );
        }

        /// The test binary is what any other program on the Mac looks like to
        /// the helper: linker-signed ad hoc, with no team and a made-up
        /// identifier. It is refused by both the Release requirement for the app
        /// and the ad-hoc one.
        #[test]
        fn a_peer_that_is_not_the_signed_app_is_refused() {
            let (_listener, client, server) = socket_pair();
            for policy in [
                PeerPolicy::Require(
                    super::super::requirement_for("com.velvt.mac", Some("ABCDE12345")).unwrap(),
                ),
                PeerPolicy::Require(super::super::requirement_for("com.velvt.mac", None).unwrap()),
            ] {
                let error = policy.verify(&server).unwrap_err();
                assert_eq!(
                    error,
                    PeerCheckError::SignatureRejected {
                        status: ERR_SEC_CS_REQ_FAILED
                    }
                );
                assert_eq!(error.code(), "ipc_peer_signature_rejected");
            }
            drop(client);
        }

        /// The other half: the same check passes a peer that does satisfy the
        /// requirement, so the refusal above is the requirement's doing.
        #[test]
        fn a_peer_that_satisfies_the_requirement_is_served() {
            let identifier = signing_identity_of_this_process()
                .unwrap()
                .identifier
                .expect("the test binary is linker-signed");
            let (_listener, _client, server) = socket_pair();
            let policy = PeerPolicy::Require(format!("identifier \"{identifier}\""));
            assert_eq!(policy.verify(&server), Ok(()));
        }

        /// The audit token names the process at the other end, not this one: a
        /// separate, Apple-signed `nc` connecting is refused as the app and
        /// passes as Apple's own code.
        #[test]
        fn the_check_is_made_on_the_connecting_process_not_on_the_helper() {
            let netcat = std::path::Path::new("/usr/bin/nc");
            if !netcat.exists() {
                eprintln!("SKIP: /usr/bin/nc is not installed");
                return;
            }
            let dir = std::path::PathBuf::from(format!("/tmp/velvt-peer-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("s.sock");
            let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
            let mut child = std::process::Command::new(netcat)
                .arg("-U")
                .arg(&path)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let (server, _) = listener.accept().unwrap();

            let as_app =
                PeerPolicy::Require(super::super::requirement_for("com.velvt.mac", None).unwrap());
            let as_apple = PeerPolicy::Require("anchor apple".to_owned());
            let refused = as_app.verify(&server);
            let accepted = as_apple.verify(&server);

            child.kill().ok();
            child.wait().ok();
            std::fs::remove_dir_all(&dir).ok();
            assert_eq!(
                refused,
                Err(PeerCheckError::SignatureRejected {
                    status: ERR_SEC_CS_REQ_FAILED
                })
            );
            assert_eq!(accepted, Ok(()));
        }

        /// A peer that has already gone cannot be vouched for, even under a
        /// requirement the process at that end did satisfy.
        #[test]
        fn a_closed_peer_has_no_token_to_check() {
            let identifier = signing_identity_of_this_process()
                .unwrap()
                .identifier
                .expect("the test binary is linker-signed");
            let (_listener, client, server) = socket_pair();
            drop(server);
            let policy = PeerPolicy::Require(format!("identifier \"{identifier}\""));
            assert_eq!(
                policy.verify(&client),
                Err(PeerCheckError::PeerTokenUnavailable { errno: ENOTCONN })
            );
        }
    }

    /// A connected pair over a real Unix socket under a short `/tmp` path (see
    /// `tests/unix_socket_smoke.rs` for why not `temp_dir()`).
    fn socket_pair() -> (
        SocketDirGuard,
        std::os::unix::net::UnixStream,
        std::os::unix::net::UnixStream,
    ) {
        let dir = std::path::PathBuf::from(format!("/tmp/velvt-peer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let client = std::os::unix::net::UnixStream::connect(&path).unwrap();
        let (server, _) = listener.accept().unwrap();
        (SocketDirGuard(dir), client, server)
    }

    struct SocketDirGuard(std::path::PathBuf);

    impl Drop for SocketDirGuard {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }
}
