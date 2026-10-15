import Darwin
import Foundation
import Security
import os

/// The code-signature check the app makes on the process serving the IPC
/// socket before anything crosses it (`docs/architecture/personal-vault-and-devices.md`
/// §9.3, PR A0).
///
/// The socket's directory is owner-only, which keeps other users out but not
/// another process running as the same person. Such a process could bind the
/// socket path before the helper does and collect the session the app hands
/// over at sign-in. In a Release build the app asks the kernel which process
/// is serving the socket (its audit token) and talks to it only when that
/// process's signature satisfies a requirement built from the app's own: the
/// same Apple-issued team, and the helper's identifier. The helper makes the
/// mirror check on the app (`rust-service/src/ipc/peer.rs`). Debug builds skip
/// it.
struct SocketPeerCheck: Equatable, Sendable {
    enum Policy: Equatable, Sendable {
        /// Debug builds: any process serving the socket is used, as before.
        case skipped
        /// Release builds: only a process whose signature satisfies this
        /// requirement (code-signing requirement language).
        case required(String)
        /// A Release build that could not read its own signature, and so
        /// cannot say whom to trust. It trusts nobody.
        case unavailable(code: Int32)
    }

    /// The signing identifier `scripts/sign_release.sh` gives the helper.
    static let helperIdentifier = "com.velvt.service"

    let policy: Policy

    /// The check this build makes: none in Debug, the same signer as this app
    /// in Release.
    static let forThisBuild: SocketPeerCheck = {
        #if DEBUG
            SocketPeerCheck(policy: .skipped)
        #else
            sameSigner(peerIdentifier: helperIdentifier)
        #endif
    }()

    /// Requires a peer signed by this process's team with `peerIdentifier`.
    static func sameSigner(peerIdentifier: String) -> SocketPeerCheck {
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
            return SocketPeerCheck(policy: .required(requirement))
        }
    }

    /// The requirement for a peer signed with `peerIdentifier` by `teamIdentifier`.
    ///
    /// With a team (every Developer ID build, which `verify_release.sh --mode
    /// production` insists on), the peer must chain to Apple, carry that team
    /// in its leaf certificate, and have the identifier. Without one (an ad-hoc
    /// `make package-release` build, which never leaves the Mac that built it)
    /// there is no team to pin, so the identifier alone is required.
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
    /// peer, or a Security framework status (negative) when its code could not
    /// be found or its signature was refused.
    func verifyPeer(ofSocket descriptor: Int32) throws {
        let requirementText: String
        switch policy {
        case .skipped:
            return
        case .unavailable(let code):
            throw IPCError.peerRejected(code: code)
        case .required(let text):
            requirementText = text
        }

        var token = audit_token_t()
        var length = socklen_t(MemoryLayout<audit_token_t>.size)
        guard getsockopt(descriptor, SOL_LOCAL, LOCAL_PEERTOKEN, &token, &length) == 0 else {
            throw IPCError.peerRejected(code: errno)
        }
        let tokenData = withUnsafeBytes(of: &token) { Data($0) }

        var requirement: SecRequirement?
        var status = SecRequirementCreateWithString(requirementText as CFString, [], &requirement)
        guard status == errSecSuccess, let requirement else {
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
        status = SecCodeCheckValidity(code, [], requirement)
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
