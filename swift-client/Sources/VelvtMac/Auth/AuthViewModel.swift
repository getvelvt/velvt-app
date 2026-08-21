import Combine
import Foundation
import os.log

private let authLogger = Logger(subsystem: "com.velvt.mac", category: "AuthViewModel")

/// Drives the signup, login, logout, and account-deletion UI flows.
///
/// Sends IPC messages via `ipcClient` and observes `AccountStateManager` for
/// responses. Never touches the Keychain directly — all token storage goes
/// through `AccountStateManager`.
@MainActor
public final class AuthViewModel: ObservableObject {
    public enum AuthMode: Equatable {
        case signUp
        case logIn
    }

    @Published public var email: String = ""
    @Published public var password: String = ""
    @Published public var authMode: AuthMode = .signUp
    @Published public private(set) var isLoading: Bool = false
    @Published public private(set) var errorMessage: String?
    @Published public private(set) var showDeleteConfirmation: Bool = false
    @Published public private(set) var connectionStatus: ConnectionStatus = .disconnected

    public var canSubmitCredentials: Bool {
        !isLoading && !email.isEmpty && !password.isEmpty
    }

    private let accountStateManager: AccountStateManager
    private let ipcClient: any IPCClientProtocol
    private let authResponseTimeout: TimeInterval
    private var cancellables = Set<AnyCancellable>()
    private var authTimeoutTask: Task<Void, Never>?

    public init(
        accountStateManager: AccountStateManager,
        ipcClient: any IPCClientProtocol,
        authResponseTimeout: TimeInterval = 30
    ) {
        self.accountStateManager = accountStateManager
        self.ipcClient = ipcClient
        self.authResponseTimeout = authResponseTimeout
        bindStateObservation()
    }

    deinit {
        authTimeoutTask?.cancel()
    }

    // MARK: - Auth actions

    public func signUp() async {
        guard !isLoading else { return }
        guard !email.trimmingCharacters(in: .whitespaces).isEmpty, !password.isEmpty else {
            errorMessage = "Email and password are required."
            return
        }
        guard accountStateManager.beginAuthentication(email: email) else {
            errorMessage = "Authentication is already in progress."
            return
        }
        startLoading()
        do {
            try await ipcClient.send(.signUp(SignUpRequest(email: email, password: password)))
            startAuthResponseTimeout()
        } catch IPCError.notConnected {
            accountStateManager.transition(to: .loggedOut)
            finishLoading(withError: "Service not ready. Please wait a moment and try again.")
        } catch {
            accountStateManager.cancelAuthentication()
            finishLoading(withError: "Connection error. Please try again.")
        }
    }

    public func logIn() async {
        guard !isLoading else { return }
        guard !email.trimmingCharacters(in: .whitespaces).isEmpty, !password.isEmpty else {
            errorMessage = "Email and password are required."
            return
        }
        guard accountStateManager.beginAuthentication(email: email) else {
            errorMessage = "Authentication is already in progress."
            return
        }
        startLoading()
        do {
            try await ipcClient.send(.logIn(LogInRequest(email: email, password: password)))
            startAuthResponseTimeout()
        } catch IPCError.notConnected {
            accountStateManager.transition(to: .loggedOut)
            finishLoading(withError: "Service not ready. Please wait a moment and try again.")
        } catch {
            accountStateManager.cancelAuthentication()
            finishLoading(withError: "Connection error. Please try again.")
        }
    }

    /// Clears Keychain tokens and transitions to `.loggedOut`. Sends a
    /// fire-and-forget `logOut` IPC notification so Rust can revoke the session.
    public func logOut() {
        accountStateManager.logOut()
        Task { try? await ipcClient.send(.logOut) }
    }

    public func requestAccountDeletion() {
        showDeleteConfirmation = true
    }

    public func cancelAccountDeletion() {
        showDeleteConfirmation = false
    }

    /// Sends the `deleteAccount` IPC message after confirmation. Reverts to
    /// `.loggedIn` if the send fails.
    public func confirmAccountDeletion() async {
        showDeleteConfirmation = false
        accountStateManager.transition(to: .pendingErasure)
        do {
            try await ipcClient.send(.deleteAccount)
        } catch {
            accountStateManager.cancelPendingErasure()
            errorMessage = "Could not reach the service. Please try again."
        }
    }

    public func toggleAuthMode() {
        authMode = authMode == .signUp ? .logIn : .signUp
        errorMessage = nil
    }

    // MARK: - Private

    private func bindStateObservation() {
        // Stop loading when state resolves from loggingIn to loggedIn or loggedOut.
        // If state reverts to loggedOut while we were loading and no server error
        // has already been set, the IPC connection dropped — show a generic error.
        accountStateManager.$accountState
            .dropFirst()
            .receive(on: RunLoop.main)
            .sink { [weak self] state in
                guard let self else { return }
                let loading = self.isLoading
                authLogger.debug(
                    "auth.viewModel: accountState → \(String(describing: state)), isLoading=\(loading)"
                )
                guard loading else { return }
                switch state {
                case .loggedIn:
                    self.cancelAuthResponseTimeout()
                    self.isLoading = false
                case .loggedOut:
                    self.cancelAuthResponseTimeout()
                    self.isLoading = false
                    if self.errorMessage == nil {
                        self.errorMessage = "Connection lost. Please try again."
                    }
                default:
                    break
                }
            }
            .store(in: &cancellables)

        // Surface auth failure messages from the manager's fan-out relay.
        accountStateManager.serverMessages
            .compactMap { message -> String? in
                guard case .authFailure(let failure) = message else { return nil }
                return failure.message
            }
            .receive(on: RunLoop.main)
            .sink { [weak self] msg in
                self?.cancelAuthResponseTimeout()
                self?.errorMessage = msg
            }
            .store(in: &cancellables)

        ipcClient.connectionStatus
            .receive(on: RunLoop.main)
            .sink { [weak self] status in
                guard let self else { return }
                let previousStatus = self.connectionStatus
                self.connectionStatus = status
                guard self.isLoading, previousStatus == .connected, status != .connected else { return }
                self.cancelAuthResponseTimeout()
                self.accountStateManager.cancelAuthentication()
                self.finishLoading(withError: "Connection lost. Please try again.")
            }
            .store(in: &cancellables)
    }

    private func startLoading() {
        isLoading = true
        errorMessage = nil
    }

    private func finishLoading(withError message: String? = nil) {
        cancelAuthResponseTimeout()
        isLoading = false
        errorMessage = message
    }

    private func startAuthResponseTimeout() {
        guard isLoading else { return }
        cancelAuthResponseTimeout()
        let timeout = authResponseTimeout
        authTimeoutTask = Task { [weak self] in
            let nanoseconds = UInt64(max(timeout, 0) * 1_000_000_000)
            try? await Task.sleep(nanoseconds: nanoseconds)
            guard !Task.isCancelled else { return }
            await MainActor.run {
                guard let self, self.isLoading else { return }
                self.authTimeoutTask = nil
                self.accountStateManager.cancelAuthentication()
                self.isLoading = false
                self.errorMessage = "Authentication timed out. Please try again."
            }
        }
    }

    private func cancelAuthResponseTimeout() {
        authTimeoutTask?.cancel()
        authTimeoutTask = nil
    }
}
