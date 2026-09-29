import Foundation
import Testing
#if os(macOS)
import UserNotifications
#endif
@testable import jet

@MainActor
struct NotificationRouterTests {
    private let plane = UUID()

    // MARK: - Task Needs You

    @Test
    func aPermissionRequestInTheBackgroundNotifiesAfterItSettles() async {
        let harness = RouterHarness()
        let task = UUID()
        let run = UUID()
        let request = Self.event(task, "approval.requested", run: run, #"{"request":{}}"#)
        await harness.router.handle(request, planeRegistryID: plane)
        await harness.router.handle(Self.activity(task, run: run, "waiting_for_approval"), planeRegistryID: plane)
        #expect(harness.center.delivered.isEmpty)

        await harness.releaseSettle()
        #expect(harness.center.delivered == [
            JetUserNotification(eventID: request.eventID, conversationID: task, planeRegistryID: plane, kind: .approval),
        ])
        #expect(harness.center.delivered.first?.route == JetNotificationRoute(conversationID: task, planeRegistryID: plane))
    }

    @Test
    func aRequestAllowedWhileSettlingNeverNotifies() async {
        let harness = RouterHarness()
        let task = UUID()
        let run = UUID()
        await harness.router.handle(Self.event(task, "approval.requested", run: run), planeRegistryID: plane)
        await TaskStatusStoreTests.until { harness.sleeper.sleeping == 1 }
        // Jet's automatic review allowed it: the Run is working again.
        await harness.router.handle(Self.activity(task, run: run, "working"), planeRegistryID: plane)
        await TaskStatusStoreTests.until { harness.sleeper.sleeping == 0 }
        harness.sleeper.release()
        await harness.router.waitForPendingNotifications()
        #expect(harness.center.delivered.isEmpty)
    }

    @Test
    func oneNotificationPerWaitingEpisode() async {
        let harness = RouterHarness()
        let task = UUID()
        let run = UUID()
        await harness.router.handle(Self.event(task, "approval.requested", run: run), planeRegistryID: plane)
        await harness.releaseSettle()
        #expect(harness.center.delivered.count == 1)

        await harness.router.handle(Self.event(task, "approval.requested", run: run), planeRegistryID: plane)
        await harness.router.handle(Self.activity(task, run: run, "waiting_for_approval"), planeRegistryID: plane)
        #expect(harness.sleeper.sleeping == 0)
        #expect(harness.center.delivered.count == 1)

        await harness.router.handle(Self.activity(task, run: run, "working"), planeRegistryID: plane)
        let next = Self.event(task, "approval.requested", run: run)
        await harness.router.handle(next, planeRegistryID: plane)
        await harness.releaseSettle()
        #expect(harness.center.delivered.map(\.eventID).last == next.eventID)
        #expect(harness.center.delivered.map(\.kind) == [.approval, .approval])
    }

    @Test(arguments: ["waiting_for_auth", "waiting_for_quota"])
    func signInAndUsageLimitsNeedYouToo(_ activity: String) async {
        let harness = RouterHarness()
        let task = UUID()
        await harness.router.handle(Self.activity(task, run: UUID(), activity), planeRegistryID: plane)
        await harness.releaseSettle()
        #expect(harness.center.delivered.map(\.kind) == [.approval])
    }

    // MARK: - Reply Ready

    @Test
    func oneReplyReadyPerReply() async {
        let harness = RouterHarness()
        let task = UUID()
        let run = UUID()
        await harness.router.handle(Self.activity(task, run: run, "working"), planeRegistryID: plane)
        let waiting = Self.activity(task, run: run, "waiting_for_user")
        await harness.router.handle(waiting, planeRegistryID: plane)
        await harness.router.handle(Self.lifecycle(task, run: run, to: "completed"), planeRegistryID: plane)
        #expect(harness.center.delivered == [
            JetUserNotification(eventID: waiting.eventID, conversationID: task, planeRegistryID: plane, kind: .completion),
        ])

        await harness.router.handle(Self.activity(task, run: run, "working"), planeRegistryID: plane)
        await harness.router.handle(Self.activity(task, run: run, "waiting_for_user"), planeRegistryID: plane)
        #expect(harness.center.delivered.map(\.kind) == [.completion, .completion])

        // A Run that finishes without waiting first still gets one.
        let other = UUID()
        await harness.router.handle(Self.lifecycle(other, run: UUID(), to: "completed"), planeRegistryID: plane)
        #expect(harness.center.delivered.map(\.conversationID) == [task, task, other])
    }

    @Test
    func nothingIsShownWhileJetIsInFront() async {
        let harness = RouterHarness()
        harness.app.isActive = true
        let task = UUID()
        let run = UUID()
        await harness.router.handle(Self.activity(task, run: run, "working"), planeRegistryID: plane)
        await harness.router.handle(Self.activity(task, run: run, "waiting_for_user"), planeRegistryID: plane)
        await harness.router.handle(Self.event(task, "approval.requested", run: run), planeRegistryID: plane)
        await harness.releaseSettle()
        #expect(harness.center.delivered.isEmpty)

        // The reply was consumed while Jet was in front.
        harness.app.isActive = false
        await harness.router.handle(Self.lifecycle(task, run: run, to: "completed"), planeRegistryID: plane)
        #expect(harness.center.delivered.isEmpty)
    }

    @Test
    func aKindTurnedOffIsNeverShown() async {
        let harness = RouterHarness(enabled: [.failure])
        let task = UUID()
        let run = UUID()
        await harness.router.handle(Self.activity(task, run: run, "waiting_for_user"), planeRegistryID: plane)
        await harness.router.handle(Self.event(task, "approval.requested", run: run), planeRegistryID: plane)
        await harness.releaseSettle()
        #expect(harness.center.delivered.isEmpty)

        await harness.router.handle(Self.lifecycle(task, run: run, to: "failed"), planeRegistryID: plane)
        #expect(harness.center.delivered.map(\.kind) == [.failure])
    }

    @Test
    func withoutANotificationCenterNothingIsDelivered() async {
        let router = JetNotificationRouter(notifications: nil, preference: { _ in true }, isAppActive: { false })
        await router.handle(Self.lifecycle(UUID(), run: UUID(), to: "failed"), planeRegistryID: plane)
        await router.waitForPendingNotifications()
    }

    // MARK: - Task Stopped

    @Test
    func failuresAndLiveLossesNotifyButARebootsLossDoesNot() async {
        let harness = RouterHarness()
        let failed = UUID()
        await harness.router.handle(Self.lifecycle(failed, run: UUID(), to: "failed"), planeRegistryID: plane)

        let lostLive = UUID()
        let liveRun = UUID()
        await harness.router.handle(Self.activity(lostLive, run: liveRun, "working"), planeRegistryID: plane)
        await harness.router.handle(Self.lifecycle(lostLive, run: liveRun, to: "lost"), planeRegistryID: plane)

        let lostOutput = UUID()
        let outputRun = UUID()
        await harness.router.handle(Self.output(lostOutput, run: outputRun), planeRegistryID: plane)
        await harness.router.handle(Self.lifecycle(lostOutput, run: outputRun, to: "lost"), planeRegistryID: plane)

        let lostAfterReboot = UUID()
        await harness.router.handle(Self.lifecycle(lostAfterReboot, run: UUID(), to: "lost"), planeRegistryID: plane)

        let canceled = UUID()
        let canceledRun = UUID()
        await harness.router.handle(Self.activity(canceled, run: canceledRun, "working"), planeRegistryID: plane)
        await harness.router.handle(Self.lifecycle(canceled, run: canceledRun, to: "canceled"), planeRegistryID: plane)

        #expect(harness.center.delivered.map(\.conversationID) == [failed, lostLive, lostOutput])
        #expect(harness.center.delivered.map(\.kind) == [.failure, .failure, .failure])
    }

    @Test
    func aRepeatedEventNotifiesOnceAndOutputNeverNotifies() async {
        let harness = RouterHarness()
        let task = UUID()
        let failed = Self.lifecycle(task, run: UUID(), to: "failed")
        await harness.router.handle(failed, planeRegistryID: plane)
        await harness.router.handle(failed, planeRegistryID: plane)
        #expect(harness.center.delivered.count == 1)

        let other = UUID()
        await harness.router.handle(Self.output(other, run: UUID(), reply: true), planeRegistryID: plane)
        await harness.router.handle(Self.event(other, "turn.input", run: nil, #"{"text":"private"}"#), planeRegistryID: plane)
        #expect(harness.center.delivered.count == 1)
    }

    @Test
    func aRefusedDeliveryIsReported() async {
        let harness = RouterHarness()
        harness.center.refusesDelivery = true
        let failures = FailureCount()
        harness.router.onDeliveryFailure = { failures.value += 1 }
        await harness.router.handle(Self.lifecycle(UUID(), run: UUID(), to: "failed"), planeRegistryID: plane)
        #expect(failures.value == 1)
    }

    // MARK: - Content

    @Test
    func copyIsExactAndCasual() {
        #expect(JetNotificationKind.allCases == [.approval, .completion, .failure])
        #expect(JetNotificationKind.allCases.map(\.rawValue) == ["approval", "completion", "failure"])
        #expect(JetNotificationKind.allCases.map(\.preferenceKey) == [
            "jet.notifications.approvals", "jet.notifications.completions", "jet.notifications.failures",
        ])
        #expect(JetNotificationKind.allCases.map(\.title) == ["Task Needs You", "Reply Ready", "Task Stopped"])
        #expect(JetNotificationKind.allCases.map(\.preferenceTitle) == [
            "A task needs you", "A reply is ready", "A task stops with an error",
        ])
        #expect(JetNotificationKind.allCases.map(\.isPassive) == [false, true, false])
        for kind in JetNotificationKind.allCases {
            #expect(kind.body == "Click to open the task in Jet.")
            for text in [kind.title, kind.body, kind.preferenceTitle] {
                #expect(JetCopy.foundAvoidWords(in: text).isEmpty, "\(text)")
            }
        }
    }

    @Test
    func aRouteCarriesOnlyTheTwoIDs() {
        let conversationID = UUID()
        let planeRegistryID = UUID()
        let route = JetNotificationRoute(conversationID: conversationID, planeRegistryID: planeRegistryID)
        #expect(route.userInfo == [
            "jet.conversation": conversationID.uuidString.lowercased(),
            "jet.plane": planeRegistryID.uuidString.lowercased(),
        ])
        #expect(route.threadIdentifier == "jet-\(conversationID.uuidString.lowercased())")
        #expect(JetNotificationRoute(userInfo: route.userInfo) == route)

        #expect(JetNotificationRoute(userInfo: [:]) == nil)
        #expect(JetNotificationRoute(userInfo: ["jet.conversation": conversationID.uuidString]) == nil)
        #expect(JetNotificationRoute(userInfo: [
            "jet.conversation": "not a task",
            "jet.plane": planeRegistryID.uuidString,
        ]) == nil)
        #expect(JetNotificationRoute(userInfo: [
            "jet.conversation": conversationID.uuidString,
            "jet.plane": 42,
        ]) == nil)
    }

#if os(macOS)
    nonisolated static let kinds: [JetNotificationKind] = [.approval, .completion, .failure]

    @Test(arguments: kinds)
    func systemContentHoldsNoTaskContent(_ kind: JetNotificationKind) {
        let notification = JetUserNotification(
            eventID: UUID(), conversationID: UUID(), planeRegistryID: UUID(), kind: kind
        )
        let content = SystemJetNotificationCenter.content(for: notification)
        #expect(content.title == kind.title)
        #expect(content.subtitle.isEmpty)
        #expect(content.body == "Click to open the task in Jet.")
        #expect(content.threadIdentifier == notification.route.threadIdentifier)
        #expect(content.userInfo.count == 2)
        #expect(JetNotificationRoute(userInfo: content.userInfo) == notification.route)
        #expect(content.interruptionLevel == (kind == .completion ? .passive : .active))
    }

    @Test
    func theDockBadgeCountsTasksThatNeedYou() {
        #expect(JetDockBadge.label(for: 0) == nil)
        #expect(JetDockBadge.label(for: -1) == nil)
        #expect(JetDockBadge.label(for: 3) == JetCopy.number(3))
    }
#endif

    // MARK: - Offer

    @Test
    func theOfferShowsOnceForATaskStartedHere() {
        let (session, _, defaults) = Self.offerSession()
        #expect(!session.shouldOfferNotifications)

        let startedHere = UUID()
        session.memory.recordAssistant("jet-craft-claude", for: startedHere)
        session.selectedConversationID = UUID()
        #expect(!session.shouldOfferNotifications)
        session.selectedConversationID = startedHere
        #expect(session.shouldOfferNotifications)

        session.notificationAuthorization = .denied
        #expect(!session.shouldOfferNotifications)
        session.notificationAuthorization = .authorized
        #expect(session.shouldOfferNotifications)

        defaults.set(true, forKey: JetNotificationPreferences.failuresKey)
        #expect(!session.shouldOfferNotifications)
        defaults.set(false, forKey: JetNotificationPreferences.failuresKey)

        session.declineNotificationOffer()
        #expect(session.memory.notificationOfferShown)
        #expect(!session.shouldOfferNotifications)
    }

    @Test
    func turningOnWritesPreferencesOnlyWhenPermitted() async {
        let (session, center, defaults) = Self.offerSession()
        center.authorization = .denied
        let denied = await session.turnOnNotifications(defaults: defaults)
        #expect(!denied)
        #expect(session.memory.notificationOfferShown)
        #expect(JetNotificationKind.allCases.allSatisfy { !defaults.bool(forKey: $0.preferenceKey) })

        center.authorization = .authorized
        let granted = await session.turnOnNotifications(defaults: defaults)
        #expect(granted)
        #expect(JetNotificationKind.allCases.allSatisfy { defaults.bool(forKey: $0.preferenceKey) })
    }

    @Test
    func systemSettingsOpensOnJetsNotifications() {
        #expect(DesktopSession.notificationSettingsURL(bundleIdentifier: "me.heeka.jet")?.absoluteString
            == "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=me.heeka.jet")
        #expect(DesktopSession.notificationSettingsURL(bundleIdentifier: nil)?.absoluteString
            == "x-apple.systempreferences:com.apple.Notifications-Settings.extension")
    }

    // MARK: - Helpers

    static func event(_ task: UUID, _ kind: String, run: UUID?, _ payload: String = "{}") -> JetEvent {
        TaskStatusStoreTests.event(task, 1, kind, run: run, payload)
    }

    static func activity(_ task: UUID, run: UUID, _ activity: String) -> JetEvent {
        event(task, "run.activity_changed", run: run, #"{"activity":"\#(activity)"}"#)
    }

    static func lifecycle(_ task: UUID, run: UUID, to: String) -> JetEvent {
        event(task, "run.lifecycle_changed", run: run, #"{"to":"\#(to)"}"#)
    }

    static func output(_ task: UUID, run: UUID, reply: Bool = false) -> JetEvent {
        TaskStatusStoreTests.output(task, 1, run: run, reply: reply, tool: reply ? nil : "Read")
    }

    static func offerSession() -> (DesktopSession, FakeNotificationCenter, UserDefaults) {
        let defaults = ClientMemoryTests.isolatedDefaults().0
        let center = FakeNotificationCenter()
        let session = DesktopSession(
            notifications: center,
            notificationPreference: { defaults.bool(forKey: $0.preferenceKey) },
            memory: ClientMemoryTests.isolatedMemory()
        )
        return (session, center, defaults)
    }
}

/// A router with a fake notification center, a hand-released settle delay and
/// a switch for whether Jet is in front.
@MainActor
final class RouterHarness {
    let center: FakeNotificationCenter
    let sleeper: ManualSleeper
    let app: AppActivity
    let router: JetNotificationRouter

    init(enabled: Set<JetNotificationKind> = Set(JetNotificationKind.allCases)) {
        let center = FakeNotificationCenter()
        let sleeper = ManualSleeper()
        let app = AppActivity()
        self.center = center
        self.sleeper = sleeper
        self.app = app
        router = JetNotificationRouter(
            notifications: center,
            preference: { enabled.contains($0) },
            isAppActive: { app.isActive },
            settleDelay: .seconds(2),
            sleep: { _ in try await sleeper.sleep() }
        )
    }

    /// Lets the held "Task Needs You" go and waits for its delivery.
    func releaseSettle() async {
        await TaskStatusStoreTests.until { sleeper.sleeping > 0 }
        sleeper.release()
        await router.waitForPendingNotifications()
    }
}

@MainActor
final class AppActivity {
    var isActive = false
}

@MainActor
final class FailureCount {
    var value = 0
}

@MainActor
final class FakeNotificationCenter: JetNotificationDelivering {
    var authorization = JetNotificationAuthorization.authorized
    var refusesDelivery = false
    private(set) var delivered: [JetUserNotification] = []

    func authorizationStatus() async -> JetNotificationAuthorization { authorization }

    func requestAuthorization() async throws -> JetNotificationAuthorization { authorization }

    func deliver(_ notification: JetUserNotification) async throws {
        if refusesDelivery { throw SeedFailure() }
        delivered.append(notification)
    }
}

/// A sleep that ends only when the test releases it, or when its task is cancelled.
@MainActor
final class ManualSleeper {
    private var waiters: [UUID: CheckedContinuation<Void, any Error>] = [:]

    var sleeping: Int { waiters.count }

    func sleep() async throws {
        let id = UUID()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                waiters[id] = continuation
            }
        } onCancel: {
            Task { @MainActor in self.cancel(id) }
        }
    }

    func release() {
        let released = waiters
        waiters = [:]
        for continuation in released.values { continuation.resume() }
    }

    private func cancel(_ id: UUID) {
        waiters.removeValue(forKey: id)?.resume(throwing: CancellationError())
    }
}
