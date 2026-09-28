import Foundation

/// Decides which Plane Events become desktop notifications and delivers them.
/// Notifications never carry task content (ASVS 14.2.3).
@MainActor
final class JetNotificationRouter {
    private let notifications: (any JetNotificationDelivering)?
    private let preference: JetNotificationPreference
    /// Called when the system refused to show a notification.
    var onDeliveryFailure: (@MainActor () -> Void)?

    init(
        notifications: (any JetNotificationDelivering)?,
        preference: @escaping JetNotificationPreference
    ) {
        self.notifications = notifications
        self.preference = preference
    }

    func handle(_ event: JetEvent, planeRegistryID: UUID) async {
        guard let notifications,
              let kind = event.notificationKind(),
              preference(kind),
              let conversationID = event.conversationID
        else { return }
        do {
            try await notifications.deliver(
                JetUserNotification(
                    eventID: event.eventID,
                    conversationID: conversationID,
                    kind: kind
                )
            )
        } catch {
            onDeliveryFailure?()
        }
    }
}
