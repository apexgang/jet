import Foundation
import Observation
import Synchronization
import Testing
@testable import jet

@MainActor
struct TaskStatusStoreTests {
    private let planeA = UUID()
    private let planeB = UUID()

    // MARK: - Events

    @Test
    func workingWithAToolNameStaysWorking() {
        let store = Self.store()
        let task = UUID()
        let run = UUID()
        store.record(Self.activity(task, 10, run: run, "working"), planeRegistryID: planeA)
        store.record(Self.output(task, 11, run: run, tool: "Edit"), planeRegistryID: planeA)

        #expect(store.facts[task] == TaskStatusFacts(
            lifecycle: .active, activity: .working, runID: run, hasRuns: true, lastSequence: 10
        ))
        #expect(TaskStatus.derive(facts: store.facts[task], isOffline: false, phase: nil) == .working(nil))
    }

    @Test
    func waitingForPermissionNeedsYouUntilTheRunMovesOn() {
        let store = Self.store()
        let first = UUID()
        let run = UUID()
        store.record(Self.activity(first, 10, run: run, "waiting_for_approval"), planeRegistryID: planeA)
        #expect(store.needsYouConversationIDs == [first])

        store.record(Self.activity(first, 11, run: run, "working"), planeRegistryID: planeA)
        #expect(store.needsYouConversationIDs.isEmpty)

        store.record(Self.activity(first, 12, run: run, "waiting_for_approval"), planeRegistryID: planeA)
        store.record(Self.lifecycle(first, 13, run: run, to: "lost"), planeRegistryID: planeA)
        #expect(store.facts[first]?.activity == nil)
        #expect(TaskStatus.derive(facts: store.facts[first], isOffline: false, phase: nil) == .stopped)

        let second = UUID()
        let otherRun = UUID()
        store.record(Self.activity(second, 20, run: otherRun, "waiting_for_auth"), planeRegistryID: planeA)
        let third = UUID()
        store.record(Self.activity(third, 21, run: UUID(), "waiting_for_quota"), planeRegistryID: planeA)
        // Most recently updated first.
        #expect(store.needsYouConversationIDs == [third, second])

        store.record(Self.lifecycle(second, 22, run: otherRun, to: "canceled"), planeRegistryID: planeA)
        #expect(TaskStatus.derive(facts: store.facts[second], isOffline: false, phase: nil) == .stopped)
        #expect(store.needsYouConversationIDs == [third])
    }

    @Test
    func duplicateAndOlderEventsAreIgnored() {
        let store = Self.store()
        let task = UUID()
        let run = UUID()
        store.record(Self.activity(task, 10, run: run, "working"), planeRegistryID: planeA)
        store.record(Self.activity(task, 10, run: run, "waiting_for_user"), planeRegistryID: planeA)
        store.record(Self.activity(task, 9, run: run, "waiting_for_approval"), planeRegistryID: planeA)
        #expect(store.facts[task]?.activity == .working)

        store.record(Self.activity(task, 11, run: run, "waiting_for_user"), planeRegistryID: planeA)
        #expect(store.facts[task]?.activity == .waitingForUser)
    }

    @Test
    func olderSnapshotsAreFencedAndNewerOnesKeepEventFacts() {
        let store = Self.store()
        let conversation = Self.conversation()
        let run = Self.run(conversation, .active)
        store.record(Self.activity(conversation.id, 20, run: run.id, "working"), planeRegistryID: planeA)
        store.record(Self.output(conversation.id, 21, run: run.id, reply: true), planeRegistryID: planeA)
        store.record(Self.event(conversation.id, 22, "change.checkpoint_recorded", run: run.id), planeRegistryID: planeA)
        store.record(Self.event(conversation.id, 23, "turn.input", run: run.id, #"{"text":"x"}"#), planeRegistryID: planeA)
        let before = store.facts[conversation.id]

        let stale = store.record(
            snapshot: Self.snapshot(conversation, [run], cursor: 15),
            execution: Self.execution(run, .waitingForApproval, cursor: 15),
            cursor: 15
        )
        #expect(!stale)
        #expect(store.facts[conversation.id] == before)

        let fresh = store.record(
            snapshot: Self.snapshot(conversation, [run], cursor: 30),
            execution: Self.execution(run, .waitingForUser, cursor: 30),
            cursor: 30
        )
        #expect(fresh)
        #expect(store.facts[conversation.id] == TaskStatusFacts(
            lifecycle: .active,
            activity: .waitingForUser,
            runID: run.id,
            hasRuns: true,
            hasRecordedChanges: true,
            lastSequence: 30,
            lastReplySequence: 21,
            lastInputSequence: 23
        ))
    }

    @Test
    func aNewRunIsAdoptedAndAnOlderRunEndingIsIgnored() {
        let store = Self.store()
        let task = UUID()
        let oldRun = UUID()
        let newRun = UUID()
        store.record(Self.activity(task, 10, run: oldRun, "working"), planeRegistryID: planeA)
        store.record(Self.event(task, 11, "run.created", run: newRun), planeRegistryID: planeA)
        #expect(store.facts[task]?.runID == newRun)
        #expect(store.facts[task]?.lifecycle == .created)

        store.record(Self.lifecycle(task, 12, run: oldRun, to: "failed"), planeRegistryID: planeA)
        #expect(store.facts[task]?.runID == newRun)
        #expect(TaskStatus.derive(facts: store.facts[task], isOffline: false, phase: nil) == .starting)

        store.record(Self.lifecycle(task, 13, run: newRun, to: "active"), planeRegistryID: planeA)
        store.record(Self.activity(task, 14, run: newRun, "waiting_for_user"), planeRegistryID: planeA)
        #expect(TaskStatus.derive(facts: store.facts[task], isOffline: false, phase: nil) == .waitingForReply)

        // A later Run reporting activity takes over too.
        let laterRun = UUID()
        store.record(Self.activity(task, 15, run: laterRun, "waiting_for_approval"), planeRegistryID: planeA)
        #expect(store.facts[task]?.runID == laterRun)
        #expect(store.facts[task]?.lifecycle == .active)
        #expect(store.needsYouConversationIDs == [task])
    }

    @Test
    func activityCreatesProvisionalFactsButOutputDoesNot() {
        let store = Self.store()
        let task = UUID()
        let run = UUID()
        store.record(Self.event(task, 3, "change.checkpoint_recorded", run: run), planeRegistryID: planeA)
        store.record(Self.event(task, 4, "turn.input", run: run, #"{"text":"x"}"#), planeRegistryID: planeA)
        store.record(Self.output(task, 5, run: run, reply: true), planeRegistryID: planeA)
        store.record(Self.activity(task, 6, run: run, nil), planeRegistryID: planeA)
        store.record(Self.event(task, 7, "approval.requested", run: run, #"{"request":{}}"#), planeRegistryID: planeA)
        #expect(store.facts[task] == nil)

        store.record(Self.activity(task, 8, run: run, "waiting_for_user"), planeRegistryID: planeA)
        #expect(store.facts[task] == TaskStatusFacts(
            lifecycle: .active,
            activity: .waitingForUser,
            runID: run,
            hasRuns: true,
            hasRecordedChanges: true,
            lastSequence: 8,
            lastReplySequence: 5,
            lastInputSequence: 4
        ))

        let finished = UUID()
        store.record(Self.lifecycle(finished, 9, run: UUID(), to: "completed"), planeRegistryID: planeA)
        #expect(TaskStatus.derive(facts: store.facts[finished], isOffline: false, phase: nil) == .finished)

        store.record(Self.event(task, 10, "conversation.trashed", run: nil), planeRegistryID: planeA)
        #expect(store.facts[task] == nil)
    }

    // MARK: - Unread

    @Test
    func repliesAreUnreadUntilTheTaskIsOpened() {
        let memory = ClientMemoryTests.isolatedMemory()
        let store = Self.store(memory: memory)
        let task = UUID()
        let run = UUID()
        store.record(Self.activity(task, 10, run: run, "working"), planeRegistryID: planeA)
        #expect(!store.isUnread(task))

        store.record(Self.output(task, 11, run: run, reply: true), planeRegistryID: planeA)
        #expect(store.isUnread(task))

        store.noteSelected(task)
        #expect(store.selectedConversationID == task)
        #expect(!store.isUnread(task))
        #expect(memory.seenSequence(task) == 11)

        store.record(Self.output(task, 12, run: run, reply: true), planeRegistryID: planeA)
        #expect(!store.isUnread(task))

        // Leaving the open task: what was on screen counts as seen.
        store.noteSelected(nil)
        #expect(store.selectedConversationID == nil)
        #expect(!store.isUnread(task))
        #expect(memory.seenSequence(task) == 12)

        store.record(Self.output(task, 13, run: run, reply: true), planeRegistryID: planeA)
        #expect(store.isUnread(task))
    }

    @Test
    func openingAnotherTaskDoesNotMarkAnEarlierOneSeen() {
        let memory = ClientMemoryTests.isolatedMemory()
        let store = Self.store(memory: memory)
        let first = UUID()
        let second = UUID()
        store.record(Self.activity(first, 10, run: UUID(), "working"), planeRegistryID: planeA)
        store.noteSelected(first)
        // The person left `first` for New Task; the session doesn't report that here.
        store.record(Self.output(first, 11, run: nil, reply: true), planeRegistryID: planeA)
        store.noteSelected(second)
        #expect(memory.seenSequence(first) == nil)
        #expect(store.isUnread(first))
    }

    @Test
    func streamingRepliesRedrawOnlyWhenTheUnreadStateChanges() {
        let store = Self.store()
        let task = UUID()
        let run = UUID()
        store.record(Self.activity(task, 10, run: run, "working"), planeRegistryID: planeA)

        let firstChunk = Self.observeFacts(of: store) {
            store.record(Self.output(task, 11, run: run, reply: true), planeRegistryID: planeA)
        }
        #expect(firstChunk)
        #expect(store.facts[task]?.lastReplySequence == 11)

        let secondChunk = Self.observeFacts(of: store) {
            store.record(Self.output(task, 12, run: run, reply: true), planeRegistryID: planeA)
        }
        #expect(!secondChunk)
        #expect(store.latestReplySequence(task) == 12)
        #expect(store.isUnread(task))

        let toolChunk = Self.observeFacts(of: store) {
            store.record(Self.output(task, 13, run: run, tool: "Bash"), planeRegistryID: planeA)
        }
        #expect(!toolChunk)

        let statusChange = Self.observeFacts(of: store) {
            store.record(Self.activity(task, 14, run: run, "waiting_for_user"), planeRegistryID: planeA)
        }
        #expect(statusChange)
        #expect(store.facts[task]?.lastReplySequence == 12)
    }

    // MARK: - Lazy seeding

    @Test
    func seedsRunAtMostFourAtATimePerComputer() async {
        let fetch = FakeFetch()
        let store = Self.store(fetch: fetch)
        let onA = (0..<6).map { _ in UUID() }
        let onB = (0..<2).map { _ in UUID() }
        for task in onA { store.noteVisible(task, planeRegistryID: planeA) }
        for task in onB { store.noteVisible(task, planeRegistryID: planeB) }

        await Self.until { fetch.calls.count == 6 }
        #expect(fetch.calls.count == 6)
        #expect(store.seedsInFlight(on: planeA) == 4)
        #expect(store.seedsInFlight(on: planeB) == 2)
        #expect(store.queuedSeeds(on: planeA) == Array(onA[4...]))
        #expect(Set(fetch.calls.map(\.conversationID)) == Set(onA.prefix(4) + onB))

        // Already queued or running: no second request.
        store.noteVisible(onA[0], planeRegistryID: planeA)
        store.noteVisible(onA[5], planeRegistryID: planeA)
        #expect(store.queuedSeeds(on: planeA) == Array(onA[4...]))

        fetch.succeed(onA[0], with: Self.seed(onA[0], cursor: 10, activity: .working))
        await Self.until { fetch.calls.count == 7 }
        #expect(fetch.calls.last?.conversationID == onA[4])
        #expect(fetch.calls.last?.planeRegistryID == planeA)
        #expect(store.seedsInFlight(on: planeA) == 4)
        #expect(store.facts[onA[0]]?.activity == .working)

        // A task with facts isn't seeded again.
        store.noteVisible(onA[0], planeRegistryID: planeA)
        #expect(store.queuedSeeds(on: planeA) == [onA[5]])
    }

    @Test
    func aRowHiddenBeforeItsSeedStartsIsNeverFetched() async {
        let fetch = FakeFetch()
        let store = Self.store(fetch: fetch)
        let tasks = (0..<5).map { _ in UUID() }
        for task in tasks { store.noteVisible(task, planeRegistryID: planeA) }
        await Self.until { fetch.calls.count == 4 }
        #expect(store.queuedSeeds(on: planeA) == [tasks[4]])

        store.noteHidden(tasks[4])
        #expect(store.queuedSeeds(on: planeA).isEmpty)
        // A running seed completes even when its row is hidden.
        store.noteHidden(tasks[0])
        fetch.succeed(tasks[0], with: Self.seed(tasks[0], cursor: 10, activity: .waitingForUser))
        await Self.until { store.facts[tasks[0]] != nil }
        await Self.settle()
        #expect(fetch.calls.count == 4)
        #expect(store.facts[tasks[0]]?.activity == .waitingForUser)
        #expect(store.seedsInFlight(on: planeA) == 3)
    }

    @Test
    func anEventNewerThanARunningSeedWins() async {
        let fetch = FakeFetch()
        let store = Self.store(fetch: fetch)
        let task = UUID()
        store.noteVisible(task, planeRegistryID: planeA)
        await Self.until { fetch.calls.count == 1 }

        store.record(
            Self.activity(task, 200, run: Self.seedRun(task).id, "waiting_for_approval"),
            planeRegistryID: planeA
        )
        fetch.succeed(task, with: Self.seed(task, cursor: 150, activity: .working))
        await Self.until { store.seedsInFlight(on: planeA) == 0 }
        #expect(store.facts[task]?.activity == .waitingForApproval)
        #expect(store.facts[task]?.lastSequence == 200)

        // Output alone doesn't advance the fence, so the seed still applies and keeps the reply.
        let other = UUID()
        store.noteVisible(other, planeRegistryID: planeA)
        await Self.until { fetch.calls.count == 2 }
        store.record(Self.output(other, 180, run: Self.seedRun(other).id, reply: true), planeRegistryID: planeA)
        fetch.succeed(other, with: Self.seed(other, cursor: 150, activity: .waitingForUser))
        await Self.until { store.facts[other] != nil }
        #expect(store.facts[other]?.activity == .waitingForUser)
        #expect(store.facts[other]?.lastReplySequence == 180)
        #expect(store.facts[other]?.lastSequence == 150)
    }

    @Test
    func removingDropsARunningSeedAndResetReseedsVisibleRows() async {
        let fetch = FakeFetch()
        let store = Self.store(fetch: fetch)
        let removed = UUID()
        store.noteVisible(removed, planeRegistryID: planeA)
        await Self.until { fetch.calls.count == 1 }
        store.remove(removed)
        #expect(store.seedsInFlight(on: planeA) == 0)
        fetch.succeed(removed, with: Self.seed(removed, cursor: 10, activity: .working))
        await Self.settle()
        #expect(store.facts[removed] == nil)

        let visible = UUID()
        let offscreen = UUID()
        store.noteVisible(visible, planeRegistryID: planeA)
        store.noteVisible(offscreen, planeRegistryID: planeA)
        await Self.until { fetch.calls.count == 3 }
        fetch.succeed(visible, with: Self.seed(visible, cursor: 20, activity: .working))
        fetch.succeed(offscreen, with: Self.seed(offscreen, cursor: 20, activity: .working))
        await Self.until { store.facts[visible] != nil && store.facts[offscreen] != nil }
        store.noteHidden(offscreen)

        store.reset(conversationIDs: [visible, offscreen])
        #expect(store.facts[visible] == nil)
        #expect(store.facts[offscreen] == nil)
        await Self.until { fetch.calls.count == 4 }
        #expect(fetch.calls.count == 4)
        #expect(fetch.calls.last?.conversationID == visible)

        // The new seed applies even with an older cursor: reset forgot the fence.
        fetch.succeed(visible, with: Self.seed(visible, cursor: 5, activity: .waitingForUser))
        await Self.until { store.facts[visible] != nil }
        #expect(store.facts[visible]?.activity == .waitingForUser)
    }

    @Test
    func failedSeedsWaitBeforeRetrying() async {
        let clock = StatusManualClock()
        let fetch = FakeFetch()
        let store = Self.store(fetch: fetch, clock: clock)
        let task = UUID()
        store.noteVisible(task, planeRegistryID: planeA)
        await Self.until { fetch.calls.count == 1 }
        fetch.fail(task, SeedFailure())
        await Self.until { store.seedsInFlight(on: planeA) == 0 }

        store.noteHidden(task)
        store.noteVisible(task, planeRegistryID: planeA)
        clock.advance(by: .seconds(29))
        store.noteVisible(task, planeRegistryID: planeA)
        await Self.settle()
        #expect(fetch.calls.count == 1)

        clock.advance(by: .seconds(1))
        store.noteVisible(task, planeRegistryID: planeA)
        await Self.until { fetch.calls.count == 2 }
        #expect(fetch.calls.count == 2)
        fetch.fail(task, SeedFailure())
        await Self.until { store.seedsInFlight(on: planeA) == 0 }

        // An Event from the computer shows it is reachable again.
        let neighbour = UUID()
        clock.advance(by: .seconds(4))
        store.record(Self.activity(neighbour, 50, run: UUID(), "working"), planeRegistryID: planeA)
        await Self.settle()
        #expect(fetch.calls.count == 2)

        clock.advance(by: .seconds(1))
        store.record(Self.activity(neighbour, 51, run: UUID(), "working"), planeRegistryID: planeA)
        await Self.until { fetch.calls.count == 3 }
        #expect(fetch.calls.last?.conversationID == task)

        // Events from another computer say nothing about this one.
        fetch.fail(task, SeedFailure())
        await Self.until { store.seedsInFlight(on: planeA) == 0 }
        clock.advance(by: .seconds(10))
        store.record(Self.activity(UUID(), 60, run: UUID(), "working"), planeRegistryID: planeB)
        await Self.settle()
        #expect(fetch.calls.count == 3)
    }

    @Test
    func aCancelledSeedCanStartAgainAtOnce() async {
        let fetch = FakeFetch()
        let store = Self.store(fetch: fetch)
        let task = UUID()
        store.noteVisible(task, planeRegistryID: planeA)
        await Self.until { fetch.calls.count == 1 }
        fetch.fail(task, CancellationError())
        await Self.until { store.seedsInFlight(on: planeA) == 0 }
        #expect(store.facts[task] == nil)

        store.noteVisible(task, planeRegistryID: planeA)
        await Self.until { fetch.calls.count == 2 }
        #expect(fetch.calls.count == 2)
    }

    @Test
    func gitDeliveriesAloneDontStopASeed() async {
        let fetch = FakeFetch()
        let store = Self.store(fetch: fetch)
        let task = UUID()
        store.recordGitDeliveries([SessionSelectionTests.unknownDelivery(for: task)], conversationID: task)
        #expect(store.needsYouConversationIDs == [task])

        store.noteVisible(task, planeRegistryID: planeA)
        await Self.until { fetch.calls.count == 1 }
        fetch.succeed(task, with: Self.seed(task, cursor: 10, lifecycle: .completed, activity: nil))
        await Self.until { store.facts[task]?.lifecycle != nil }
        #expect(store.facts[task]?.lifecycle == .completed)
        #expect(store.facts[task]?.gitUnconfirmed == true)
        #expect(store.needsYouConversationIDs == [task])
    }

    // MARK: - Helpers

    static func store(
        memory: ClientMemory? = nil,
        fetch: FakeFetch? = nil,
        clock: StatusManualClock? = nil
    ) -> TaskStatusStore {
        let memory = memory ?? ClientMemoryTests.isolatedMemory()
        let store: TaskStatusStore
        if let clock {
            store = TaskStatusStore(memory: memory, now: { clock.now })
        } else {
            store = TaskStatusStore(memory: memory)
        }
        if let fetch {
            store.configure { conversationID, planeRegistryID in
                try await fetch.fetch(conversationID, planeRegistryID)
            }
        }
        return store
    }

    static func event(_ task: UUID, _ sequence: UInt64, _ kind: String, run: UUID?, _ payload: String = "{}") -> JetEvent {
        JetEvent(
            sequence: sequence,
            eventID: UUID(),
            actor: JetRawJSON(source: #"{"type":"harness"}"#),
            origin: nil,
            recordedAtUnixMilliseconds: 1,
            conversationID: task,
            runID: run,
            kind: kind,
            payloadVersion: 1,
            payload: JetRawJSON(source: payload)
        )
    }

    static func activity(_ task: UUID, _ sequence: UInt64, run: UUID?, _ activity: String?) -> JetEvent {
        let payload = activity.map { #"{"activity":"\#($0)"}"# } ?? #"{"activity":null}"#
        return event(task, sequence, "run.activity_changed", run: run, payload)
    }

    static func lifecycle(_ task: UUID, _ sequence: UInt64, run: UUID?, to: String) -> JetEvent {
        event(task, sequence, "run.lifecycle_changed", run: run, #"{"to":"\#(to)"}"#)
    }

    static func output(_ task: UUID, _ sequence: UInt64, run: UUID?, reply: Bool = false, tool: String? = nil) -> JetEvent {
        var blocks: [(kind: String, text: String)] = []
        if let tool { blocks.append(("text", tool)) }
        if reply { blocks.append(("markdown", "Here is what changed.")) }
        return event(task, sequence, "run.output", run: run, StatusSignalCase.output(blocks))
    }

    static func conversation(_ id: UUID = UUID()) -> JetConversationSummary {
        JetConversationSummary(id: id, revision: 1, title: "Task", createdAtUnixMilliseconds: 1, projectID: nil)
    }

    static func run(_ conversation: JetConversationSummary, _ lifecycle: JetRunLifecycle, id: UUID = UUID()) -> JetRunSummary {
        JetRunSummary(
            id: id, conversationID: conversation.id, revision: 1, lifecycle: lifecycle,
            title: conversation.title, createdAtUnixMilliseconds: 1,
            endedAtUnixMilliseconds: lifecycle.isLive ? nil : 2
        )
    }

    static func snapshot(_ conversation: JetConversationSummary, _ runs: [JetRunSummary], cursor: UInt64) -> JetConversationSnapshot {
        JetConversationSnapshot(cursor: cursor, conversation: conversation, workspaceID: nil, workspaceRoot: nil, runs: runs)
    }

    static func execution(_ run: JetRunSummary, _ activity: JetRunActivity?, cursor: UInt64) -> JetRunExecution {
        JetRunExecution(cursor: cursor, run: run, activity: activity, needsAttention: false, termination: nil)
    }

    /// Each seeded task's one Run has a stable ID derived from the task.
    static func seedRun(_ task: UUID, lifecycle: JetRunLifecycle = .active) -> JetRunSummary {
        var bytes = task.uuid
        bytes.0 ^= 0xFF
        return run(conversation(task), lifecycle, id: UUID(uuid: bytes))
    }

    static func seed(
        _ task: UUID,
        cursor: UInt64,
        lifecycle: JetRunLifecycle = .active,
        activity: JetRunActivity?
    ) -> FakeFetch.Result {
        let run = seedRun(task, lifecycle: lifecycle)
        return (
            cursor: cursor,
            snapshot: snapshot(conversation(task), [run], cursor: cursor),
            execution: execution(run, activity, cursor: cursor)
        )
    }

    /// Whether `change` notified an observer of `facts`.
    static func observeFacts(of store: TaskStatusStore, during change: () -> Void) -> Bool {
        let fired = ObservationFlag()
        withObservationTracking {
            _ = store.facts
        } onChange: {
            fired.set()
        }
        change()
        return fired.value
    }

    /// Lets queued main-actor work run until `condition` holds, within a bound.
    static func until(_ condition: () -> Bool) async {
        for _ in 0..<500 {
            if condition() { return }
            await Task.yield()
        }
    }

    /// Lets queued main-actor work run.
    static func settle() async {
        for _ in 0..<50 { await Task.yield() }
    }
}

/// A status fetch the test answers by hand.
@MainActor
final class FakeFetch {
    typealias Result = (cursor: UInt64, snapshot: JetConversationSnapshot, execution: JetRunExecution?)

    struct Call: Equatable {
        let conversationID: UUID
        let planeRegistryID: UUID
    }

    private(set) var calls: [Call] = []
    private var waiting: [UUID: CheckedContinuation<Result, any Error>] = [:]

    func fetch(_ conversationID: UUID, _ planeRegistryID: UUID) async throws -> Result {
        calls.append(Call(conversationID: conversationID, planeRegistryID: planeRegistryID))
        return try await withCheckedThrowingContinuation { continuation in
            waiting[conversationID] = continuation
        }
    }

    func succeed(_ conversationID: UUID, with result: Result) {
        waiting.removeValue(forKey: conversationID)?.resume(returning: result)
    }

    func fail(_ conversationID: UUID, _ error: any Error) {
        waiting.removeValue(forKey: conversationID)?.resume(throwing: error)
    }
}

@MainActor
final class StatusManualClock {
    private(set) var now = ContinuousClock.now

    func advance(by duration: Duration) {
        now = now.advanced(by: duration)
    }
}

nonisolated final class ObservationFlag: Sendable {
    private let fired = Mutex(false)

    func set() { fired.withLock { $0 = true } }
    var value: Bool { fired.withLock { $0 } }
}

nonisolated struct SeedFailure: Error {}
