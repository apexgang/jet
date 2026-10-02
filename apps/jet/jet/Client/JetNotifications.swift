import Foundation

#if os(macOS)
import UserNotifications
#endif

/// The three things Jet can notify about (design §6.13). Raw values and
/// preference keys are persisted and never change.
enum JetNotificationKind: String, Sendable, Equatable, CaseIterable {
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

    /// The notification's title. Notifications carry no task content, so the
    /// title and body are fixed per kind (ASVS 14.2.3).
    var title: String {
        switch self {
        case .approval: String(localized: "Task Needs You")
        case .completion: String(localized: "Reply Ready")
        case .failure: String(localized: "Task Stopped")
        }
    }

    var body: String {
        String(localized: "Click to open the task in Jet.")
    }

    /// The Settings toggle that turns this kind on or off.
    var preferenceTitle: String {
        switch self {
        case .approval: String(localized: "A task needs you")
        case .completion: String(localized: "A reply is ready")
        case .failure: String(localized: "A task stops with an error")
        }
    }

    /// A ready reply arrives quietly in Notification Center; the others interrupt.
    var isPassive: Bool { self == .completion }
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
    let planeRegistryID: UUID
    let kind: JetNotificationKind

    /// Where a click on this notification leads.
    var route: JetNotificationRoute {
        JetNotificationRoute(conversationID: conversationID, planeRegistryID: planeRegistryID)
    }
}

/// The task a notification opens. It is the only thing a notification carries:
/// the task and computer IDs, never a title, prompt, path or command.
nonisolated struct JetNotificationRoute: Sendable, Equatable {
    static let conversationKey = "jet.conversation"
    static let planeKey = "jet.plane"

    let conversationID: UUID
    let planeRegistryID: UUID

    init(conversationID: UUID, planeRegistryID: UUID) {
        self.conversationID = conversationID
        self.planeRegistryID = planeRegistryID
    }

    /// Reads a route back from a delivered notification. Both IDs must be valid.
    init?(userInfo: [AnyHashable: Any]) {
        guard let conversation = userInfo[Self.conversationKey] as? String,
              let conversationID = UUID(uuidString: conversation),
              let plane = userInfo[Self.planeKey] as? String,
              let planeRegistryID = UUID(uuidString: plane)
        else { return nil }
        self.init(conversationID: conversationID, planeRegistryID: planeRegistryID)
    }

    var userInfo: [String: String] {
        [
            Self.conversationKey: conversationID.uuidString.lowercased(),
            Self.planeKey: planeRegistryID.uuidString.lowercased(),
        ]
    }

    /// Groups one task's notifications in Notification Center.
    var threadIdentifier: String {
        "jet-\(conversationID.uuidString.lowercased())"
    }
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
    /// Clears a task's delivered notifications once the task is open.
    func removeDelivered(threadIdentifier: String) async
}

extension JetNotificationDelivering {
    func removeDelivered(threadIdentifier: String) async {}
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

        let request = UNNotificationRequest(
            identifier: identifier,
            content: Self.content(for: notification),
            trigger: nil
        )
        try await center.add(request)

        delivered.append(identifier)
        if delivered.count > Self.maximumRememberedEvents {
            delivered.removeFirst(delivered.count - Self.maximumRememberedEvents)
        }
        defaults.set(delivered, forKey: Self.deliveredEventIDsKey)
    }

    func removeDelivered(threadIdentifier: String) async {
        let center = UNUserNotificationCenter.current()
        let identifiers = await center.deliveredNotifications()
            .filter { $0.request.content.threadIdentifier == threadIdentifier }
            .map(\.request.identifier)
        guard !identifiers.isEmpty else { return }
        center.removeDeliveredNotifications(withIdentifiers: identifiers)
    }

    /// What the system shows. Notifications can appear on a locked screen, so
    /// prompts, paths, tool actions and task names stay out (ASVS 14.2.3): the
    /// content is the kind's fixed copy plus the route's two IDs.
    static func content(for notification: JetUserNotification) -> UNMutableNotificationContent {
        let route = notification.route
        let content = UNMutableNotificationContent()
        content.title = notification.kind.title
        content.body = notification.kind.body
        content.userInfo = route.userInfo
        content.threadIdentifier = route.threadIdentifier
        content.interruptionLevel = notification.kind.isPassive ? .passive : .active
        return content
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
