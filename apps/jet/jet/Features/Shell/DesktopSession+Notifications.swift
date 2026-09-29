import Foundation
#if os(macOS)
import AppKit
#endif

// The one-time notification offer and the notification settings shortcuts
// (design §6.6 and §6.12). The composer's offer and Settings › General call these
// instead of keeping their own rules.
extension DesktopSession {
    /// Whether the composer offers notifications: once, for a task started in
    /// this app, while permission isn't denied and no notification is turned on.
    var shouldOfferNotifications: Bool {
        guard let conversationID = selectedConversationID else { return false }
        return shouldOfferNotifications(for: conversationID)
    }

    /// Preview sessions have no notification center but still render the offer.
    func shouldOfferNotifications(for conversationID: UUID) -> Bool {
        (notifications != nil || isPreviewSession)
            && NotificationOffer.isEligible(
                offerShown: memory.notificationOfferShown,
                authorization: notificationAuthorization,
                anyPreferenceOn: JetNotificationKind.allCases.contains { notificationPreference($0) },
                startedHere: memory.assistant(for: conversationID) != nil
            )
    }

    /// Turn On: asks for permission and, when it is granted, turns on all three
    /// notifications. Returns whether notifications can be delivered.
    func turnOnNotifications(defaults: UserDefaults = .standard) async -> Bool {
        memory.notificationOfferShown = true
        guard await requestNotificationAuthorization() else { return false }
        for kind in JetNotificationKind.allCases {
            defaults.set(true, forKey: kind.preferenceKey)
        }
        return true
    }

    /// Not Now: the offer isn't shown again.
    func declineNotificationOffer() {
        memory.notificationOfferShown = true
    }

    /// The Notifications pane of System Settings, scrolled to Jet.
    static func notificationSettingsURL(bundleIdentifier: String?) -> URL? {
        var components = URLComponents()
        components.scheme = "x-apple.systempreferences"
        components.path = "com.apple.Notifications-Settings.extension"
        if let bundleIdentifier, !bundleIdentifier.isEmpty {
            components.queryItems = [URLQueryItem(name: "id", value: bundleIdentifier)]
        }
        return components.url
    }

#if os(macOS)
    /// Opens System Settings › Notifications for Jet, where a denied permission is changed.
    func openNotificationSettings() {
        guard let url = Self.notificationSettingsURL(bundleIdentifier: Bundle.main.bundleIdentifier) else {
            return
        }
        NSWorkspace.shared.open(url)
    }
#endif
}
