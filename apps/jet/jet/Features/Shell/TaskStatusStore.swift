import Foundation
import Observation

/// Status facts for every task the sidebar knows about, not only the selected
/// one. Conversation summaries carry no status, so facts come from snapshots and
/// Events (protocol gap 2). Unknown facts show no glyph.
///
/// Every source is fenced by the Plane sequence it reflects: an Event or a
/// snapshot older than what is already applied never overwrites newer state.
/// Rows that come into view without facts are seeded lazily, at most
/// `maximumSeedsPerComputer` at a time for each computer.
@MainActor
@Observable
final class TaskStatusStore {
    typealias Fetch = @MainActor (_ conversationID: UUID, _ planeRegistryID: UUID) async throws -> (
        cursor: UInt64,
        snapshot: JetConversationSnapshot,
        execution: JetRunExecution?
    )

    static let maximumSeedsPerComputer = 4
    /// A failed seed waits this long before a row coming into view tries again.
    static let seedRetryInterval: Duration = .seconds(30)
    /// A computer that sends Events again is reachable, so failed seeds older
    /// than this try again at once.
    static let reachableRetryInterval: Duration = .seconds(5)

    private(set) var facts: [UUID: TaskStatusFacts] = [:]
    /// The task the person has open, as `noteSelected` last reported it.
    private(set) var selectedConversationID: UUID?

    @ObservationIgnored let memory: ClientMemory
    @ObservationIgnored private(set) var fetch: Fetch?
    @ObservationIgnored private let now: @MainActor () -> ContinuousClock.Instant

    /// The newest Plane sequence applied to each task's facts: the fence.
    @ObservationIgnored private var appliedSequence: [UUID: UInt64] = [:]
    /// The newest reply each task's Events carried. Facts copy it only when the
    /// unread outcome changes, so streaming output doesn't redraw the sidebar per chunk.
    @ObservationIgnored private var pendingReply: [UUID: UInt64] = [:]
    /// Input and recorded changes seen before a task had facts.
    @ObservationIgnored private var pendingInput: [UUID: UInt64] = [:]
    @ObservationIgnored private var pendingChanges: Set<UUID> = []
    @ObservationIgnored private var planeOf: [UUID: UUID] = [:]
    /// Rows on screen, per computer.
    @ObservationIgnored private var visible: [UUID: Set<UUID>] = [:]
    /// Rows waiting for a seed, per computer, in the order they appeared.
    @ObservationIgnored private var queue: [UUID: [UUID]] = [:]
    @ObservationIgnored private var inFlight: [UUID: Seed] = [:]
    @ObservationIgnored private var failedAt: [UUID: ContinuousClock.Instant] = [:]
    /// Bumped when a task's facts are forgotten, so a seed started earlier is dropped.
    @ObservationIgnored private var generation: [UUID: Int] = [:]

    private struct Seed {
        let planeRegistryID: UUID
        let generation: Int
        let task: Task<Void, Never>
    }

    init(
        memory: ClientMemory,
        now: @escaping @MainActor () -> ContinuousClock.Instant = { .now }
    ) {
        self.memory = memory
        self.now = now
    }

    func configure(fetch: @escaping Fetch) {
        self.fetch = fetch
    }

    // MARK: - Snapshots

    /// Records a Conversation snapshot and, when known, its latest Run's execution.
    /// Returns false, changing nothing, when `cursor` is older than what the facts
    /// already reflect. A snapshot alone says nothing about activity, so it keeps
    /// the activity its latest Run last reported while that Run is live. What only
    /// Events and deliveries report (replies, input, recorded changes and an
    /// unconfirmed Git step) is kept.
    @discardableResult
    func record(snapshot: JetConversationSnapshot, execution: JetRunExecution?, cursor: UInt64) -> Bool {
        let conversationID = snapshot.conversation.id
        if let applied = appliedSequence[conversationID], cursor < applied { return false }
        var next = facts[conversationID] ?? TaskStatusFacts()
        let latestRun = snapshot.runs.last
        let matchingExecution = execution.flatMap { execution in
            execution.run.conversationID == conversationID
                && (latestRun == nil || execution.run.id == latestRun?.id)
                ? execution
                : nil
        }
        let activity: JetRunActivity?
        if let matchingExecution {
            activity = matchingExecution.activity
        } else if execution == nil, let runID = next.runID, runID == latestRun?.id {
            activity = next.activity
        } else {
            activity = nil
        }
        next.lifecycle = matchingExecution?.run.lifecycle ?? latestRun?.lifecycle
        next.activity = next.lifecycle?.isLive == true ? activity : nil
        next.runID = matchingExecution?.run.id ?? latestRun?.id
        next.hasRuns = !snapshot.runs.isEmpty || matchingExecution != nil
        next.lastSequence = cursor
        foldPendingBits(conversationID, into: &next)
        appliedSequence[conversationID] = cursor
        // Snapshots arrive once per load, not per output chunk, so their cursor is kept.
        write(conversationID, next, keepsSequence: true)
        return true
    }

    /// A Git step whose outcome is unknown and not yet marked as checked puts the
    /// task in Needs You. Deliveries alone don't count as a status source, so a
    /// task that has only these facts is still seeded when its row appears.
    func recordGitDeliveries(_ deliveries: [JetGitDelivery], conversationID: UUID) {
        let unconfirmed = deliveries.contains { $0.needsAcknowledgement }
        guard var next = facts[conversationID] else {
            // A delivery belongs to a Run's changes, so the task has Runs.
            guard unconfirmed else { return }
            write(conversationID, TaskStatusFacts(hasRuns: true, gitUnconfirmed: true))
            return
        }
        next.gitUnconfirmed = unconfirmed
        write(conversationID, next)
    }

    // MARK: - Events

    /// Updates facts from a live Event from `planeRegistryID`.
    func record(_ event: JetEvent, planeRegistryID: UUID) {
        guard let conversationID = event.conversationID else { return }
        planeOf[conversationID] = planeRegistryID
        retryUnreachableSeeds(on: planeRegistryID)
        guard let signal = event.statusSignal else { return }
        if signal == .trashed {
            remove(conversationID)
            return
        }
        if let applied = appliedSequence[conversationID], applied >= event.sequence { return }

        guard var next = facts[conversationID] else {
            recordWithoutFacts(event, signal: signal, conversationID: conversationID)
            return
        }
        switch signal {
        case .runCreated:
            adopt(event.runID, lifecycle: .created, activity: nil, into: &next)
        case let .lifecycle(_, to):
            if isTracked(event.runID, by: next) {
                if let runID = event.runID { next.runID = runID }
                next.lifecycle = to
                next.hasRuns = true
                if !to.isLive { next.activity = nil }
            } else if to.isLive {
                adopt(event.runID, lifecycle: to, activity: nil, into: &next)
            } else {
                // An older Run ended; the task follows its newer Run.
                return
            }
        case let .activity(activity):
            if isTracked(event.runID, by: next) {
                if let runID = event.runID { next.runID = runID }
                next.activity = activity
                if activity != nil {
                    next.hasRuns = true
                    if next.lifecycle == nil { next.lifecycle = .active }
                }
            } else if let activity {
                adopt(event.runID, lifecycle: .active, activity: activity, into: &next)
            } else {
                return
            }
        case let .output(hasReply, _):
            // Output never implies activity: the Run reports that itself.
            if hasReply { notePendingReply(conversationID, event.sequence) }
        case .input:
            pendingInput[conversationID] = max(pendingInput[conversationID] ?? 0, event.sequence)
            next.lastInputSequence = max(next.lastInputSequence ?? 0, event.sequence)
        case .changesRecorded:
            pendingChanges.insert(conversationID)
            next.hasRecordedChanges = true
        case .approvalRequested, .trashed:
            // The Run's activity carries a waiting request.
            break
        }
        next.lastSequence = event.sequence
        appliedSequence[conversationID] = event.sequence
        write(conversationID, next)
    }

    /// Without facts, only the Run's own reports create them. Output, input and
    /// recorded changes are kept aside and folded in when facts arrive, without
    /// advancing the fence, so a seed already in flight still applies.
    private func recordWithoutFacts(_ event: JetEvent, signal: JetEventStatusSignal, conversationID: UUID) {
        var next = TaskStatusFacts()
        switch signal {
        case .runCreated:
            adopt(event.runID, lifecycle: .created, activity: nil, into: &next)
        case let .lifecycle(_, to):
            adopt(event.runID, lifecycle: to, activity: nil, into: &next)
        case let .activity(activity?):
            adopt(event.runID, lifecycle: .active, activity: activity, into: &next)
        case let .output(hasReply, _):
            if hasReply { notePendingReply(conversationID, event.sequence) }
            return
        case .input:
            pendingInput[conversationID] = max(pendingInput[conversationID] ?? 0, event.sequence)
            return
        case .changesRecorded:
            pendingChanges.insert(conversationID)
            return
        case .activity(nil), .approvalRequested, .trashed:
            return
        }
        next.lastSequence = event.sequence
        foldPendingBits(conversationID, into: &next)
        appliedSequence[conversationID] = event.sequence
        write(conversationID, next)
    }

    private func isTracked(_ runID: UUID?, by facts: TaskStatusFacts) -> Bool {
        runID == nil || facts.runID == nil || runID == facts.runID
    }

    private func adopt(
        _ runID: UUID?,
        lifecycle: JetRunLifecycle,
        activity: JetRunActivity?,
        into facts: inout TaskStatusFacts
    ) {
        facts.runID = runID
        facts.lifecycle = lifecycle
        facts.activity = activity
        facts.hasRuns = true
    }

    private func notePendingReply(_ conversationID: UUID, _ sequence: UInt64) {
        pendingReply[conversationID] = max(pendingReply[conversationID] ?? 0, sequence)
    }

    private func foldPendingBits(_ conversationID: UUID, into facts: inout TaskStatusFacts) {
        if let input = pendingInput[conversationID] {
            facts.lastInputSequence = max(facts.lastInputSequence ?? 0, input)
        }
        if pendingChanges.contains(conversationID) { facts.hasRecordedChanges = true }
    }

    // MARK: - Writes

    /// The newest reply known for a task, including one its facts haven't copied yet.
    func latestReplySequence(_ conversationID: UUID) -> UInt64 {
        max(facts[conversationID]?.lastReplySequence ?? 0, pendingReply[conversationID] ?? 0)
    }

    /// Stores new facts only when something a row shows changed: a field other
    /// than the sequences, or whether the task has an unseen reply. Streaming
    /// output therefore doesn't redraw the sidebar per chunk. `keepsSequence`
    /// also stores a changed `lastSequence`.
    private func write(_ conversationID: UUID, _ proposed: TaskStatusFacts, keepsSequence: Bool = false) {
        var next = proposed
        let reply = max(proposed.lastReplySequence ?? 0, pendingReply[conversationID] ?? 0)
        next.lastReplySequence = reply > 0 ? reply : nil
        guard let current = facts[conversationID] else {
            facts[conversationID] = next
            return
        }
        guard current != next else { return }
        var currentShape = current
        if !keepsSequence { currentShape.lastSequence = next.lastSequence }
        currentShape.lastReplySequence = next.lastReplySequence
        if currentShape != next || hasUnseenReply(conversationID, current) != hasUnseenReply(conversationID, next) {
            facts[conversationID] = next
        }
    }

    /// Whether facts carry a reply newer than the last one seen. The selection
    /// plays no part, so leaving a task can't hide a later reply.
    private func hasUnseenReply(_ conversationID: UUID, _ facts: TaskStatusFacts) -> Bool {
        (facts.lastReplySequence ?? 0) > (memory.seenSequence(conversationID) ?? 0)
    }

    // MARK: - Seen and unread

    /// The person opened a task, or left the open one (nil). Opening marks the
    /// task's replies seen. An explicit nil also marks the task being left,
    /// because its replies were on screen until now; switching straight to
    /// another task doesn't, because the session marks the task it leaves itself.
    func noteSelected(_ conversationID: UUID?) {
        let previous = selectedConversationID
        if previous != conversationID { selectedConversationID = conversationID }
        if let conversationID {
            markSeen(conversationID)
        } else if let previous {
            markSeen(previous)
        }
    }

    /// Marks a task's newest reply as seen.
    func markSeen(_ conversationID: UUID) {
        let latest = latestReplySequence(conversationID)
        guard latest > (memory.seenSequence(conversationID) ?? 0) else { return }
        memory.markSeen(conversationID, sequence: latest)
    }

    /// A reply arrived that the person hasn't seen. The open task is never
    /// unread. Replies from before this launch can't be known (protocol gap 2),
    /// so only replies observed live make a task unread.
    func isUnread(_ conversationID: UUID) -> Bool {
        guard conversationID != selectedConversationID else { return false }
        return latestReplySequence(conversationID) > (memory.seenSequence(conversationID) ?? 0)
    }

    // MARK: - Lazy seeding

    /// A row came into view. A task without facts gets a status load, unless one
    /// is queued or running, or the last one failed within `seedRetryInterval`.
    func noteVisible(_ conversationID: UUID, planeRegistryID: UUID) {
        visible[planeRegistryID, default: []].insert(conversationID)
        if planeOf[conversationID] == nil { planeOf[conversationID] = planeRegistryID }
        enqueueSeed(conversationID, on: planeRegistryID, retryInterval: Self.seedRetryInterval)
    }

    /// A row left the screen. Its queued seed is dropped; a running one completes,
    /// because its result is still useful.
    func noteHidden(_ conversationID: UUID) {
        for plane in visible.keys { visible[plane]?.remove(conversationID) }
        for plane in queue.keys { queue[plane]?.removeAll { $0 == conversationID } }
    }

    /// Seeds running for one computer.
    func seedsInFlight(on planeRegistryID: UUID) -> Int {
        inFlight.values.filter { $0.planeRegistryID == planeRegistryID }.count
    }

    /// Tasks waiting for a seed on one computer, in order.
    func queuedSeeds(on planeRegistryID: UUID) -> [UUID] {
        queue[planeRegistryID] ?? []
    }

    /// Whether a task has no status from a snapshot or an Event yet.
    private func needsSeed(_ conversationID: UUID) -> Bool {
        appliedSequence[conversationID] == nil
    }

    private func enqueueSeed(_ conversationID: UUID, on planeRegistryID: UUID, retryInterval: Duration) {
        guard fetch != nil,
              needsSeed(conversationID),
              inFlight[conversationID] == nil,
              queue[planeRegistryID]?.contains(conversationID) != true
        else { return }
        if let failure = failedAt[conversationID], now() - failure < retryInterval { return }
        queue[planeRegistryID, default: []].append(conversationID)
        pump(planeRegistryID)
    }

    /// Starts queued seeds while the computer has a free slot.
    private func pump(_ planeRegistryID: UUID) {
        guard let fetch else { return }
        while seedsInFlight(on: planeRegistryID) < Self.maximumSeedsPerComputer,
              let conversationID = queue[planeRegistryID]?.first
        {
            queue[planeRegistryID]?.removeFirst()
            let expected = generation[conversationID, default: 0]
            let task = Task { [weak self] in
                do {
                    let result = try await fetch(conversationID, planeRegistryID)
                    guard let self else { return }
                    if self.generation[conversationID, default: 0] == expected {
                        self.record(snapshot: result.snapshot, execution: result.execution, cursor: result.cursor)
                        self.failedAt[conversationID] = nil
                    }
                } catch is CancellationError {
                    // Cancelled on purpose (or a preview session): nothing is known.
                } catch {
                    if let self, self.generation[conversationID, default: 0] == expected {
                        self.failedAt[conversationID] = self.now()
                    }
                }
                self?.finishSeed(conversationID, on: planeRegistryID, generation: expected)
            }
            inFlight[conversationID] = Seed(planeRegistryID: planeRegistryID, generation: expected, task: task)
        }
    }

    private func finishSeed(_ conversationID: UUID, on planeRegistryID: UUID, generation: Int) {
        if inFlight[conversationID]?.generation == generation {
            inFlight.removeValue(forKey: conversationID)
        }
        pump(planeRegistryID)
    }

    /// An Event from a computer shows it is reachable again: rows on screen whose
    /// seed failed a while ago try again without waiting for `seedRetryInterval`.
    private func retryUnreachableSeeds(on planeRegistryID: UUID) {
        guard !failedAt.isEmpty, let rows = visible[planeRegistryID] else { return }
        for conversationID in rows where failedAt[conversationID] != nil {
            enqueueSeed(conversationID, on: planeRegistryID, retryInterval: Self.reachableRetryInterval)
        }
    }

    // MARK: - Forgetting

    func remove(_ conversationID: UUID) {
        let plane = inFlight[conversationID]?.planeRegistryID
        forget(conversationID)
        planeOf.removeValue(forKey: conversationID)
        for key in visible.keys { visible[key]?.remove(conversationID) }
        if let plane { pump(plane) }
    }

    /// Forgets facts that a cursor expiry made untrustworthy. Rows still on
    /// screen load their status again.
    func reset(conversationIDs: some Sequence<UUID>) {
        var planes = Set<UUID>()
        var reseed: [(UUID, UUID)] = []
        for conversationID in conversationIDs {
            if let plane = inFlight[conversationID]?.planeRegistryID { planes.insert(plane) }
            forget(conversationID)
            if let plane = visible.first(where: { $0.value.contains(conversationID) })?.key {
                reseed.append((conversationID, plane))
            }
        }
        for (conversationID, plane) in reseed {
            enqueueSeed(conversationID, on: plane, retryInterval: .zero)
        }
        for plane in planes { pump(plane) }
    }

    private func forget(_ conversationID: UUID) {
        generation[conversationID, default: 0] += 1
        inFlight.removeValue(forKey: conversationID)?.task.cancel()
        for plane in queue.keys { queue[plane]?.removeAll { $0 == conversationID } }
        if facts[conversationID] != nil { facts.removeValue(forKey: conversationID) }
        appliedSequence.removeValue(forKey: conversationID)
        pendingReply.removeValue(forKey: conversationID)
        pendingInput.removeValue(forKey: conversationID)
        pendingChanges.remove(conversationID)
        failedAt.removeValue(forKey: conversationID)
    }

    // MARK: - Needs You

    /// Tasks whose status needs the person, most recently updated first. Offline
    /// state is applied by `DesktopSession`, which knows each computer's connection.
    var needsYouConversationIDs: [UUID] {
        facts
            .filter { TaskStatus.derive(facts: $0.value, isOffline: false, phase: nil).needsYou }
            .map { (id: $0.key, sequence: appliedSequence[$0.key] ?? $0.value.lastSequence) }
            .sorted { left, right in
                left.sequence != right.sequence
                    ? left.sequence > right.sequence
                    : left.id.uuidString < right.id.uuidString
            }
            .map(\.id)
    }

#if DEBUG
    /// Seeds facts directly for previews and screenshots.
    func seedForPreview(_ facts: TaskStatusFacts, for conversationID: UUID) {
        self.facts[conversationID] = facts
        appliedSequence[conversationID] = facts.lastSequence
    }
#endif
}
