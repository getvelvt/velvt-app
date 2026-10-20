import Darwin
import Foundation
import Security
import os

/// The check the app makes on the process serving the IPC socket before
/// anything crosses it (`docs/architecture/personal-vault-and-devices.md`
/// §9.3, PR A0).
///
/// The socket's directory is owner-only, which keeps other users out but not
/// another process running as the same person. Such a process could bind the
/// socket path before the helper does and collect the session the app hands
/// over at sign-in. In a Release build the app asks the kernel which process
/// is serving the socket (its audit token) and talks to it only when it is a
/// child of this app, which the helper `ServiceProcessLauncher` started is, and
/// its signature satisfies a requirement built from the app's own: the same
/// Apple-issued team, and the helper's identifier.
///
/// The signature alone is not enough. The audit token describes whatever image
/// the serving process ID runs when it is read, so a process could bind the
/// path, fork a child that keeps the listening socket, and exec a genuinely
/// signed binary: the check would pass on the new image while the child reads
/// what the app sends. Nothing but this app can make a process its child, and
/// the signature then confirms that child is the helper. The helper makes the
/// mirror check on the app (`rust-service/src/ipc/peer.rs`), pinning its peer
/// to its own parent. Debug builds skip both.
struct SocketPeerCheck: Equatable, Sendable {
    enum Policy: Equatable, Sendable {
        /// Debug builds: any process serving the socket is used, as before.
        case skipped
        /// Release builds: only `process`, and only when its signature
        /// satisfies `requirement` (code-signing requirement language).
        case required(String, process: PeerProcess)
        /// A Release build that could not read its own signature, and so
        /// cannot say whom to trust. It trusts nobody.
        case unavailable(code: Int32)
    }

    /// Which process may serve the socket.
    enum PeerProcess: Equatable, Sendable {
        /// A child of this app: the helper `ServiceProcessLauncher` started.
        case childOfThisApp
        /// The process with this identifier. Tests use it for a server they
        /// run themselves.
        case process(pid_t)
    }

    /// The code a refusal carries when the serving process is not the one the
    /// policy names (`EPERM`). Every other code is an `errno` from reading the
    /// token or a Security framework status.
    static let unexpectedProcessCode: Int32 = EPERM

    /// The signing identifier `scripts/sign_release.sh` gives the helper.
    static let helperIdentifier = "com.velvt.service"

    let policy: Policy

    /// The check this build makes: none in Debug; in Release, a child of this
    /// app signed by the same team as the helper.
    static let forThisBuild: SocketPeerCheck = {
        #if DEBUG
            SocketPeerCheck(policy: .skipped)
        #else
            sameSigner(peerIdentifier: helperIdentifier, process: .childOfThisApp)
        #endif
    }()

    /// Requires `process`, signed by this process's team with `peerIdentifier`.
    static func sameSigner(peerIdentifier: String, process: PeerProcess) -> SocketPeerCheck {
        switch SigningIdentity.ofThisProcess() {
        case .failure(let error):
            return SocketPeerCheck(policy: .unavailable(code: error.status))
        case .success(let identity):
            guard
                let requirement = requirement(
                    peerIdentifier: peerIdentifier,
                    teamIdentifier: identity.teamIdentifier
                )
            else {
                return SocketPeerCheck(policy: .unavailable(code: errSecCSReqInvalid))
            }
            return SocketPeerCheck(policy: .required(requirement, process: process))
        }
    }

    /// The requirement for a peer signed with `peerIdentifier` by `teamIdentifier`.
    ///
    /// With a team (every Developer ID build, which `verify_release.sh --mode
    /// production` insists on), the peer must chain to Apple, carry that team
    /// in its leaf certificate, and have the identifier. Without one (an ad-hoc
    /// `make package-release` build, which never leaves the Mac that built it)
    /// there is no team to pin, so the identifier alone is required. That
    /// authenticates nothing, since anyone can sign ad hoc under any
    /// identifier; it only keeps a local build from refusing its own helper. In
    /// such a build the pin to this app's child is the whole of the protection.
    /// `rust-service/src/ipc/peer.rs` builds the helper's half by the same rule.
    static func requirement(peerIdentifier: String, teamIdentifier: String?) -> String? {
        let identifierCharacters = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-")
        guard !peerIdentifier.isEmpty, peerIdentifier.allSatisfy(identifierCharacters.contains) else {
            return nil
        }
        guard let team = teamIdentifier else {
            return "identifier \"\(peerIdentifier)\""
        }
        let teamCharacters = Set("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
        guard team.count == 10, team.allSatisfy(teamCharacters.contains) else {
            return nil
        }
        return "anchor apple generic and certificate leaf[subject.OU] = \"\(team)\" "
            + "and identifier \"\(peerIdentifier)\""
    }

    /// Throws `IPCError.peerRejected` unless the process at the other end of
    /// the connected socket `descriptor` passes.
    ///
    /// The code is an `errno` (positive) when the kernel could not name the
    /// peer, `unexpectedProcessCode` when the peer is not the process the
    /// policy names, or a Security framework status (negative) when its code
    /// could not be found or its signature was refused.
    func verifyPeer(ofSocket descriptor: Int32) throws {
        let requirement: String
        let process: PeerProcess
        switch policy {
        case .skipped:
            return
        case .unavailable(let code):
            throw IPCError.peerRejected(code: code)
        case .required(let text, let expected):
            requirement = text
            process = expected
        }
        // One token for both questions, and in this order. The token names the
        // peer by process ID and pid version, the version changing on every
        // fork and exec, and `SecCodeCheckValidity` fails unless the process
        // still has that version. A pass therefore means the process that was
        // this app's child in between is the one whose signature was checked.
        let token = try Self.peerToken(ofSocket: descriptor)
        try Self.check(Self.processIdentifier(of: token), is: process)
        try Self.checkSignature(of: token, against: requirement)
    }

    /// The audit token of the process at the other end of `descriptor`.
    static func peerToken(ofSocket descriptor: Int32) throws -> audit_token_t {
        var token = audit_token_t()
        var length = socklen_t(MemoryLayout<audit_token_t>.size)
        guard getsockopt(descriptor, SOL_LOCAL, LOCAL_PEERTOKEN, &token, &length) == 0 else {
            throw IPCError.peerRejected(code: errno)
        }
        // A short token would leave the rest zero-filled; the helper's half
        // refuses one too.
        guard length == socklen_t(MemoryLayout<audit_token_t>.size) else {
            throw IPCError.peerRejected(code: EINVAL)
        }
        return token
    }

    /// The process ID `token` names: what libbsm's `audit_token_to_pid`
    /// returns, read here so the app need not link libbsm. A test pins it
    /// against this process's own token.
    static func processIdentifier(of token: audit_token_t) -> pid_t {
        pid_t(bitPattern: token.val.5)
    }

    static func check(_ processIdentifier: pid_t, is expected: PeerProcess) throws {
        switch expected {
        case .process(let expectedIdentifier):
            guard processIdentifier == expectedIdentifier else {
                throw IPCError.peerRejected(code: unexpectedProcessCode)
            }
        case .childOfThisApp:
            var info = proc_bsdinfo()
            let size = Int32(MemoryLayout<proc_bsdinfo>.size)
            guard proc_pidinfo(processIdentifier, PROC_PIDTBSDINFO, 0, &info, size) == size else {
                throw IPCError.peerRejected(code: errno == 0 ? ESRCH : errno)
            }
            guard pid_t(bitPattern: info.pbi_ppid) == getpid() else {
                throw IPCError.peerRejected(code: unexpectedProcessCode)
            }
        }
    }

    /// Throws unless the process `token` names, as it is now, satisfies
    /// `requirement`. Alone this is the signature-only check, which a process
    /// that execs a signed binary after binding the socket passes.
    static func checkSignature(of token: audit_token_t, against requirement: String) throws {
        var token = token
        let tokenData = withUnsafeBytes(of: &token) { Data($0) }
        var compiled: SecRequirement?
        var status = SecRequirementCreateWithString(requirement as CFString, [], &compiled)
        guard status == errSecSuccess, let compiled else {
            throw IPCError.peerRejected(code: status)
        }
        var code: SecCode?
        status = SecCodeCopyGuestWithAttributes(
            nil,
            [kSecGuestAttributeAudit: tokenData] as CFDictionary,
            [],
            &code
        )
        guard status == errSecSuccess, let code else {
            throw IPCError.peerRejected(code: status)
        }
        status = SecCodeCheckValidity(code, [], compiled)
        guard status == errSecSuccess else {
            throw IPCError.peerRejected(code: status)
        }
    }
}

/// The identifier and team this process is signed with, from its own signature.
struct SigningIdentity: Equatable, Sendable {
    let identifier: String?
    let teamIdentifier: String?

    /// The Security framework status that kept this process from reading its
    /// own signature.
    struct Unavailable: Error, Equatable {
        let status: OSStatus
    }

    static func ofThisProcess() -> Result<SigningIdentity, Unavailable> {
        var code: SecCode?
        var status = SecCodeCopySelf([], &code)
        guard status == errSecSuccess, let code else { return .failure(Unavailable(status: status)) }
        var staticCode: SecStaticCode?
        status = SecCodeCopyStaticCode(code, [], &staticCode)
        guard status == errSecSuccess, let staticCode else { return .failure(Unavailable(status: status)) }
        var information: CFDictionary?
        status = SecCodeCopySigningInformation(
            staticCode,
            SecCSFlags(rawValue: kSecCSSigningInformation),
            &information
        )
        guard status == errSecSuccess, let information = information as? [String: Any] else {
            return .failure(Unavailable(status: status))
        }
        return .success(
            SigningIdentity(
                identifier: information[kSecCodeInfoIdentifier as String] as? String,
                teamIdentifier: information[kSecCodeInfoTeamIdentifier as String] as? String
            )
        )
    }
}

enum SocketPeerCheckLog {
    static let shared = Logger(subsystem: "com.velvt.mac", category: "IPC")
}
