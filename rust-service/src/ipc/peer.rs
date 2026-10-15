//! The check the helper makes on the process at the other end of the socket
//! before it serves the connection (design:
//! `docs/architecture/personal-vault-and-devices.md` §9.3, PR A0).
//!
//! The socket's directory is owner-only (`0700`). That keeps other users out,
//! but not another process running as the same person, which could connect and
//! receive what the helper pushes, sessions included. In a Release build the
//! helper asks the kernel which process is at the other end (its audit token,
//! from `LOCAL_PEERTOKEN`) and serves it only when two things hold:
//!
//! - it is the process that launched this helper. The app starts the helper as
//!   its own child (`ServiceProcessLauncher`), so this is the app, and only
//!   while it runs;
//! - its code signature satisfies a requirement built from the helper's own:
//!   the same Apple-issued team, and the app's identifier.
//!
//! The signature alone is not enough. The audit token describes whatever
//! image the connecting process ID runs when it is queried, so a process could
//! connect, fork a child that keeps the socket, and exec a genuinely signed
//! binary: the check would pass on the new image while the child reads what
//! the helper sends. Pinning the peer to the helper's parent closes that, since
//! no other process can be the parent, and the signature then confirms the
//! parent is the app. The app makes the mirror check on the helper
//! (`swift-client/Sources/VelvtMac/IPC/SocketPeerCheck.swift`), pinning its
//! peer to its own child.
//!
//! Security.framework, CoreFoundation and libbsm are called through the small
//! FFI in [`security`], so the check adds no crate.

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
    /// A helper built for a Release app: only `process` is served, and only
    /// when its signature satisfies `requirement` (code-signing requirement
    /// language).
    Require {
        requirement: String,
        process: PeerProcess,
    },
    /// A helper built for a Release app that could not read its own signature,
    /// and so cannot say whom to trust. It serves nobody.
    Unavailable(PeerCheckError),
}

/// Which process may be at the other end of the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerProcess {
    /// The process that launched this helper, while it is still running. The
    /// app starts its helper as a child, so in a Release build this is the
    /// app. An orphaned helper (adopted by launchd) serves nobody.
    Parent,
    /// The process with this ID. Tests use it for a peer they started, or for
    /// themselves.
    Id(u32),
}

impl PeerProcess {
    /// Whether the process at the other end, `peer`, is this one, given this
    /// helper's parent `parent` (`std::os::unix::process::parent_id`).
    pub fn admits(self, peer: u32, parent: u32) -> Result<(), PeerCheckError> {
        let expected = match self {
            // launchd (1) adopts a helper whose app has exited. It never
            // connects, and no other process can be the app that left.
            Self::Parent if parent <= 1 => return Err(PeerCheckError::ParentExited),
            Self::Parent => parent,
            Self::Id(id) => id,
        };
        if peer == expected {
            Ok(())
        } else {
            Err(PeerCheckError::UnexpectedProcess)
        }
    }
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
    /// The peer is not the process the policy names: in a Release build, not
    /// the app that launched this helper.
    #[error("the IPC peer is not the process this helper serves")]
    UnexpectedProcess,
    /// The app that launched this helper has exited, so there is nobody left
    /// to serve.
    #[error("the app that launched this helper has exited")]
    ParentExited,
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
            Self::UnexpectedProcess => "ipc_peer_unexpected_process",
            Self::ParentExited => "ipc_peer_parent_exited",
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
            Self::UnexpectedProcess | Self::ParentExited | Self::Unsupported => 0,
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
    /// built for a Release app (`build_info::PEER_SIGNATURE_CHECK`), and then
    /// the app that launched this helper, held to a requirement built from
    /// this helper's own signature.
    pub fn for_this_build() -> Self {
        if crate::build_info::PEER_SIGNATURE_CHECK {
            Self::same_signer_as_this_process(APP_IDENTIFIER, PeerProcess::Parent)
        } else {
            Self::Skip
        }
    }

    /// Requires `process`, signed by this process's team with
    /// `peer_identifier`.
    pub fn same_signer_as_this_process(peer_identifier: &str, process: PeerProcess) -> Self {
        let identity = match signing_identity_of_this_process() {
            Ok(identity) => identity,
            Err(error) => return Self::Unavailable(error),
        };
        match requirement_for(peer_identifier, identity.team_identifier.as_deref()) {
            Ok(requirement) => Self::Require {
                requirement,
                process,
            },
            Err(error) => Self::Unavailable(error),
        }
    }

    /// What `velvt-service --peer-requirement` prints: `none` when nothing is
    /// checked, otherwise the code requirement. `scripts/verify_release.sh`
    /// refuses a Release app whose helper answers `none`, and checks the app
    /// against the requirement with `codesign -R`. Whenever there is a
    /// requirement, the peer is also pinned to the helper's parent; that part
    /// is not a property of the signed artifact, so it is not printed.
    pub fn describe(&self) -> Result<&str, PeerCheckError> {
        match self {
            Self::Skip => Ok("none"),
            Self::Require { requirement, .. } => Ok(requirement),
            Self::Unavailable(error) => Err(*error),
        }
    }

    /// Checks the process connected on `socket`. `Ok` means serve it.
    pub fn verify(&self, socket: &impl AsRawFd) -> Result<(), PeerCheckError> {
        match self {
            Self::Skip => Ok(()),
            Self::Unavailable(error) => Err(*error),
            Self::Require {
                requirement,
                process,
            } => verify_peer(socket.as_raw_fd(), requirement, *process),
        }
    }
}

/// The requirement for a peer signed with `peer_identifier` by the same team.
///
/// With a team (every Developer ID build, which `verify_release.sh --mode
/// production` insists on), the peer must chain to Apple, carry that team in
/// its leaf certificate, and have the identifier. Without one (an ad-hoc
/// `make package-release` build, which never leaves the machine that built it)
/// there is no team to pin, so the identifier alone is required. That
/// authenticates nothing, since anyone can sign ad hoc under any identifier;
/// it only keeps a local build from refusing itself. In such a build the pin
/// to the helper's parent is the whole of the protection.
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

fn verify_peer(
    descriptor: i32,
    requirement: &str,
    process: PeerProcess,
) -> Result<(), PeerCheckError> {
    #[cfg(target_os = "macos")]
    {
        // One token for both questions, and in this order. The token names
        // the peer by process ID and pid version, the version changing on
        // every fork and exec. `SecCodeCheckValidity` fails unless the process
        // still has that version when it runs, so a pass means the process
        // that was the helper's parent in between is the one whose signature
        // was checked: it cannot have exec'd or been replaced in between.
        let token = security::peer_audit_token(descriptor)?;
        process.admits(
            security::audit_token_pid(&token),
            std::os::unix::process::parent_id(),
        )?;
        security::check_code_with_audit_token(&token, requirement)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (descriptor, requirement, process);
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

    /// `audit_token_t` (<mach/message.h>): eight `unsigned int`s, naming a
    /// process by ID and pid version among other things.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct AuditToken {
        val: [u32; 8],
    }

    const AUDIT_TOKEN_SIZE: usize = std::mem::size_of::<AuditToken>();

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

    #[link(name = "bsm")]
    extern "C" {
        fn audit_token_to_pid(token: AuditToken) -> c_int;
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

    pub(super) fn peer_audit_token(descriptor: c_int) -> Result<AuditToken, PeerCheckError> {
        let mut token = AuditToken { val: [0; 8] };
        let mut length = AUDIT_TOKEN_SIZE as u32;
        // SAFETY: `token` is writable for `length` bytes, and the kernel writes
        // at most that many.
        let result = unsafe {
            getsockopt(
                descriptor,
                SOL_LOCAL,
                LOCAL_PEERTOKEN,
                std::ptr::addr_of_mut!(token).cast(),
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

    /// The ID of the process `token` names (`audit_token_to_pid`).
    pub(super) fn audit_token_pid(token: &AuditToken) -> u32 {
        // SAFETY: a plain function of the token's value.
        let pid = unsafe { audit_token_to_pid(*token) };
        u32::try_from(pid).unwrap_or(0)
    }

    pub(super) fn check_code_with_audit_token(
        token: &AuditToken,
        requirement: &str,
    ) -> Result<(), PeerCheckError> {
        let requirement = compile_requirement(requirement)?;
        // SAFETY: every pointer passed below is either null where the API
        // allows it or a live object owned by an `Owned` for the whole call.
        unsafe {
            let token_data = Owned::new(CFDataCreate(
                std::ptr::null(),
                std::ptr::addr_of!(*token).cast(),
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
    use super::{
        requirement_for, PeerCheckError, PeerPolicy, PeerProcess, APP_IDENTIFIER,
        SERVICE_IDENTIFIER,
    };

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
    /// helper always checks, and only ever its parent: it never falls back to
    /// serving everyone.
    #[test]
    fn only_a_helper_built_for_a_release_app_checks_its_peer() {
        if crate::build_info::PEER_SIGNATURE_CHECK {
            assert!(matches!(
                PeerPolicy::for_this_build(),
                PeerPolicy::Require {
                    process: PeerProcess::Parent,
                    ..
                } | PeerPolicy::Unavailable(_)
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

    /// A Release helper serves the process that launched it and nobody else,
    /// and nobody at all once that process has exited and launchd has adopted
    /// the helper.
    #[test]
    fn only_the_app_that_launched_the_helper_is_served() {
        assert_eq!(PeerProcess::Parent.admits(4242, 4242), Ok(()));
        assert_eq!(
            PeerProcess::Parent.admits(4243, 4242),
            Err(PeerCheckError::UnexpectedProcess)
        );
        for peer in [1, 4242] {
            assert_eq!(
                PeerProcess::Parent.admits(peer, 1),
                Err(PeerCheckError::ParentExited)
            );
        }
        assert_eq!(PeerProcess::Id(4243).admits(4243, 4242), Ok(()));
        assert_eq!(
            PeerProcess::Id(4243).admits(4242, 4242),
            Err(PeerCheckError::UnexpectedProcess)
        );
        assert_eq!(
            PeerCheckError::UnexpectedProcess.code(),
            "ipc_peer_unexpected_process"
        );
        assert_eq!(
            PeerCheckError::ParentExited.code(),
            "ipc_peer_parent_exited"
        );
        assert_eq!(PeerCheckError::ParentExited.status(), 0);
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use std::io::{Read, Write};
        use std::path::{Path, PathBuf};
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        use super::super::security::{
            audit_token_pid, check_code_with_audit_token, peer_audit_token,
        };
        use super::super::{
            signing_identity_of_this_process, PeerCheckError, PeerPolicy, PeerProcess,
        };
        use super::socket_pair;

        /// errSecCSReqFailed: the code is validly signed but does not satisfy
        /// the requirement.
        const ERR_SEC_CS_REQ_FAILED: i32 = -67050;
        /// <sys/errno.h>
        const ENOTCONN: i32 = 57;
        /// What `/bin/sleep` is signed as, and what `/bin/zsh` is not.
        const SLEEP_ONLY: &str = "anchor apple and identifier \"com.apple.sleep\"";

        fn this_binarys_identifier() -> String {
            signing_identity_of_this_process()
                .unwrap()
                .identifier
                .expect("the test binary is linker-signed")
        }

        fn this_process() -> PeerProcess {
            PeerProcess::Id(std::process::id())
        }

        /// What a Release helper checks: the app's identifier, and this
        /// process's own team when it has one, on the process that launched it.
        #[test]
        fn the_release_check_is_built_from_this_processs_own_signature() {
            let identity = signing_identity_of_this_process().unwrap();
            assert_eq!(
                PeerPolicy::same_signer_as_this_process("com.velvt.mac", PeerProcess::Parent),
                PeerPolicy::Require {
                    requirement: super::super::requirement_for(
                        "com.velvt.mac",
                        identity.team_identifier.as_deref()
                    )
                    .unwrap(),
                    process: PeerProcess::Parent,
                }
            );
        }

        /// The token names the process at the other end. Here both ends are
        /// this process.
        #[test]
        fn the_token_names_the_process_at_the_other_end() {
            let (_listener, _client, server) = socket_pair();
            let token = peer_audit_token(std::os::fd::AsRawFd::as_raw_fd(&server)).unwrap();
            assert_eq!(audit_token_pid(&token), std::process::id());
        }

        /// The test binary is what any other program on the Mac looks like to
        /// the helper: linker-signed ad hoc, with no team and a made-up
        /// identifier. It is refused by both the Release requirement for the app
        /// and the ad-hoc one, even as the expected process.
        #[test]
        fn a_peer_that_is_not_the_signed_app_is_refused() {
            let (_listener, client, server) = socket_pair();
            for requirement in [
                super::super::requirement_for("com.velvt.mac", Some("ABCDE12345")).unwrap(),
                super::super::requirement_for("com.velvt.mac", None).unwrap(),
            ] {
                let policy = PeerPolicy::Require {
                    requirement,
                    process: this_process(),
                };
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

        /// The other half: the same check passes a peer that is the expected
        /// process and satisfies the requirement, so the refusals above are the
        /// requirement's doing.
        #[test]
        fn a_peer_that_satisfies_the_requirement_is_served() {
            let (_listener, _client, server) = socket_pair();
            let policy = PeerPolicy::Require {
                requirement: format!("identifier \"{}\"", this_binarys_identifier()),
                process: this_process(),
            };
            assert_eq!(policy.verify(&server), Ok(()));
        }

        /// The audit token names the process at the other end, not this one: a
        /// separate, Apple-signed `nc` connecting is refused as the app and
        /// passes as Apple's own code. It is this test's child, not its parent,
        /// so the Release pin refuses it before its signature is looked at.
        #[test]
        fn the_check_is_made_on_the_connecting_process_not_on_the_helper() {
            let netcat = Path::new("/usr/bin/nc");
            if !netcat.exists() {
                eprintln!("SKIP: /usr/bin/nc is not installed");
                return;
            }
            let dir = SocketDir::new();
            let listener = std::os::unix::net::UnixListener::bind(dir.path()).unwrap();
            let mut child = Command::new(netcat)
                .arg("-U")
                .arg(dir.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let (server, _) = listener.accept().unwrap();
            let netcat_process = PeerProcess::Id(child.id());

            let as_app = PeerPolicy::Require {
                requirement: super::super::requirement_for("com.velvt.mac", None).unwrap(),
                process: netcat_process,
            };
            let as_apple = PeerPolicy::Require {
                requirement: "anchor apple".to_owned(),
                process: netcat_process,
            };
            let as_released = PeerPolicy::Require {
                requirement: "anchor apple".to_owned(),
                process: PeerProcess::Parent,
            };
            let token = peer_audit_token(std::os::fd::AsRawFd::as_raw_fd(&server));
            let refused = as_app.verify(&server);
            let accepted = as_apple.verify(&server);
            let not_the_parent = as_released.verify(&server);

            child.kill().ok();
            child.wait().ok();
            assert_eq!(audit_token_pid(&token.unwrap()), child.id());
            assert_eq!(
                refused,
                Err(PeerCheckError::SignatureRejected {
                    status: ERR_SEC_CS_REQ_FAILED
                })
            );
            assert_eq!(accepted, Ok(()));
            assert_eq!(not_the_parent, Err(PeerCheckError::UnexpectedProcess));
        }

        /// A peer that has already gone cannot be vouched for, even under a
        /// requirement the process at that end did satisfy.
        #[test]
        fn a_closed_peer_has_no_token_to_check() {
            let (_listener, client, server) = socket_pair();
            drop(server);
            let policy = PeerPolicy::Require {
                requirement: format!("identifier \"{}\"", this_binarys_identifier()),
                process: this_process(),
            };
            assert_eq!(
                policy.verify(&client),
                Err(PeerCheckError::PeerTokenUnavailable { errno: ENOTCONN })
            );
        }

        /// The attack the signature alone does not stop (review of PR #88): a
        /// process connects, then execs a binary that satisfies the
        /// requirement. The audit token describes whatever the connecting
        /// process ID runs when it is read, so the signature check passes on
        /// the new image, while a child forked before the exec could still be
        /// holding the socket. Pinning the peer to the helper's parent refuses
        /// it. Here zsh connects, waits for a line on stdin, and execs
        /// `/bin/sleep`, which `SLEEP_ONLY` accepts and zsh does not satisfy.
        #[test]
        fn a_peer_that_execs_a_signed_binary_after_connecting_is_refused() {
            let Some(zsh) = zsh_with_sockets() else {
                return;
            };
            let dir = SocketDir::new();
            let listener = std::os::unix::net::UnixListener::bind(dir.path()).unwrap();
            let mut peer = Command::new(zsh)
                .args([
                    "-fc",
                    "zmodload zsh/net/socket && zsocket \"$1\" && read -r && exec /bin/sleep 10",
                    "zsh",
                ])
                .arg(dir.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let (server, _) = listener.accept().unwrap();
            let descriptor = std::os::fd::AsRawFd::as_raw_fd(&server);
            let before = peer_audit_token(descriptor).unwrap();
            assert_eq!(audit_token_pid(&before), peer.id());

            peer.stdin.take().unwrap().write_all(b"\n").unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let after = loop {
                let token = peer_audit_token(descriptor).unwrap();
                if token != before {
                    break token;
                }
                assert!(Instant::now() < deadline, "the peer never exec'd");
                std::thread::sleep(Duration::from_millis(10));
            };
            let signature_alone = check_code_with_audit_token(&after, SLEEP_ONLY);
            let stale_token = check_code_with_audit_token(&before, "anchor apple");
            let released = PeerPolicy::Require {
                requirement: SLEEP_ONLY.to_owned(),
                process: PeerProcess::Parent,
            }
            .verify(&server);

            peer.kill().ok();
            peer.wait().ok();
            // Same process ID, new image.
            assert_eq!(audit_token_pid(&after), peer.id());
            assert_eq!(signature_alone, Ok(()), "the signature alone is fooled");
            // The token's pid version pins the image it was read for, which is
            // what lets one token answer both questions in `verify_peer`.
            assert!(stale_token.is_err(), "a token outlived its image");
            assert_eq!(released, Err(PeerCheckError::UnexpectedProcess));
        }

        /// The Release pin end to end: a helper run as this test's child serves
        /// this process, its parent, and refuses its sibling, which runs the
        /// very same signed binary. The helper is this test binary, re-run as
        /// `role_launched_helper`.
        #[test]
        fn the_app_that_launched_the_helper_is_served_and_its_sibling_is_not() {
            let dir = SocketDir::new();
            let mut helper = rerun_as("role_launched_helper", &dir.path());
            let deadline = Instant::now() + Duration::from_secs(10);
            while !dir.path().exists() {
                assert!(
                    Instant::now() < deadline,
                    "the helper never bound its socket"
                );
                std::thread::sleep(Duration::from_millis(10));
            }

            let sibling = rerun_as("role_sibling_client", &dir.path())
                .wait_with_output()
                .unwrap();
            let mut served = String::new();
            std::os::unix::net::UnixStream::connect(dir.path())
                .unwrap()
                .read_to_string(&mut served)
                .unwrap();
            let mut helper_output = Vec::new();
            helper
                .stdout
                .take()
                .unwrap()
                .read_to_end(&mut helper_output)
                .unwrap();
            helper.wait().ok();
            let helper_verdicts = role_output(&helper_output);

            assert!(
                role_output(&sibling.stdout).contains(&"received 0 bytes".to_owned()),
                "the sibling was sent something: {:?}",
                String::from_utf8_lossy(&sibling.stdout)
            );
            assert_eq!(served, "served\n");
            assert_eq!(
                helper_verdicts,
                ["ipc_peer_unexpected_process", "served"],
                "the helper's verdicts, in order"
            );
        }

        /// Not a test of its own: `the_app_that_launched_the_helper_is_served_
        /// and_its_sibling_is_not` runs this binary again with this as the only
        /// test, and it then plays a Release helper on the socket it is given,
        /// for two connections.
        #[test]
        #[ignore = "run as a child process by the_app_that_launched_the_helper_is_served_and_its_sibling_is_not"]
        fn role_launched_helper() {
            let Some(path) = std::env::var_os(ROLE_SOCKET) else {
                return;
            };
            let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
            let policy = PeerPolicy::Require {
                requirement: format!("identifier \"{}\"", this_binarys_identifier()),
                process: PeerProcess::Parent,
            };
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                match policy.verify(&stream) {
                    Ok(()) => {
                        stream.write_all(b"served\n").unwrap();
                        println!("verdict: served");
                    }
                    Err(error) => println!("verdict: {}", error.code()),
                }
            }
        }

        /// Not a test of its own: connects to the helper above as a second
        /// child of the test process, and reports what it was sent.
        #[test]
        #[ignore = "run as a child process by the_app_that_launched_the_helper_is_served_and_its_sibling_is_not"]
        fn role_sibling_client() {
            let Some(path) = std::env::var_os(ROLE_SOCKET) else {
                return;
            };
            let mut received = Vec::new();
            std::os::unix::net::UnixStream::connect(path)
                .unwrap()
                .read_to_end(&mut received)
                .unwrap();
            println!("verdict: received {} bytes", received.len());
        }

        const ROLE_SOCKET: &str = "VELVT_PEER_TEST_ROLE_SOCKET";

        fn rerun_as(role: &str, socket: &Path) -> std::process::Child {
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    &format!("ipc::peer::tests::macos::{role}"),
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(ROLE_SOCKET, socket)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        }

        /// The `verdict: ` lines a role printed, from among libtest's own
        /// output, which can share a line with them.
        fn role_output(stdout: &[u8]) -> Vec<String> {
            String::from_utf8_lossy(stdout)
                .lines()
                .filter_map(|line| line.split_once("verdict: "))
                .map(|(_, verdict)| verdict.to_owned())
                .collect()
        }

        /// `/bin/zsh` with its socket module, which every supported macOS
        /// ships; skipped, with a note, where it does not.
        fn zsh_with_sockets() -> Option<&'static str> {
            let available = Command::new("/bin/zsh")
                .args(["-fc", "zmodload zsh/net/socket"])
                .status()
                .is_ok_and(|status| status.success());
            if !available {
                eprintln!("SKIP: /bin/zsh with zsh/net/socket is not available");
            }
            available.then_some("/bin/zsh")
        }

        /// A socket path under a fresh `/tmp` directory, removed on drop.
        struct SocketDir(PathBuf);

        impl SocketDir {
            fn new() -> Self {
                let dir = PathBuf::from(format!("/tmp/velvt-peer-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&dir).unwrap();
                Self(dir)
            }

            fn path(&self) -> PathBuf {
                self.0.join("s.sock")
            }
        }

        impl Drop for SocketDir {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).ok();
            }
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
