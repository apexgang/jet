import Foundation
import Observation

/// An in-memory cache of recent transcripts, fed by every computer's Event
/// stream, plus a bounded journal replay that fills in the selected task's
/// earlier history. The protocol has no per-Conversation history query, so a
/// transcript may still start part-way through a task (design §6.5, protocol gap 1).
@MainActor
@Observable
final class TranscriptStore {
    enum HistoryState: Equatable, Sendable {
        /// Every entry since the task was created is cached or replayed.
        case complete
        /// Earlier entries may be missing.
        case partial
        /// A bounded journal replay is filling in earlier entries.
        case loading(progress: Double)
        /// Earlier entries could not be recovered on this computer.
        case unavailable
        /// The task has no Runs yet.
        case notStarted
    }

    typealias FetchEvents = @MainActor (_ planeRegistryID: UUID, _ after: UInt64) async throws -> JetEventBatch

    /// The limits of one history replay (design §6.5).
    struct ReplayBudget: Equatable, Sendable {
        /// Windows read backwards from the fence.
        var windows = 10
        /// Journal sequences per window.
        var windowSpan: UInt64 = 2_000
        /// Approximate bytes fetched, counted per Event.
        var maximumBytes = 16 * 1_048_576
        /// Checked between pages; a page in flight is never raced against it.
        var maximumDuration: Duration = .seconds(8)
        /// The wait before a failed replay may start again on its own.
        var retryDelay: Duration = .seconds(15)
        /// Automatic restarts after a failure, until the next success or Try Again.
        var automaticAttempts = 3

        static let standard = ReplayBudget()
    }

    /// Monotonic time for replay budgets and retry delays.
    struct ReplayClock {
        var elapsed: @MainActor () -> Duration

        static var continuous: ReplayClock {
            let clock = ContinuousClock()
            let origin = clock.now
            return ReplayClock { origin.duration(to: clock.now) }
        }
    }

    /// Why a finished replay didn't reach the task's start.
    enum Shortfall: Equatable, Sendable {
        /// The window, byte or time budget ran out.
        case beyondLimit
        /// The journal no longer holds older Events.
        case journalTrimmed
        /// The replayed history has more entries than a transcript keeps.
        case truncated
        /// The computer couldn't answer; Try Again may help.
        case unreachable
    }

    enum ReplayPhase: Equatable, Sendable {
        case idle
        case running(progress: Double)
        /// `nil`: the replay reached the task's start.
        case finished(Shortfall?)
    }

    /// What the latest snapshot says about a task's history.
    struct HistoryFacts: Equatable, Sendable {
        let planeRegistryID: UUID
        let createdAtUnixMilliseconds: Int64
        /// `nil` while the latest Run is live or before any Run has ended.
        let lastRunEndedAtUnixMilliseconds: Int64?
        let hasRuns: Bool
        /// The newest journal sequence the snapshot or the live stream has covered.
        let fence: UInt64
    }

    struct HistoryRecord: Equatable {
        var facts: HistoryFacts?
        var phase = ReplayPhase.idle
        /// Replayed entries, oldest first, at most `entryCapacity`.
        var entries: [JetTimelineEntry] = []
        /// The replay covered journal sequences `(floor, fence]`.
        var floor: UInt64 = 0
        var fence: UInt64 = 0
        var automaticAttempts = 0
        var failedAt: Duration?
        var revision = 0
    }

    /// The row above the transcript that explains its history (design §6.5, §9).
    enum HistoryNotice: Equatable, Sendable {
        /// `nil` progress is indeterminate.
        case loading(progress: Double?)
        case unavailable(summary: String?, canRetry: Bool)
        case noMessages

        var title: String {
            switch self {
            case .loading:
                String(localized: "Loading earlier messages…")
            case let .unavailable(_, canRetry):
                canRetry
                    ? String(localized: "Couldn't load earlier messages.")
                    : String(localized: "Earlier messages aren't available on this Mac.")
            case .noMessages:
                String(localized: "No Messages Yet")
            }
        }
    }

    /// Replayed history combined with the live transcript, and its notice.
    struct HistoryPresentation: Equatable {
        var entries: [JetTimelineEntry]
        var notice: HistoryNotice?
    }

    /// One journal Event of the replayed task.
    struct ReplayItem: Equatable, Sendable {
        let sequence: UInt64
        let projections: [JetTimelineEntry]
    }

    /// At most this many Conversations stay cached, least recently used first out.
    nonisolated static let conversationCapacity = 32
    /// Each cached transcript keeps at most this many entries.
    nonisolated static let entryCapacity = 256

    private var transcripts: [UUID: [JetTimelineEntry]] = [:]
    /// Least recently used first.
    private var recency: [UUID] = []
    private var observedStarts: Set<UUID> = []
    /// Tasks whose cached transcript dropped older entries at the cap.
    private var truncatedIDs: Set<UUID> = []
    private var histories: [UUID: HistoryRecord] = [:]
    /// The task whose snapshot loaded last. Eviction passes over it.
    @ObservationIgnored private var activeConversationID: UUID?
    @ObservationIgnored private var fetchEvents: FetchEvents?
    @ObservationIgnored private var replayTask: Task<Void, Never>?
    @ObservationIgnored private var replayConversationID: UUID?
    @ObservationIgnored private var replayGeneration = 0
    @ObservationIgnored private var presentationMemo: PresentationMemo?
    private let budget: ReplayBudget
    private let clock: ReplayClock
    private let now: @MainActor () -> Date

    init(
        budget: ReplayBudget = .standard,
        clock: ReplayClock = .continuous,
        now: @escaping @MainActor () -> Date = { Date() }
    ) {
        self.budget = budget
        self.clock = clock
        self.now = now
    }

    /// Cached Conversation IDs, least recently used first.
    var cachedConversationIDs: [UUID] { recency }

    func configure(fetchEvents: @escaping FetchEvents) {
        self.fetchEvents = fetchEvents
    }

    /// The live cache only; `presentation(for:live:)` adds replayed history.
    func entries(for conversationID: UUID) -> [JetTimelineEntry] {
        transcripts[conversationID] ?? []
    }

    /// Replaces the cached transcript, for example with the selected task's timeline.
    func save(_ entries: [JetTimelineEntry], for conversationID: UUID) {
        if entries.count > Self.entryCapacity {
            transcripts[conversationID] = Array(entries.suffix(Self.entryCapacity))
            markPartial(conversationID)
        } else {
            transcripts[conversationID] = entries
        }
        touch(conversationID)
    }

    /// Applies one Event's projections to a task that isn't on screen.
    func ingest(
        projections: [JetTimelineEntry],
        rawSequence: UInt64,
        conversationID: UUID
    ) {
        var entries = transcripts[conversationID] ?? []
        var dropped = false
        if projections.isEmpty {
            dropped = Self.groupRaw(sequence: rawSequence, into: &entries)
        } else {
            for projection in projections {
                dropped = Self.merge(projection, into: &entries) || dropped
            }
        }
        transcripts[conversationID] = entries
        if dropped { markPartial(conversationID) }
        touch(conversationID)
    }

    /// This client saw the task's `conversation.created` Event, so the cache can
    /// hold its whole history.
    func markObservedStart(_ conversationID: UUID) {
        guard !observedStarts.contains(conversationID) else { return }
        observedStarts.insert(conversationID)
    }

    /// Earlier entries were dropped from the cached transcript.
    func markPartial(_ conversationID: UUID) {
        guard !truncatedIDs.contains(conversationID) else { return }
        truncatedIDs.insert(conversationID)
    }

    func historyState(for conversationID: UUID) -> HistoryState {
        historyState(for: conversationID, liveIsEmpty: (transcripts[conversationID] ?? []).isEmpty)
    }

    /// A task's replay record, for tests and diagnostics.
    func historyRecord(for conversationID: UUID) -> HistoryRecord? {
        histories[conversationID]
    }

    private func historyState(for conversationID: UUID, liveIsEmpty: Bool) -> HistoryState {
        let record = histories[conversationID]
        if let facts = record?.facts, !facts.hasRuns {
            return liveIsEmpty && (record?.entries.isEmpty ?? true) ? .notStarted : .complete
        }
        let phase = record?.phase ?? .idle
        let observed = observedStarts.contains(conversationID)
        if truncatedIDs.contains(conversationID), observed || phase == .finished(nil) {
            return .unavailable
        }
        if observed { return .complete }
        switch phase {
        case .idle: return .partial
        case let .running(progress): return .loading(progress: progress)
        case .finished(nil): return .complete
        case .finished: return .unavailable
        }
    }

    // MARK: Snapshots and replay

    /// Records what a fresh snapshot of the selected task says about its history
    /// and starts the bounded journal replay when earlier entries may be missing.
    /// A finished replay is remembered, so repeated loads never restart it.
    func didLoad(
        snapshot: JetConversationSnapshot,
        planeRegistryID: UUID,
        headCursor: UInt64
    ) {
        let conversationID = snapshot.conversation.id
        if let running = replayConversationID, running != conversationID {
            cancelReplay()
        }

        let runs = snapshot.runs
        let facts = HistoryFacts(
            planeRegistryID: planeRegistryID,
            createdAtUnixMilliseconds: snapshot.conversation.createdAtUnixMilliseconds,
            lastRunEndedAtUnixMilliseconds: runs.last?.lifecycle.isLive == true
                ? nil
                : runs.compactMap(\.endedAtUnixMilliseconds).max(),
            hasRuns: !runs.isEmpty,
            fence: max(snapshot.cursor, headCursor)
        )
        let previous = histories[conversationID]?.facts
        if let previous, !previous.hasRuns, facts.hasRuns {
            // The cache knew this task before its first Run and has followed every
            // Event since; eviction, removal or a cursor expiry drops this record.
            markObservedStart(conversationID)
        }
        if previous != facts {
            histories[conversationID, default: HistoryRecord()].facts = facts
        }
        activeConversationID = conversationID
        touch(conversationID)

        guard facts.hasRuns,
              !observedStarts.contains(conversationID),
              fetchEvents != nil,
              let record = histories[conversationID]
        else { return }
        switch record.phase {
        case .idle:
            startReplay(conversationID)
        case .finished(.unreachable):
            guard record.automaticAttempts < budget.automaticAttempts,
                  let failedAt = record.failedAt,
                  clock.elapsed() - failedAt >= budget.retryDelay
            else { return }
            histories[conversationID]?.automaticAttempts += 1
            startReplay(conversationID)
        case .running, .finished:
            // A running replay keeps its fence; a finished one is kept until the
            // record leaves the cache.
            break
        }
    }

    /// Try Again, after a replay couldn't reach the computer.
    func retryReplay(for conversationID: UUID) {
        guard histories[conversationID]?.phase == .finished(.unreachable),
              fetchEvents != nil
        else { return }
        histories[conversationID]?.automaticAttempts = 0
        activeConversationID = conversationID
        startReplay(conversationID)
    }

    /// Cancels a running history replay; its task falls back to a partial history.
    func cancelReplay() {
        stopReplay()
        activeConversationID = nil
    }

    /// Returns once no replay is running. For tests.
    func waitForReplay() async {
        while let task = replayTask {
            await task.value
            if replayTask == task { return }
        }
    }

    func remove(_ conversationID: UUID) {
        if replayConversationID == conversationID { stopReplay() }
        if activeConversationID == conversationID { activeConversationID = nil }
        recency.removeAll { $0 == conversationID }
        drop(conversationID)
    }

    /// Drops transcripts whose Event stream can no longer be trusted to be
    /// continuous, for example after the Plane cursor expired.
    func reset(conversationIDs: some Sequence<UUID>) {
        for conversationID in conversationIDs { remove(conversationID) }
    }

    private func touch(_ conversationID: UUID) {
        if recency.last != conversationID {
            recency.removeAll { $0 == conversationID }
            recency.append(conversationID)
        }
        while recency.count > Self.conversationCapacity {
            guard let index = recency.firstIndex(where: {
                $0 != activeConversationID && $0 != replayConversationID
            }) else { return }
            drop(recency.remove(at: index))
        }
    }

    /// Forgets everything about a task: its transcript, start, truncation and history.
    private func drop(_ conversationID: UUID) {
        transcripts.removeValue(forKey: conversationID)
        observedStarts.remove(conversationID)
        truncatedIDs.remove(conversationID)
        histories.removeValue(forKey: conversationID)
    }

    private func startReplay(_ conversationID: UUID) {
        guard let fetchEvents, let facts = histories[conversationID]?.facts else { return }
        stopReplay()
        let generation = replayGeneration
        replayConversationID = conversationID
        histories[conversationID]?.phase = .running(progress: 0)
        replayTask = Task { [weak self] in
            await self?.replay(conversationID, facts: facts, fetch: fetchEvents, generation: generation)
        }
    }

    /// Cancels the replay task and moves its record back to idle, keeping any
    /// entries an earlier replay published.
    private func stopReplay() {
        replayTask?.cancel()
        replayTask = nil
        replayGeneration += 1
        if let conversationID = replayConversationID,
           case .running = histories[conversationID]?.phase
        {
            histories[conversationID]?.phase = .idle
        }
        replayConversationID = nil
    }

    /// Reads the journal backwards from the fence in windows, each read forwards,
    /// until the task's start or a budget. An unfinished window is dropped, so
    /// the published history never has a gap.
    private func replay(
        _ conversationID: UUID,
        facts: HistoryFacts,
        fetch: FetchEvents,
        generation: Int
    ) async {
        func isStale() -> Bool {
            generation != replayGeneration || Task.isCancelled
        }
        let head = facts.fence
        let span = max(budget.windowSpan, 1)
        let startedAt = clock.elapsed()
        var end = head
        var done = 0
        var bytes = 0
        var windows: [[ReplayItem]] = []
        var floor = head
        var progress = 0.0
        var oldestRecorded: Int64?
        var outcome: Shortfall?

        windowLoop: while end > 0 {
            if done >= budget.windows {
                outcome = .beyondLimit
                break
            }
            let start = end > span ? end - span : 0
            var after = start
            var items: [ReplayItem] = []
            var sawStart = false
            var trimmed: UInt64?
            pageLoop: while after < end {
                if clock.elapsed() - startedAt >= budget.maximumDuration || bytes >= budget.maximumBytes {
                    outcome = .beyondLimit
                    break windowLoop
                }
                let batch: JetEventBatch
                do {
                    batch = try await fetch(facts.planeRegistryID, after)
                } catch {
                    if isStale() { return }
                    if Self.isCancellation(error) {
                        // Not this store's cancellation: the session went away.
                        abandonReplay(conversationID)
                        return
                    }
                    if let minimum = Self.minimumAvailableCursor(in: error), minimum > after {
                        trimmed = minimum
                        if minimum >= end { break pageLoop }
                        after = minimum
                        continue
                    }
                    outcome = .unreachable
                    break windowLoop
                }
                if isStale() { return }
                // Pages stop at 256 Events or 512 KiB, so advance by the last Event,
                // not the batch cursor.
                guard let last = batch.events.last, last.sequence > after else { break }
                for event in batch.events {
                    bytes += event.payload.source.utf8.count
                        + event.actor.source.utf8.count
                        + (event.origin?.source.utf8.count ?? 0)
                        + 256
                    if event.sequence > end { break }
                    oldestRecorded = min(oldestRecorded ?? .max, event.recordedAtUnixMilliseconds)
                    guard event.conversationID == conversationID else { continue }
                    if event.kind == "conversation.created" || event.kind == "conversation.transfer_imported" {
                        sawStart = true
                    }
                    items.append(ReplayItem(
                        sequence: event.sequence,
                        projections: DesktopSession.stamped(event.timelineProjections(), from: event)
                    ))
                }
                after = last.sequence
                progress = Self.replayProgress(
                    previous: progress,
                    window: (done: done, after: after, start: start, end: end),
                    bytes: bytes,
                    elapsed: clock.elapsed() - startedAt,
                    oldestRecordedAtUnixMilliseconds: oldestRecorded,
                    createdAtUnixMilliseconds: facts.createdAtUnixMilliseconds,
                    nowUnixMilliseconds: Int64(now().timeIntervalSince1970 * 1_000),
                    budget: budget
                )
                histories[conversationID]?.phase = .running(progress: progress)
            }
            windows.append(items)
            done += 1
            floor = trimmed.map { min($0, end) } ?? start
            if sawStart || (start == 0 && trimmed == nil) { break }
            if trimmed != nil {
                outcome = .journalTrimmed
                break
            }
            if Self.rebuild(windows).entries.count >= Self.entryCapacity {
                outcome = .truncated
                break
            }
            end = start
        }
        if isStale() { return }
        publishReplay(conversationID, windows: windows, floor: floor, fence: head, outcome: outcome)
    }

    private func publishReplay(
        _ conversationID: UUID,
        windows: [[ReplayItem]],
        floor: UInt64,
        fence: UInt64,
        outcome: Shortfall?
    ) {
        replayTask = nil
        replayConversationID = nil
        guard var record = histories[conversationID] else { return }
        var outcome = outcome
        var (entries, firstSequences) = Self.rebuild(windows)
        if outcome != nil {
            // A message that starts right above the floor may have earlier parts below it.
            entries.removeAll { $0.kind == .user && firstSequences[$0.id] == floor &+ 1 }
        }
        if entries.count > Self.entryCapacity {
            entries.removeFirst(entries.count - Self.entryCapacity)
            if outcome == nil { outcome = .truncated }
        }
        record.entries = entries
        record.floor = floor
        record.fence = fence
        record.phase = .finished(outcome)
        record.revision += 1
        if outcome == .unreachable { record.failedAt = clock.elapsed() }
        if outcome == nil { record.automaticAttempts = 0 }
        histories[conversationID] = record
    }

    private func abandonReplay(_ conversationID: UUID) {
        replayTask = nil
        replayConversationID = nil
        if case .running = histories[conversationID]?.phase {
            histories[conversationID]?.phase = .idle
        }
    }

    /// Replay progress: never lower than before and at most 0.99 until it finishes.
    static func replayProgress(
        previous: Double,
        window: (done: Int, after: UInt64, start: UInt64, end: UInt64),
        bytes: Int,
        elapsed: Duration,
        oldestRecordedAtUnixMilliseconds: Int64?,
        createdAtUnixMilliseconds: Int64,
        nowUnixMilliseconds: Int64,
        budget: ReplayBudget
    ) -> Double {
        var candidates: [Double] = []
        if window.end > window.start, budget.windows > 0 {
            let read = min(max(window.after, window.start), window.end) - window.start
            let fraction = Double(read) / Double(window.end - window.start)
            candidates.append((Double(window.done) + fraction) / Double(budget.windows))
        }
        if budget.maximumBytes > 0 {
            candidates.append(Double(bytes) / Double(budget.maximumBytes))
        }
        if budget.maximumDuration > .zero {
            candidates.append(elapsed / budget.maximumDuration)
        }
        if let oldest = oldestRecordedAtUnixMilliseconds,
           nowUnixMilliseconds > createdAtUnixMilliseconds
        {
            let covered = Double(nowUnixMilliseconds - min(oldest, nowUnixMilliseconds))
            candidates.append(covered / Double(nowUnixMilliseconds - createdAtUnixMilliseconds))
        }
        let value = candidates.filter(\.isFinite).max() ?? 0
        return min(0.99, max(previous, value))
    }

    private static func presentationError(in error: any Error) -> JetPresentationError? {
        switch error {
        case let failure as JetClientFailure:
            if case let .presentation(presentation) = failure { return presentation }
            return nil
        case let presentation as JetPresentationError:
            return presentation
        default:
            return nil
        }
    }

    private static func isCancellation(_ error: any Error) -> Bool {
        error is CancellationError || presentationError(in: error)?.category == .cancelled
    }

    private static func minimumAvailableCursor(in error: any Error) -> UInt64? {
        if case let .cursorExpired(minimum, _) = presentationError(in: error)?.restart {
            return minimum
        }
        return nil
    }

    /// Applies replayed windows oldest first, without a cap. Also returns the
    /// sequence at which each entry first appeared.
    static func rebuild(
        _ windows: [[ReplayItem]]
    ) -> (entries: [JetTimelineEntry], firstSequences: [String: UInt64]) {
        var entries: [JetTimelineEntry] = []
        var firstSequences: [String: UInt64] = [:]
        for window in windows.reversed() {
            for item in window {
                if item.projections.isEmpty {
                    groupRaw(sequence: item.sequence, into: &entries, limit: .max)
                    if let id = entries.last?.id, firstSequences[id] == nil {
                        firstSequences[id] = item.sequence
                    }
                } else {
                    for projection in item.projections {
                        merge(projection, into: &entries, limit: .max)
                        if firstSequences[projection.id] == nil {
                            firstSequences[projection.id] = item.sequence
                        }
                    }
                }
            }
        }
        return (entries, firstSequences)
    }

    // MARK: Presentation

    private struct PresentationKey: Equatable {
        let conversationID: UUID
        let revision: Int
        let phase: ReplayPhase
        let facts: HistoryFacts?
        let observedStart: Bool
        let truncated: Bool
        let canFetch: Bool
    }

    private struct PresentationMemo {
        let key: PresentationKey
        let live: [JetTimelineEntry]
        let value: HistoryPresentation
    }

    /// Replayed history, then the live transcript, with the notice that explains
    /// what's missing. Without a history record the live transcript comes back
    /// unchanged.
    func presentation(for conversationID: UUID, live: [JetTimelineEntry]) -> HistoryPresentation {
        guard let record = histories[conversationID] else {
            return HistoryPresentation(entries: live, notice: nil)
        }
        let key = PresentationKey(
            conversationID: conversationID,
            revision: record.revision,
            phase: record.phase,
            facts: record.facts,
            observedStart: observedStarts.contains(conversationID),
            truncated: truncatedIDs.contains(conversationID),
            canFetch: fetchEvents != nil
        )
        // Approval entries change in place, so the whole live list is compared;
        // an unchanged array compares by buffer identity.
        if let memo = presentationMemo, memo.key == key, memo.live == live {
            return memo.value
        }
        let combined = Self.combine(
            live: live,
            history: record.entries,
            floor: record.floor,
            fence: record.fence
        )
        let value = HistoryPresentation(
            entries: combined.entries,
            notice: notice(for: conversationID, record: record, live: live, combined: combined)
        )
        presentationMemo = PresentationMemo(key: key, live: live, value: value)
        return value
    }

    private func notice(
        for conversationID: UUID,
        record: HistoryRecord,
        live: [JetTimelineEntry],
        combined: (entries: [JetTimelineEntry], droppedOlder: Bool)
    ) -> HistoryNotice? {
        let hasRuns = record.facts?.hasRuns == true
        let summary = record.facts.map {
            Self.summary($0, now: now(), locale: JetCopy.uiLocale, timeZone: .autoupdatingCurrent)
        }
        switch historyState(for: conversationID, liveIsEmpty: live.isEmpty) {
        case let .loading(progress):
            return .loading(progress: progress)
        case .partial:
            guard hasRuns, live.isEmpty, fetchEvents != nil else { return nil }
            return .loading(progress: nil)
        case .unavailable:
            return .unavailable(summary: summary, canRetry: record.phase == .finished(.unreachable))
        case .notStarted:
            return combined.entries.isEmpty ? .noMessages : nil
        case .complete:
            if combined.droppedOlder { return .unavailable(summary: summary, canRetry: false) }
            guard combined.entries.isEmpty, hasRuns else { return nil }
            return .noMessages
        }
    }

    /// Combines replayed history with the live transcript (design §6.5). The
    /// replay is authoritative for `(floor, fence]`; live entries at or below the
    /// floor and after the fence surround it. Ids stay unique: a repeated id
    /// keeps its first position, and the longer message or the later approval wins.
    static func combine(
        live: [JetTimelineEntry],
        history: [JetTimelineEntry],
        floor: UInt64,
        fence: UInt64,
        limit: Int = entryCapacity
    ) -> (entries: [JetTimelineEntry], droppedOlder: Bool) {
        var entries: [JetTimelineEntry] = []
        entries.reserveCapacity(live.count + history.count)
        var positions: [String: Int] = [:]
        func upsert(_ entry: JetTimelineEntry) {
            guard let index = positions[entry.id] else {
                positions[entry.id] = entries.count
                entries.append(entry)
                return
            }
            if prefers(entry, over: entries[index]) { entries[index] = entry }
        }
        var liveByID: [String: JetTimelineEntry] = [:]
        for entry in live {
            if let existing = liveByID[entry.id], !prefers(entry, over: existing) { continue }
            liveByID[entry.id] = entry
        }

        for entry in live {
            if let sequence = entry.sequence, sequence <= floor { upsert(entry) }
        }
        for entry in history {
            upsert(entry)
            if let twin = liveByID[entry.id] { upsert(twin) }
        }
        for entry in live {
            if let sequence = entry.sequence, sequence <= fence { continue }
            upsert(entry)
        }

        let limit = max(limit, 0)
        guard entries.count > limit else { return (entries, false) }
        return (Array(entries.suffix(limit)), true)
    }

    /// Whether `candidate` replaces `existing`, an entry with the same id.
    private static func prefers(_ candidate: JetTimelineEntry, over existing: JetTimelineEntry) -> Bool {
        switch existing.kind {
        case .user:
            candidate.text.utf8.count > existing.text.utf8.count
        case .approval:
            (candidate.sequence ?? 0) > (existing.sequence ?? 0)
        default:
            false
        }
    }

    /// "Started Sep 26 · Last run ended Sep 27", with the year when it isn't
    /// the current one.
    static func summary(
        _ facts: HistoryFacts,
        now: Date,
        locale: Locale,
        timeZone: TimeZone
    ) -> String {
        var calendar = locale.calendar
        calendar.timeZone = timeZone
        let currentYear = calendar.component(.year, from: now)
        func day(_ milliseconds: Int64) -> String {
            let date = Date(timeIntervalSince1970: Double(milliseconds) / 1_000)
            var style = Date.FormatStyle(locale: locale, calendar: calendar, timeZone: timeZone)
                .month(.abbreviated)
                .day()
            if calendar.component(.year, from: date) != currentYear { style = style.year() }
            return date.formatted(style)
        }
        let started = day(facts.createdAtUnixMilliseconds)
        guard let ended = facts.lastRunEndedAtUnixMilliseconds else {
            return String(localized: "Started \(started)")
        }
        return String(localized: "Started \(started) · Last run ended \(day(ended))")
    }

    #if DEBUG
    /// Sets a task's history directly for previews and never fetches. The task
    /// counts as joined late, so the seeded phase decides its notice.
    func seedPreviewHistory(
        _ conversationID: UUID,
        facts: HistoryFacts,
        phase: ReplayPhase,
        entries: [JetTimelineEntry] = [],
        floor: UInt64 = 0,
        fence: UInt64 = 0
    ) {
        observedStarts.remove(conversationID)
        truncatedIDs.remove(conversationID)
        histories[conversationID] = HistoryRecord(
            facts: facts,
            phase: phase,
            entries: entries,
            floor: floor,
            fence: fence,
            revision: (histories[conversationID]?.revision ?? 0) + 1
        )
        touch(conversationID)
    }
    #endif

    // MARK: Merge rules

    /// Merges one projection into a transcript. Approval cards update in place,
    /// streamed user text appends to its Turn entry, everything else appends.
    /// Returns true when the entry cap (`.max`: none) dropped older entries.
    @discardableResult
    static func merge(
        _ entry: JetTimelineEntry,
        into timeline: inout [JetTimelineEntry],
        limit: Int = entryCapacity
    ) -> Bool {
        if entry.kind == .approval,
           let index = timeline.firstIndex(where: { $0.id == entry.id })
        {
            timeline[index] = entry
        } else if entry.kind == .user,
                  let index = timeline.firstIndex(where: { $0.id == entry.id })
        {
            timeline[index].text += entry.text
            timeline[index].sequence = entry.sequence
        } else {
            timeline.append(entry)
            return capEntries(&timeline, limit: limit)
        }
        return false
    }

    /// Folds an Event without a known projection into the trailing
    /// "background updates" entry. Returns true when the cap dropped older entries.
    @discardableResult
    static func groupRaw(
        sequence: UInt64,
        into timeline: inout [JetTimelineEntry],
        limit: Int = entryCapacity
    ) -> Bool {
        if let index = timeline.indices.last, timeline[index].rawCount > 0 {
            timeline[index].rawCount += 1
            timeline[index].text = String(localized: "\(timeline[index].rawCount) background updates")
            timeline[index].sequence = sequence
            timeline[index].isTechnical = true
            return false
        }
        timeline.append(
            JetTimelineEntry(
                id: "raw-\(sequence)",
                kind: .activity,
                text: String(localized: "1 background update"),
                sequence: sequence,
                rawCount: 1,
                isTechnical: true
            )
        )
        return capEntries(&timeline, limit: limit)
    }

    private static func capEntries(_ timeline: inout [JetTimelineEntry], limit: Int) -> Bool {
        guard timeline.count > limit else { return false }
        timeline.removeFirst(timeline.count - limit)
        return true
    }
}
