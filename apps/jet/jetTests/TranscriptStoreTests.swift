import Foundation
import Testing
@testable import jet

@MainActor
struct TranscriptStoreTests {
    @Test
    func theLeastRecentlyUsedTaskLeavesAtThirtyThree() {
        let store = TranscriptStore()
        let ids = (0 ..< TranscriptStore.conversationCapacity).map { _ in UUID() }
        for id in ids {
            store.save([entry("m", .agent, "Hello")], for: id)
        }
        #expect(store.cachedConversationIDs.count == 32)

        // Using the oldest task again protects it; the next oldest leaves instead.
        store.save([entry("m", .agent, "Again")], for: ids[0])
        let newcomer = UUID()
        store.ingest(projections: [entry("n", .agent, "New")], rawSequence: 1, conversationID: newcomer)

        #expect(store.cachedConversationIDs.count == 32)
        #expect(store.entries(for: ids[0]).map(\.text) == ["Again"])
        #expect(store.entries(for: ids[1]).isEmpty)
        #expect(store.entries(for: newcomer).map(\.text) == ["New"])
    }

    @Test
    func eachTranscriptKeepsTheNewest256Entries() {
        let store = TranscriptStore()
        let id = UUID()
        store.markObservedStart(id)
        #expect(store.historyState(for: id) == .complete)

        for index in 0 ..< 300 {
            store.ingest(
                projections: [entry("e\(index)", .agent, "Entry \(index)", sequence: UInt64(index))],
                rawSequence: UInt64(index),
                conversationID: id
            )
        }

        let entries = store.entries(for: id)
        #expect(entries.count == 256)
        #expect(entries.first?.text == "Entry 44")
        #expect(entries.last?.text == "Entry 299")
        #expect(store.historyState(for: id) == .partial)

        let saved = UUID()
        store.save((0 ..< 260).map { entry("s\($0)", .agent, "\($0)") }, for: saved)
        #expect(store.entries(for: saved).count == 256)
        #expect(store.entries(for: saved).first?.text == "4")
    }

    @Test
    func mergeRulesMatchTheTranscriptBehaviour() {
        var timeline: [JetTimelineEntry] = []

        TranscriptStore.merge(entry("turn", .user, "Fix the "), into: &timeline)
        TranscriptStore.merge(entry("turn", .user, "redirect", sequence: 2), into: &timeline)
        #expect(timeline.count == 1)
        #expect(timeline[0].text == "Fix the redirect")
        #expect(timeline[0].sequence == 2)

        let requested = approval(.requested)
        TranscriptStore.merge(requested, into: &timeline)
        TranscriptStore.merge(approval(.denied), into: &timeline)
        #expect(timeline.count == 2)
        #expect(timeline[1].approval?.state == .denied)

        TranscriptStore.merge(entry("a1", .agent, "Done"), into: &timeline)
        TranscriptStore.merge(entry("a1", .agent, "Done"), into: &timeline)
        #expect(timeline.count == 4)

        TranscriptStore.groupRaw(sequence: 10, into: &timeline)
        TranscriptStore.groupRaw(sequence: 11, into: &timeline)
        #expect(timeline.count == 5)
        #expect(timeline[4].id == "raw-10")
        #expect(timeline[4].text == "2 background updates")
        #expect(timeline[4].rawCount == 2)
        #expect(timeline[4].sequence == 11)
        #expect(timeline[4].isTechnical)

        TranscriptStore.merge(entry("a2", .agent, "Next"), into: &timeline)
        TranscriptStore.groupRaw(sequence: 12, into: &timeline)
        #expect(timeline.last?.text == "1 background update")
        #expect(timeline.last?.id == "raw-12")
    }

    @Test
    func tasksDoNotShareTranscripts() {
        let store = TranscriptStore()
        let first = UUID()
        let second = UUID()

        store.ingest(projections: [entry("a", .agent, "First only")], rawSequence: 1, conversationID: first)
        store.ingest(projections: [], rawSequence: 2, conversationID: second)

        #expect(store.entries(for: first).map(\.text) == ["First only"])
        #expect(store.entries(for: second).map(\.text) == ["1 background update"])

        store.remove(first)
        #expect(store.entries(for: first).isEmpty)
        #expect(store.entries(for: second).count == 1)

        store.reset(conversationIDs: [second])
        #expect(store.cachedConversationIDs.isEmpty)
    }

    @Test
    func snapshotsDecideWhetherHistoryIsComplete() {
        let store = TranscriptStore()
        let observed = UUID()
        let joinedLate = UUID()
        let fresh = UUID()
        store.markObservedStart(observed)

        store.didLoad(snapshot: snapshot(observed, runs: 1), planeRegistryID: UUID(), headCursor: 5)
        store.didLoad(snapshot: snapshot(joinedLate, runs: 1), planeRegistryID: UUID(), headCursor: 5)
        store.didLoad(snapshot: snapshot(fresh, runs: 0), planeRegistryID: UUID(), headCursor: 5)

        #expect(store.historyState(for: observed) == .complete)
        #expect(store.historyState(for: joinedLate) == .partial)
        #expect(store.historyState(for: fresh) == .notStarted)

        store.remove(observed)
        #expect(store.historyState(for: observed) == .partial)

        // A task followed from before its first Run keeps its whole history.
        store.didLoad(snapshot: snapshot(fresh, runs: 1), planeRegistryID: UUID(), headCursor: 6)
        #expect(store.historyState(for: fresh) == .complete)
    }

    // MARK: - Helpers

    private func entry(
        _ id: String,
        _ kind: JetTimelineKind,
        _ text: String,
        sequence: UInt64 = 1
    ) -> JetTimelineEntry {
        JetTimelineEntry(id: id, kind: kind, text: text, sequence: sequence, rawCount: 0)
    }

    private func approval(_ state: JetApprovalState) -> JetTimelineEntry {
        JetTimelineEntry(
            id: "approval-run-req-1",
            kind: .approval,
            text: "shell needs approval.",
            sequence: 3,
            rawCount: 0,
            approval: JetApprovalPresentation(
                requestID: "req-1", reviewID: nil, runID: nil, tool: "shell", action: "{}",
                target: "Current Run", scope: "This action once", consequence: "",
                rationale: nil, state: state, canAuthorizeRetry: false
            )
        )
    }

    private func snapshot(_ id: UUID, runs: Int) -> JetConversationSnapshot {
        let conversation = JetConversationSummary(
            id: id, revision: 1, title: "Task", createdAtUnixMilliseconds: 1, projectID: nil
        )
        return JetConversationSnapshot(
            cursor: 5,
            conversation: conversation,
            workspaceID: nil,
            workspaceRoot: nil,
            runs: (0 ..< runs).map { _ in
                JetRunSummary(
                    id: UUID(), conversationID: id, revision: 1, lifecycle: .completed,
                    title: "Task", createdAtUnixMilliseconds: 1, endedAtUnixMilliseconds: 2
                )
            }
        )
    }
}
