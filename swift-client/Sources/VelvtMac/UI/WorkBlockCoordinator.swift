import AppKit
import Combine
import Foundation

/// Main-actor presentation bridge for Rust-owned meaningful-work state.
///
/// This type issues direct user/OS lifecycle commands and publishes the exact
/// Rust snapshot. It does not persist intention text, aggregate events, derive
/// evidence, or author behavioral claims.
@MainActor
public final class WorkBlockCoordinator: ObservableObject {
  @Published public private(set) var snapshot: WorkBlockSnapshot?
  @Published public private(set) var commandError: String?

  private let ipcClient: any IPCClientProtocol
  private var cancellables = Set<AnyCancellable>()
  private var sendChain: Task<Void, Never>?

  public init(ipcClient: any IPCClientProtocol) {
    self.ipcClient = ipcClient
  }

  public func start(
    messages: some Publisher<ServerMessage, Never>,
    connectionStatus: some Publisher<ConnectionStatus, Never>,
    workspaceNotifications: NotificationCenter = NSWorkspace.shared.notificationCenter,
    systemNotifications: NotificationCenter = .default
  ) {
    messages
      .receive(on: RunLoop.main)
      .sink { [weak self] message in
        switch message {
        case .workBlockState(let snapshot):
          self?.snapshot = snapshot
          self?.commandError = nil
        case .errorResponse(let error)
        where error.code.hasPrefix("work_block_")
          || error.code.hasPrefix("invalid_work_block_"):
          self?.commandError = error.message
        default:
          break
        }
      }
      .store(in: &cancellables)

    connectionStatus
      .removeDuplicates()
      .receive(on: RunLoop.main)
      .sink { [weak self] status in
        guard status == .connected else { return }
        self?.send(.requestWorkBlockState)
      }
      .store(in: &cancellables)

    workspaceNotifications.publisher(for: NSWorkspace.willSleepNotification)
      .sink { [weak self] _ in self?.reportLifecycle(.sleep) }
      .store(in: &cancellables)
    workspaceNotifications.publisher(for: NSWorkspace.didWakeNotification)
      .sink { [weak self] _ in self?.reportLifecycle(.wake) }
      .store(in: &cancellables)
    systemNotifications.publisher(for: .NSSystemClockDidChange)
      .sink { [weak self] _ in self?.reportLifecycle(.clockChanged) }
      .store(in: &cancellables)
    systemNotifications.publisher(for: .NSSystemTimeZoneDidChange)
      .sink { [weak self] _ in self?.reportLifecycle(.timeZoneChanged) }
      .store(in: &cancellables)
  }

  public func startBlock(
    intention: String?,
    durationSeconds: Int,
    purpose: WorkBlockPurpose?,
    intensity: WorkBlockIntensity
  ) {
    send(
      .startWorkBlock(
        .init(
          intention: intention,
          plannedDurationSeconds: durationSeconds,
          purpose: purpose,
          intensity: intensity
        )))
  }

  public func pause() {
    guard let blockID = snapshot?.blockID else { return }
    send(.pauseWorkBlock(.init(blockID: blockID)))
  }

  public func resume() {
    guard let blockID = snapshot?.blockID else { return }
    send(.resumeWorkBlock(.init(blockID: blockID)))
  }

  public func end() {
    guard let blockID = snapshot?.blockID else { return }
    send(.endWorkBlock(.init(blockID: blockID)))
  }

  public func acceptRecovery() {
    guard let blockID = snapshot?.blockID,
      let actionID = snapshot?.result?.nextAction.actionID
    else { return }
    send(.acceptWorkBlockRecovery(.init(blockID: blockID, actionID: actionID)))
  }

  public func clearLocalData() {
    send(.clearWorkBlockData)
  }

  public func reportLifecycle(_ event: WorkBlockLifecycleEvent) {
    send(.workBlockLifecycle(.init(event: event)))
  }

  private func send(_ message: ClientMessage) {
    commandError = nil
    let previous = sendChain
    sendChain = Task { [weak self, ipcClient] in
      await previous?.value
      do {
        try await ipcClient.send(message)
      } catch {
        await MainActor.run {
          self?.commandError = "The local service is offline. Your work block was not changed."
        }
      }
    }
  }
}

final class UnavailableWorkBlockIPCClient: IPCClientProtocol {
  let incomingMessages: AsyncStream<ServerMessage> = AsyncStream { $0.finish() }
  var connectionStatus: AnyPublisher<ConnectionStatus, Never> {
    Just(.disconnected).eraseToAnyPublisher()
  }
  func connect() async throws {}
  func disconnect() {}
  func send(_ message: ClientMessage) async throws { throw IPCError.notConnected }
}
