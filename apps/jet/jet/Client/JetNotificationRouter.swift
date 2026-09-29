import Foundation

#if os(macOS)
import AppKit
#endif

/// Decides which Plane Events become desktop notifications and delivers them
/// (design §6.13). Notifications never carry task content (ASVS 14.2.3): only
/// the Event, task and computer IDs reach the notification center.
///
/// - "Task Needs You" is sent once per waiting episode, after a short settle so
///   that a request Jet's automatic review allows at once never notifies.
/// - "Reply Ready" is sent at most once per reply.
/// - "Task Stopped" is sent for `failed`, and for `lost` only when this app saw
///   the Run live; a reboot's `lost` stays silent (§8).
/// - Nothing is sent while Jet is in front.
@MainActor
final class JetNotificationRouter {
    static let rememberedRuns = 256
    static let rememberedEvents = 512

    private let notifications: (any JetNotificationDelivering)?
    private let preference: JetNotificationPreference
    private let isAppActive: @MainActor () -> Bool
    private let settleDelay: Duration
    private let sleep: @Sendable (Duration) async throws -> Void
    /// Called when the system refused to show a notification.
    var onDeliveryFailure: (@MainActor () -> Void)?

    /// Runs this app saw live, so their `lost` isn't a reboot's.
    private var liveRuns = BoundedSet<UUID>(capacity: rememberedRuns)
    /// Runs whose current reply already raised "Reply Ready" (or was consumed silently).
    private var finishedReplies = BoundedSet<UUID>(capacity: rememberedRuns)
    /// Tasks waiting for the person right now.
    private var awaitingPerson = BoundedSet<UUID>(capacity: rememberedRuns)
    /// Tasks whose current waiting episode already raised "Task Needs You".
    private var notifiedEpisodes = BoundedSet<UUID>(capacity: rememberedRuns)
    private var postedEventIDs = BoundedSet<UUID>(capacity: rememberedEvents)
    private var pendingNeedsYou: [UUID: PendingNotification] = [:]

    private struct PendingNotification {
        let token: UUID
        let task: Task<Void, Never>
    }

    init(
        notifications: (any JetNotificationDelivering)?,
        preference: @escaping JetNotificationPreference,
        isAppActive: @escaping @MainActor () -> Bool = JetNotificationRouter.jetIsInForeground,
        settleDelay: Duration = .seconds(2),
        sleep: @escaping @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
    ) {
        self.notifications = notifications
        self.preference = preference
        self.isAppActive = isAppActive
        self.settleDelay = settleDelay
        self.sleep = sleep
    }

    /// Whether the person is looking at Jet: it is the active app and one of its
    /// windows is on screen.
    static func jetIsInForeground() -> Bool {
#if os(macOS)
        guard let app = NSApp, app.isActive else { return false }
        return app.windows.contains { $0.isVisible && !$0.isMiniaturized && $0.canBecomeMain }
#else
        return false
#endif
    }

    /// Follows one Event. It never waits for the settle delay; a held "Task Needs
    /// You" is delivered later by its own task.
    func handle(_ event: JetEvent, planeRegistryID: UUID) async {
        guard let conversationID = event.conversationID else { return }
        let runKey = event.runID ?? conversationID
        // Output only shows the Run is live; skip reading its blocks here.
        if event.kind == "run.output" {
            liveRuns.insert(runKey)
            return
        }
        guard let signal = event.statusSignal else { return }
        let wasLive = liveRuns.contains(runKey)
        track(signal, conversationID: conversationID, runKey: runKey)

        switch signal.notificationKind {
        case .approval:
            guard !notifiedEpisodes.contains(conversationID),
                  pendingNeedsYou[conversationID] == nil
            else { return }
            holdNeedsYou(event, planeRegistryID: planeRegistryID, conversationID: conversationID)
        case .completion:
            // The reply is consumed even when nothing is shown.
            guard finishedReplies.insert(runKey) else { return }
            await post(event, planeRegistryID: planeRegistryID, kind: .completion)
        case .failure:
            if case .lifecycle(_, .lost) = signal, !wasLive { return }
            await post(event, planeRegistryID: planeRegistryID, kind: .failure)
        case nil:
            return
        }
    }

    /// Waits until every held "Task Needs You" is delivered or dropped. For tests.
    func waitForPendingNotifications() async {
        while let (conversationID, pending) = pendingNeedsYou.first {
            await pending.task.value
            if pendingNeedsYou[conversationID]?.token == pending.token {
                pendingNeedsYou.removeValue(forKey: conversationID)
            }
        }
    }

    // MARK: - Tracking

    /// Keeps track of live Runs, replies and waiting episodes, even while
    /// notifications are off or Jet is in front.
    private func track(_ signal: JetEventStatusSignal, conversationID: UUID, runKey: UUID) {
        switch signal {
        case .runCreated, .output:
            liveRuns.insert(runKey)
        case let .lifecycle(_, to):
            if to.isLive {
                liveRuns.insert(runKey)
            } else {
                liveRuns.remove(runKey)
                endEpisode(conversationID)
            }
        case let .activity(activity):
            if activity != nil { liveRuns.insert(runKey) }
            switch activity {
            case .working:
                // A new reply is under way.
                finishedReplies.remove(runKey)
                endEpisode(conversationID)
            case .waitingForApproval, .waitingForAuth, .waitingForQuota:
                awaitingPerson.insert(conversationID)
            case .waitingForUser, nil:
                endEpisode(conversationID)
            case .reconnecting:
                break
            }
        case .approvalRequested:
            awaitingPerson.insert(conversationID)
        case .trashed:
            endEpisode(conversationID)
        case .input, .changesRecorded:
            break
        }
    }

    /// The task stopped waiting for the person; a held notification is dropped.
    private func endEpisode(_ conversationID: UUID) {
        awaitingPerson.remove(conversationID)
        notifiedEpisodes.remove(conversationID)
        pendingNeedsYou.removeValue(forKey: conversationID)?.task.cancel()
    }

    // MARK: - Delivery

    private func holdNeedsYou(_ event: JetEvent, planeRegistryID: UUID, conversationID: UUID) {
        let token = UUID()
        let sleep = sleep
        let settleDelay = settleDelay
        let task = Task { [weak self] in
            try? await sleep(settleDelay)
            guard let self, !Task.isCancelled,
                  self.pendingNeedsYou[conversationID]?.token == token
            else { return }
            self.pendingNeedsYou.removeValue(forKey: conversationID)
            guard self.awaitingPerson.contains(conversationID) else { return }
            self.notifiedEpisodes.insert(conversationID)
            await self.post(event, planeRegistryID: planeRegistryID, kind: .approval)
        }
        pendingNeedsYou[conversationID] = PendingNotification(token: token, task: task)
    }

    private func post(_ event: JetEvent, planeRegistryID: UUID, kind: JetNotificationKind) async {
        guard let notifications,
              let conversationID = event.conversationID,
              preference(kind),
              !isAppActive(),
              postedEventIDs.insert(event.eventID)
        else { return }
        do {
            try await notifications.deliver(
                JetUserNotification(
                    eventID: event.eventID,
                    conversationID: conversationID,
                    planeRegistryID: planeRegistryID,
                    kind: kind
                )
            )
        } catch {
            onDeliveryFailure?()
        }
    }
}

/// A set that remembers at most `capacity` members and forgets the oldest first.
private struct BoundedSet<Element: Hashable> {
    let capacity: Int
    private var members: Set<Element> = []
    private var order: [Element] = []

    init(capacity: Int) {
        self.capacity = capacity
    }

    func contains(_ element: Element) -> Bool {
        members.contains(element)
    }

    /// Returns false when the element was already a member.
    @discardableResult
    mutating func insert(_ element: Element) -> Bool {
        guard members.insert(element).inserted else { return false }
        order.append(element)
        if order.count > capacity {
            members.remove(order.removeFirst())
        }
        return true
    }

    mutating func remove(_ element: Element) {
        guard members.remove(element) != nil else { return }
        order.removeAll { $0 == element }
    }
}
