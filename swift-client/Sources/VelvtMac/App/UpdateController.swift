import Combine
import Foundation
import Sparkle

public struct AppUpdateConfiguration: Equatable, Sendable {
    public let isEnabled: Bool

    public init(infoDictionary: [String: Any]) {
        guard
            infoDictionary["VelvtUpdaterEnabled"] as? Bool == true,
            let feedURLString = infoDictionary["SUFeedURL"] as? String,
            let feedURL = URLComponents(string: feedURLString),
            feedURL.scheme?.lowercased() == "https",
            feedURL.host?.isEmpty == false,
            feedURL.user == nil,
            feedURL.password == nil,
            feedURL.query == nil,
            feedURL.fragment == nil,
            feedURL.port == nil || feedURL.port == 443,
            let publicKey = infoDictionary["SUPublicEDKey"] as? String,
            Data(base64Encoded: publicKey)?.count == 32,
            infoDictionary["SURequireSignedFeed"] as? Bool == true,
            infoDictionary["SUVerifyUpdateBeforeExtraction"] as? Bool == true,
            Self.numberValue(
                infoDictionary["SUSignedFeedFailureExpirationInterval"]
            ) == 0,
            infoDictionary["SUEnableSystemProfiling"] as? Bool == false
        else {
            isEnabled = false
            return
        }
        isEnabled = true
    }

    public static func load(bundle: Bundle = .main) -> Self {
        Self(infoDictionary: bundle.infoDictionary ?? [:])
    }

    private static func numberValue(_ value: Any?) -> Double? {
        if let number = value as? NSNumber {
            return number.doubleValue
        }
        return nil
    }
}

@MainActor
public protocol UpdateChecking: AnyObject {
    var canCheckForUpdates: Bool { get }
    var canCheckForUpdatesPublisher: AnyPublisher<Bool, Never> { get }
    func checkForUpdates()
}

@MainActor
public final class AppUpdateController: ObservableObject {
    @Published public private(set) var canCheckForUpdates: Bool

    private let configuration: AppUpdateConfiguration
    private let checker: any UpdateChecking
    private var availabilityCancellable: AnyCancellable?

    public init(
        configuration: AppUpdateConfiguration,
        checker: any UpdateChecking
    ) {
        self.configuration = configuration
        self.checker = checker
        canCheckForUpdates = configuration.isEnabled && checker.canCheckForUpdates
        availabilityCancellable = checker.canCheckForUpdatesPublisher
            .receive(on: RunLoop.main)
            .sink { [weak self] isAvailable in
                guard let self else { return }
                self.canCheckForUpdates = configuration.isEnabled && isAvailable
            }
    }

    public static func live(
        configuration: AppUpdateConfiguration = .load()
    ) -> AppUpdateController {
        AppUpdateController(
            configuration: configuration,
            checker: SparkleUpdateChecker(startingUpdater: configuration.isEnabled)
        )
    }

    public static func disabled() -> AppUpdateController {
        AppUpdateController(
            configuration: AppUpdateConfiguration(infoDictionary: [:]),
            checker: DisabledUpdateChecker()
        )
    }

    public func refreshAvailability() {
        canCheckForUpdates = configuration.isEnabled && checker.canCheckForUpdates
    }

    public func checkForUpdates() {
        refreshAvailability()
        guard canCheckForUpdates else { return }
        checker.checkForUpdates()
    }
}

@MainActor
private final class SparkleUpdateChecker: UpdateChecking {
    private let controller: SPUStandardUpdaterController

    init(startingUpdater: Bool) {
        controller = SPUStandardUpdaterController(
            startingUpdater: startingUpdater,
            updaterDelegate: nil,
            userDriverDelegate: nil
        )
    }

    var canCheckForUpdates: Bool {
        controller.updater.canCheckForUpdates
    }

    var canCheckForUpdatesPublisher: AnyPublisher<Bool, Never> {
        controller.updater.publisher(for: \.canCheckForUpdates).eraseToAnyPublisher()
    }

    func checkForUpdates() {
        controller.checkForUpdates(nil)
    }
}

@MainActor
private final class DisabledUpdateChecker: UpdateChecking {
    let canCheckForUpdates = false
    let canCheckForUpdatesPublisher = Just(false).eraseToAnyPublisher()
    func checkForUpdates() {}
}
