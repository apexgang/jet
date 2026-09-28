import Foundation
import Testing
@testable import jet

@MainActor
struct TaskStatusTests {
    // MARK: - The §8 table

    @Test
    func offlineComesFirst() {
        let needsPermission = facts(.active, .waitingForApproval)
        #expect(TaskStatus.derive(facts: needsPermission, isOffline: true, phase: nil) == .offline)
        #expect(TaskStatus.derive(facts: nil, isOffline: true, phase: nil) == .offline)
        #expect(!TaskStatus.offline.needsYou)
        #expect(!TaskStatus.offline.showsAlertGlyph)
    }

    @Test
    func waitingOnThePersonNeedsYou() {
        let cases: [(JetRunActivity, TaskStatus)] = [
            (.waitingForApproval, .needsPermission),
            (.waitingForAuth, .needsSignIn),
            (.waitingForQuota, .usageLimit),
        ]
        for (activity, expected) in cases {
            let status = TaskStatus.derive(facts: facts(.active, activity), isOffline: false, phase: .editingFiles)
            #expect(status == expected)
            #expect(status.needsYou)
            #expect(status.showsAlertGlyph)
            #expect(!status.isInProgress)
        }
    }

    @Test
    func anUnconfirmedGitStepNeedsYou() {
        var value = facts(.completed, nil)
        value.gitUnconfirmed = true
        let status = TaskStatus.derive(facts: value, isOffline: false, phase: nil)
        #expect(status == .gitUnconfirmed)
        #expect(status.needsYou)
    }

    @Test(arguments: [JetRunLifecycle.created, .starting])
    func startingRunsAreInProgress(_ lifecycle: JetRunLifecycle) {
        let status = TaskStatus.derive(facts: facts(lifecycle, nil), isOffline: false, phase: nil)
        #expect(status == .starting)
        #expect(status.isInProgress)
        #expect(!status.showsAlertGlyph)
    }

    @Test
    func workingCarriesItsPhase() {
        #expect(TaskStatus.derive(facts: facts(.active, .working), isOffline: false, phase: .runningCommand)
            == .working(.runningCommand))
        #expect(TaskStatus.derive(facts: facts(.active, nil), isOffline: false, phase: nil) == .working(nil))
        #expect(TaskStatus.working(nil).isInProgress)
    }

    @Test
    func reconnectingAndStoppingAreInProgress() {
        let reconnecting = TaskStatus.derive(facts: facts(.active, .reconnecting), isOffline: false, phase: nil)
        let stopping = TaskStatus.derive(facts: facts(.stopping, .working), isOffline: false, phase: nil)
        #expect(reconnecting == .reconnecting)
        #expect(stopping == .stopping)
        #expect(reconnecting.isInProgress)
        #expect(stopping.isInProgress)
    }

    @Test
    func failedIsRedButNotNeedsYou() {
        let status = TaskStatus.derive(facts: facts(.failed, nil), isOffline: false, phase: nil)
        #expect(status == .failed)
        #expect(!status.needsYou)
        #expect(status.showsAlertGlyph)
    }

    @Test
    func calmOutcomes() {
        #expect(TaskStatus.derive(facts: facts(.active, .waitingForUser), isOffline: false, phase: .editingFiles)
            == .waitingForReply)
        #expect(TaskStatus.derive(facts: facts(.completed, nil), isOffline: false, phase: nil) == .finished)
        for status in [TaskStatus.waitingForReply, .finished] {
            #expect(!status.needsYou)
            #expect(!status.showsAlertGlyph)
            #expect(!status.isInProgress)
        }
    }

    @Test(arguments: [JetRunLifecycle.canceled, .lost])
    func canceledAndLostAreStoppedNotNeedsYou(_ lifecycle: JetRunLifecycle) {
        // A reboot leaves `lost`: it stays neutral, never red and never Needs You.
        let status = TaskStatus.derive(facts: facts(lifecycle, .waitingForApproval), isOffline: false, phase: nil)
        #expect(status == .stopped)
        #expect(!status.needsYou)
        #expect(!status.showsAlertGlyph)
    }

    @Test
    func unknownAndNotStarted() {
        #expect(TaskStatus.derive(facts: nil, isOffline: false, phase: nil) == .unknown)
        #expect(TaskStatus.derive(facts: TaskStatusFacts(), isOffline: false, phase: nil) == .notStarted)
        #expect(TaskStatus.derive(facts: TaskStatusFacts(hasRuns: true), isOffline: false, phase: nil) == .unknown)
    }

    // MARK: - Priority

    @Test
    func needsYouBeatsProgressAndFailure() {
        var working = facts(.active, .working)
        working.gitUnconfirmed = true
        #expect(TaskStatus.derive(facts: working, isOffline: false, phase: nil) == .gitUnconfirmed)

        var failed = facts(.failed, nil)
        failed.gitUnconfirmed = true
        #expect(TaskStatus.derive(facts: failed, isOffline: false, phase: nil) == .gitUnconfirmed)

        #expect(TaskStatus.derive(facts: facts(.starting, .waitingForAuth), isOffline: false, phase: nil)
            == .needsSignIn)
    }

    @Test
    func finishedRunsIgnoreStaleActivity() {
        #expect(TaskStatus.derive(facts: facts(.completed, .waitingForApproval), isOffline: false, phase: nil)
            == .finished)
        #expect(TaskStatus.derive(facts: facts(.failed, .waitingForQuota), isOffline: false, phase: nil)
            == .failed)
    }

    // MARK: - Phase

    @Test
    func toolNamesMapToPhases() {
        let cases: [(String, TaskPhase)] = [
            ("Edit", .editingFiles), ("Write", .editingFiles), ("MultiEdit", .editingFiles),
            ("Bash", .runningCommand),
            ("Read", .readingProject), ("Grep", .readingProject), ("Glob", .readingProject), ("LS", .readingProject),
            ("WebFetch", .searchingWeb), ("WebSearch", .searchingWeb),
            ("TodoWrite", .working), ("mcp__github__create_issue", .working),
        ]
        for (tool, phase) in cases {
            #expect(TaskPhase.from(toolName: tool) == phase, "\(tool)")
        }
    }

    @Test
    func onlyBareToolIdentifiersCountAsToolNames() {
        #expect(TaskPhase.toolName(of: agent("Read")) == "Read")
        #expect(TaskPhase.toolName(of: agent("  Edit\n")) == "Edit")
        #expect(TaskPhase.toolName(of: agent("mcp__github__create_issue")) == "mcp__github__create_issue")
        #expect(TaskPhase.toolName(of: agent("_private:tool-1.2")) == "_private:tool-1.2")
        #expect(TaskPhase.toolName(of: agent(String(repeating: "a", count: 40))) != nil)

        #expect(TaskPhase.toolName(of: agent(String(repeating: "a", count: 41))) == nil)
        #expect(TaskPhase.toolName(of: agent("Read the file")) == nil)
        #expect(TaskPhase.toolName(of: agent("Read\nEdit")) == nil)
        #expect(TaskPhase.toolName(of: agent("2fa")) == nil)
        #expect(TaskPhase.toolName(of: agent("`Read`")) == nil)
        #expect(TaskPhase.toolName(of: agent("")) == nil)
        #expect(TaskPhase.toolName(of: JetTimelineEntry(id: "u", kind: .user, text: "Read", sequence: 1, rawCount: 0)) == nil)
        #expect(TaskPhase.toolName(of: JetTimelineEntry(id: "x", kind: .activity, text: "Bash", sequence: 1, rawCount: 0)) == nil)
    }

    @Test
    func theLatestToolNameSetsThePhase() {
        #expect(TaskPhase.latest(in: []) == nil)
        #expect(TaskPhase.latest(in: [agent("I'll look into it.")]) == nil)
        #expect(TaskPhase.latest(in: [agent("Read"), agent("Found it."), agent("Bash"), agent("Running the tests now.")])
            == .runningCommand)
    }

    // MARK: - Store

    @Test
    func storeRecordsSnapshotsAndFencesOlderOnes() {
        let store = TaskStatusStore(memory: ClientMemoryTests.isolatedMemory())
        let conversation = JetConversationSummary(
            id: UUID(), revision: 1, title: "Task", createdAtUnixMilliseconds: 1, projectID: nil
        )
        let run = JetRunSummary(
            id: UUID(), conversationID: conversation.id, revision: 1, lifecycle: .active,
            title: "Task", createdAtUnixMilliseconds: 1, endedAtUnixMilliseconds: nil
        )
        let snapshot = JetConversationSnapshot(
            cursor: 10, conversation: conversation, workspaceID: nil, workspaceRoot: nil, runs: [run]
        )

        store.record(
            snapshot: snapshot,
            execution: JetRunExecution(cursor: 10, run: run, activity: .waitingForApproval, needsAttention: true, termination: nil),
            cursor: 10
        )
        #expect(store.facts[conversation.id]?.activity == .waitingForApproval)
        #expect(store.facts[conversation.id]?.hasRuns == true)
        #expect(store.needsYouConversationIDs == [conversation.id])

        store.record(
            snapshot: snapshot,
            execution: JetRunExecution(cursor: 9, run: run, activity: .working, needsAttention: false, termination: nil),
            cursor: 9
        )
        #expect(store.facts[conversation.id]?.activity == .waitingForApproval)

        store.record(
            snapshot: snapshot,
            execution: JetRunExecution(cursor: 11, run: run, activity: .working, needsAttention: false, termination: nil),
            cursor: 11
        )
        #expect(store.facts[conversation.id]?.activity == .working)
        #expect(store.facts[conversation.id]?.lastSequence == 11)
        #expect(store.needsYouConversationIDs.isEmpty)
    }

    @Test
    func aSnapshotAloneKeepsItsRunsActivity() {
        let store = TaskStatusStore(memory: ClientMemoryTests.isolatedMemory())
        let conversation = JetConversationSummary(
            id: UUID(), revision: 1, title: "Task", createdAtUnixMilliseconds: 1, projectID: nil
        )
        let first = run(conversation, .active)
        store.record(
            snapshot: snapshot(conversation, [first], cursor: 10),
            execution: JetRunExecution(cursor: 10, run: first, activity: .waitingForApproval, needsAttention: true, termination: nil),
            cursor: 10
        )

        // A snapshot without an execution keeps what the same live Run reported.
        store.record(snapshot: snapshot(conversation, [first], cursor: 11), execution: nil, cursor: 11)
        #expect(store.facts[conversation.id]?.activity == .waitingForApproval)
        #expect(store.facts[conversation.id]?.lastSequence == 11)
        #expect(store.needsYouConversationIDs == [conversation.id])

        // Another Run's snapshot doesn't inherit it.
        let second = run(conversation, .active)
        store.record(snapshot: snapshot(conversation, [first, second], cursor: 12), execution: nil, cursor: 12)
        #expect(store.facts[conversation.id]?.runID == second.id)
        #expect(store.facts[conversation.id]?.activity == nil)
        #expect(store.needsYouConversationIDs.isEmpty)

        // A finished Run has no activity, even from a snapshot alone.
        store.record(
            snapshot: snapshot(conversation, [first, second], cursor: 13),
            execution: JetRunExecution(cursor: 13, run: second, activity: .waitingForAuth, needsAttention: true, termination: nil),
            cursor: 13
        )
        #expect(store.facts[conversation.id]?.activity == .waitingForAuth)
        let finished = JetRunSummary(
            id: second.id, conversationID: conversation.id, revision: 2, lifecycle: .completed,
            title: "Task", createdAtUnixMilliseconds: 1, endedAtUnixMilliseconds: 2
        )
        store.record(snapshot: snapshot(conversation, [first, finished], cursor: 14), execution: nil, cursor: 14)
        #expect(store.facts[conversation.id]?.lifecycle == .completed)
        #expect(store.facts[conversation.id]?.activity == nil)
    }

    @Test
    func storeTracksUnconfirmedGitStepsAndForgets() {
        let store = TaskStatusStore(memory: ClientMemoryTests.isolatedMemory())
        let conversationID = UUID()

        store.recordGitDeliveries([SessionSelectionTests.unknownDelivery(for: conversationID)], conversationID: conversationID)
        #expect(store.facts[conversationID]?.gitUnconfirmed == true)
        #expect(store.needsYouConversationIDs == [conversationID])

        store.recordGitDeliveries([], conversationID: conversationID)
        #expect(store.facts[conversationID]?.gitUnconfirmed == false)

        store.recordGitDeliveries([SessionSelectionTests.unknownDelivery(for: conversationID)], conversationID: conversationID)
        store.remove(conversationID)
        #expect(store.facts[conversationID] == nil)

        let other = UUID()
        store.recordGitDeliveries([], conversationID: other)
        store.reset(conversationIDs: [other])
        #expect(store.facts.isEmpty)
    }

    // MARK: - Helpers

    private func facts(_ lifecycle: JetRunLifecycle?, _ activity: JetRunActivity?) -> TaskStatusFacts {
        TaskStatusFacts(lifecycle: lifecycle, activity: activity, runID: UUID(), hasRuns: lifecycle != nil)
    }

    private func run(_ conversation: JetConversationSummary, _ lifecycle: JetRunLifecycle) -> JetRunSummary {
        JetRunSummary(
            id: UUID(), conversationID: conversation.id, revision: 1, lifecycle: lifecycle,
            title: conversation.title, createdAtUnixMilliseconds: 1,
            endedAtUnixMilliseconds: lifecycle.isLive ? nil : 2
        )
    }

    private func snapshot(
        _ conversation: JetConversationSummary,
        _ runs: [JetRunSummary],
        cursor: UInt64
    ) -> JetConversationSnapshot {
        JetConversationSnapshot(
            cursor: cursor, conversation: conversation, workspaceID: nil, workspaceRoot: nil, runs: runs
        )
    }

    private func agent(_ text: String) -> JetTimelineEntry {
        JetTimelineEntry(id: UUID().uuidString, kind: .agent, text: text, sequence: 1, rawCount: 0)
    }
}
