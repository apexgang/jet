#if os(macOS)
import AppKit
import Observation
import SwiftUI
import UserNotifications

/// What the SwiftUI scenes can't do on their own (design §6.13):
/// - a click on a notification opens its task, even when the click launched Jet;
/// - Jet keeps running after its window closes, like Mail, so notifications and
///   the Dock badge keep working, and a Dock click brings the window back;
/// - the Dock badge counts the tasks that need the person.
///
/// Wire it in `jetApp` (macOS only):
/// `@NSApplicationDelegateAdaptor(JetAppDelegate.self) private var appDelegate`, and
/// `ContentView(session: session).jetAppDelegate(appDelegate, session: session)`
/// inside `Window("Jet", id: "main")`.
@MainActor
final class JetAppDelegate: NSObject, NSApplicationDelegate {
    /// How long a click during launch waits for the first task list.
    static let launchWait: Duration = .seconds(10)
    static let launchPollInterval: Duration = .milliseconds(200)

    private weak var session: DesktopSession?
    private var openMainWindow: (@MainActor () -> Void)?
    /// A click that arrived before the window's session was attached.
    private var pendingRoute: JetNotificationRoute?
    private var openTaskRequest: Task<Void, Never>?
    private var badgeLabel: String?

    // MARK: - Application

    func applicationWillFinishLaunching(_ notification: Notification) {
        // Set before launch finishes, so the click that launched Jet is delivered.
        UNUserNotificationCenter.current().delegate = self
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows { openMainWindow?() }
        return true
    }

    // MARK: - Session

    /// Connects the main window's session. Calling it again only refreshes the
    /// window action.
    func attach(_ session: DesktopSession, openMainWindow: @escaping @MainActor () -> Void) {
        self.openMainWindow = openMainWindow
        guard self.session !== session else { return }
        self.session = session
        observeBadge()
        if let route = pendingRoute {
            pendingRoute = nil
            open(route)
        }
    }

    /// Brings Jet forward and opens the notification's task.
    func open(_ route: JetNotificationRoute) {
        NSApp.activate()
        guard let session else {
            pendingRoute = route
            return
        }
        openMainWindow?()
        openTaskRequest?.cancel()
        openTaskRequest = Task { [weak session] in
            // Wait for the first task list, so restoring the last task at launch
            // can't replace the one the person clicked.
            let deadline = ContinuousClock.now + Self.launchWait
            while let session, session.conversationFreshness == .loading, ContinuousClock.now < deadline {
                try? await Task.sleep(for: Self.launchPollInterval)
                if Task.isCancelled { return }
            }
            guard !Task.isCancelled, let session else { return }
            session.openTask(conversationID: route.conversationID, planeRegistryID: route.planeRegistryID)
            await session.notifications?.removeDelivered(threadIdentifier: route.threadIdentifier)
        }
    }

    // MARK: - Dock badge

    /// Shows the Needs You count and re-arms after every change to it.
    private func observeBadge() {
        guard let session else { return }
        let count = withObservationTracking {
            session.needsYouConversationIDs.count
        } onChange: { [weak self] in
            let delegate = self
            Task { @MainActor in delegate?.observeBadge() }
        }
        let label = JetDockBadge.label(for: count)
        guard label != badgeLabel else { return }
        badgeLabel = label
        NSApp.dockTile.badgeLabel = label
    }
}

extension JetAppDelegate: nonisolated UNUserNotificationCenterDelegate {
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let route = JetNotificationRoute(userInfo: response.notification.request.content.userInfo)
        let dismissed = response.actionIdentifier == UNNotificationDismissActionIdentifier
        completionHandler()
        guard !dismissed else { return }
        Task { @MainActor in
            if let route {
                self.open(route)
            } else {
                NSApp.activate()
            }
        }
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        // Never a banner over the open window; the notification stays in the list.
        completionHandler([.list])
    }
}

/// The Dock badge: the number of tasks that need the person, or nothing.
enum JetDockBadge {
    static func label(for count: Int) -> String? {
        count > 0 ? count.formatted() : nil
    }
}

extension View {
    /// Attaches the main window's session to the app delegate.
    func jetAppDelegate(_ delegate: JetAppDelegate, session: DesktopSession) -> some View {
        modifier(JetAppDelegateAttachment(delegate: delegate, session: session))
    }
}

private struct JetAppDelegateAttachment: ViewModifier {
    let delegate: JetAppDelegate
    let session: DesktopSession

    @Environment(\.openWindow) private var openWindow

    func body(content: Content) -> some View {
        content.task {
            delegate.attach(session, openMainWindow: { openWindow(id: "main") })
        }
    }
}
#endif
