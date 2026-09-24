import Foundation

#if os(macOS)
import UserNotifications
#endif

enum JetNotificationKind: String, Sendable, Equatable {
    case approval
    case completion
    case failure

    var preferenceKey: String {
        switch self {
        case .approval: JetNotificationPreferences.approvalsKey
        case .completion: JetNotificationPreferences.completionsKey
        case .failure: JetNotificationPreferences.failuresKey
        }
    }

    var title: String {
        switch self {
        case .approval: "Approval needed"
        case .completion: "Task completed"
        case .failure: "Task failed"
        }
    }

    var body: String {
        switch self {
        case .approval: "Open Jet to review the request."
        case .completion: "Open Jet to review the result."
        case .failure: "Open Jet to inspect the failure and recovery options."
        }
    }
}

enum JetNotificationPreferences {
    static let approvalsKey = "jet.notifications.approvals"
    static let completionsKey = "jet.notifications.completions"
    static let failuresKey = "jet.notifications.failures"

    static func isEnabled(
        _ kind: JetNotificationKind,
        defaults: UserDefaults = .standard
    ) -> Bool {
        defaults.bool(forKey: kind.preferenceKey)
    }
}

struct JetUserNotification: Sendable, Equatable {
    let eventID: UUID
    let conversationID: UUID
    let kind: JetNotificationKind
}

enum JetNotificationAuthorization: String, Sendable, Equatable {
    case notDetermined
    case denied
    case authorized
    case provisional

    var permitsDelivery: Bool {
        self == .authorized || self == .provisional
    }
}

@MainActor
protocol JetNotificationDelivering: AnyObject {
    func authorizationStatus() async -> JetNotificationAuthorization
    func requestAuthorization() async throws -> JetNotificationAuthorization
    func deliver(_ notification: JetUserNotification) async throws
}

#if os(macOS)
@MainActor
final class SystemJetNotificationCenter: JetNotificationDelivering {
    private static let deliveredEventIDsKey = "jet.notifications.delivered-event-ids"
    private static let maximumRememberedEvents = 512

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func authorizationStatus() async -> JetNotificationAuthorization {
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        return map(settings.authorizationStatus)
    }

    func requestAuthorization() async throws -> JetNotificationAuthorization {
        // Apple recommends requesting notification access only after a person
        // opts in. Jet asks for alerts only; Sound cues remain an independent
        // device-local preference (ASVS 14.3.3).
        _ = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert])
        return await authorizationStatus()
    }

    func deliver(_ notification: JetUserNotification) async throws {
        let identifier = notification.eventID.uuidString.lowercased()
        var delivered = defaults.stringArray(forKey: Self.deliveredEventIDsKey) ?? []
        guard !delivered.contains(identifier) else { return }

        let center = UNUserNotificationCenter.current()
        let settings = await center.notificationSettings()
        guard map(settings.authorizationStatus).permitsDelivery,
              settings.alertSetting == .enabled
        else {
            return
        }

        let content = UNMutableNotificationContent()
        // Notifications can appear on a locked screen. Keep prompts, paths,
        // tool actions, and task names out of their content (ASVS 14.2.3).
        content.title = notification.kind.title
        content.body = notification.kind.body
        content.threadIdentifier = "jet-\(notification.conversationID.uuidString.lowercased())"
        content.interruptionLevel = notification.kind == .completion ? .passive : .active

        let request = UNNotificationRequest(
            identifier: identifier,
            content: content,
            trigger: nil
        )
        try await center.add(request)

        delivered.append(identifier)
        if delivered.count > Self.maximumRememberedEvents {
            delivered.removeFirst(delivered.count - Self.maximumRememberedEvents)
        }
        defaults.set(delivered, forKey: Self.deliveredEventIDsKey)
    }

    private func map(_ status: UNAuthorizationStatus) -> JetNotificationAuthorization {
        switch status {
        case .notDetermined: .notDetermined
        case .denied: .denied
        case .authorized, .ephemeral: .authorized
        case .provisional: .provisional
        @unknown default: .denied
        }
    }
}
#endif
