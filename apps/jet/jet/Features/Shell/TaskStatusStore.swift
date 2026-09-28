import Foundation
import Observation

/// Status facts for every task the sidebar knows about, not only the selected
/// one. Conversation summaries carry no status, so facts come from snapshots and
/// Events (protocol gap 2). Unknown facts show no glyph.
@MainActor
@Observable
final class TaskStatusStore {
    typealias Fetch = @MainActor (_ conversationID: UUID, _ planeRegistryID: UUID) async throws -> (
        cursor: UInt64,
        snapshot: JetConversationSnapshot,
        execution: JetRunExecution?
    )

    private(set) var facts: [UUID: TaskStatusFacts] = [:]

    @ObservationIgnored let memory: ClientMemory
    @ObservationIgnored private(set) var fetch: Fetch?

    init(memory: ClientMemory) {
        self.memory = memory
    }

    func configure(fetch: @escaping Fetch) {
        self.fetch = fetch
    }

    /// Records a Conversation snapshot and, when known, its latest Run's execution.
    /// A record whose cursor is older than the facts already held is ignored.
    func record(snapshot: JetConversationSnapshot, execution: JetRunExecution?, cursor: UInt64) {
        let conversationID = snapshot.conversation.id
        var next = facts[conversationID] ?? TaskStatusFacts()
        guard cursor >= next.lastSequence else { return }
        let latestRun = snapshot.runs.last
        let matchingExecution = execution.flatMap { execution in
            execution.run.conversationID == conversationID
                && (latestRun == nil || execution.run.id == latestRun?.id)
                ? execution
                : nil
        }
        next.lifecycle = matchingExecution?.run.lifecycle ?? latestRun?.lifecycle
        next.activity = next.lifecycle?.isLive == true ? matchingExecution?.activity : nil
        next.runID = matchingExecution?.run.id ?? latestRun?.id
        next.hasRuns = !snapshot.runs.isEmpty || matchingExecution != nil
        next.lastSequence = cursor
        facts[conversationID] = next
    }

    /// A Git step whose outcome is unknown and not yet marked as checked puts the
    /// task in Needs You.
    func recordGitDeliveries(_ deliveries: [JetGitDelivery], conversationID: UUID) {
        var next = facts[conversationID] ?? TaskStatusFacts()
        next.gitUnconfirmed = deliveries.contains { $0.needsAcknowledgement }
        guard next != facts[conversationID] else { return }
        facts[conversationID] = next
    }

    func remove(_ conversationID: UUID) {
        facts.removeValue(forKey: conversationID)
    }

    /// Forgets facts that a cursor expiry made untrustworthy.
    func reset(conversationIDs: some Sequence<UUID>) {
        for conversationID in conversationIDs {
            facts.removeValue(forKey: conversationID)
        }
    }

    /// Updates facts from a live Event. Filled in by the status work package; until
    /// then snapshots are the only source.
    func record(_ event: JetEvent, planeRegistryID: UUID) {}

    /// A row became visible and may need a lazy status load. Filled in by the
    /// status work package.
    func noteVisible(_ conversationID: UUID, planeRegistryID: UUID) {}

    /// The person opened this task. Filled in by the status work package.
    func noteSelected(_ conversationID: UUID) {}

    /// Tasks whose status needs the person, most recently updated first. Offline
    /// state is applied by `DesktopSession`, which knows each computer's connection.
    var needsYouConversationIDs: [UUID] {
        facts
            .filter { TaskStatus.derive(facts: $0.value, isOffline: false, phase: nil).needsYou }
            .sorted { left, right in
                left.value.lastSequence != right.value.lastSequence
                    ? left.value.lastSequence > right.value.lastSequence
                    : left.key.uuidString < right.key.uuidString
            }
            .map(\.key)
    }

#if DEBUG
    /// Seeds facts directly for previews and screenshots.
    func seedForPreview(_ facts: TaskStatusFacts, for conversationID: UUID) {
        self.facts[conversationID] = facts
    }
#endif
}
