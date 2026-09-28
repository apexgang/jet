import Foundation
import Observation

/// An in-memory cache of recent transcripts, fed by every computer's Event
/// stream. The protocol has no per-Conversation history query, so a transcript
/// here may start part-way through a task (design §6.5, protocol gap 1).
@MainActor
@Observable
final class TranscriptStore {
    enum HistoryState: Equatable, Sendable {
        /// Every entry since the task was created is cached.
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

    /// At most this many Conversations stay cached, least recently used first out.
    static let conversationCapacity = 32
    /// Each cached transcript keeps at most this many entries.
    static let entryCapacity = 256

    private var transcripts: [UUID: [JetTimelineEntry]] = [:]
    /// Least recently used first.
    private var recency: [UUID] = []
    private var observedStarts: Set<UUID> = []
    private var histories: [UUID: HistoryState] = [:]
    @ObservationIgnored private var fetchEvents: FetchEvents?

    /// Cached Conversation IDs, least recently used first.
    var cachedConversationIDs: [UUID] { recency }

    func configure(fetchEvents: @escaping FetchEvents) {
        self.fetchEvents = fetchEvents
    }

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
        observedStarts.insert(conversationID)
    }

    /// Earlier entries were dropped from the cached transcript.
    func markPartial(_ conversationID: UUID) {
        switch histories[conversationID] {
        case .loading, .unavailable: break
        default: histories[conversationID] = .partial
        }
    }

    func historyState(for conversationID: UUID) -> HistoryState {
        if let state = histories[conversationID] { return state }
        return observedStarts.contains(conversationID) ? .complete : .partial
    }

    /// Records what a fresh snapshot says about the task's history. The bounded
    /// journal replay that fills a partial history is added separately.
    func didLoad(
        snapshot: JetConversationSnapshot,
        planeRegistryID: UUID,
        headCursor: UInt64
    ) {
        let conversationID = snapshot.conversation.id
        switch histories[conversationID] {
        case .loading, .unavailable:
            return
        case .partial where observedStarts.contains(conversationID):
            // Entries were dropped at the cap; the start alone doesn't make it complete.
            return
        case .notStarted where !snapshot.runs.isEmpty:
            // The cache knew this task before its first Run and has followed every
            // Event since; a cursor expiry or an eviction would have cleared this state.
            observedStarts.insert(conversationID)
            histories[conversationID] = .complete
            return
        default:
            break
        }
        if snapshot.runs.isEmpty {
            histories[conversationID] = (transcripts[conversationID] ?? []).isEmpty
                ? .notStarted
                : .complete
        } else {
            histories[conversationID] = observedStarts.contains(conversationID)
                ? .complete
                : .partial
        }
    }

    /// Cancels a running history replay. The replay itself is added separately.
    func cancelReplay() {}

    func remove(_ conversationID: UUID) {
        transcripts.removeValue(forKey: conversationID)
        recency.removeAll { $0 == conversationID }
        observedStarts.remove(conversationID)
        histories.removeValue(forKey: conversationID)
    }

    /// Drops transcripts whose Event stream can no longer be trusted to be
    /// continuous, for example after the Plane cursor expired.
    func reset(conversationIDs: some Sequence<UUID>) {
        for conversationID in conversationIDs { remove(conversationID) }
    }

    private func touch(_ conversationID: UUID) {
        recency.removeAll { $0 == conversationID }
        recency.append(conversationID)
        while recency.count > Self.conversationCapacity {
            let evicted = recency.removeFirst()
            transcripts.removeValue(forKey: evicted)
            observedStarts.remove(evicted)
            histories.removeValue(forKey: evicted)
        }
    }

    // MARK: Merge rules

    /// Merges one projection into a transcript. Approval cards update in place,
    /// streamed user text appends to its Turn entry, everything else appends.
    /// Returns true when the entry cap dropped older entries.
    @discardableResult
    static func merge(_ entry: JetTimelineEntry, into timeline: inout [JetTimelineEntry]) -> Bool {
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
            return capEntries(&timeline)
        }
        return false
    }

    /// Folds an Event without a known projection into the trailing
    /// "background updates" entry. Returns true when the cap dropped older entries.
    @discardableResult
    static func groupRaw(sequence: UInt64, into timeline: inout [JetTimelineEntry]) -> Bool {
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
        return capEntries(&timeline)
    }

    private static func capEntries(_ timeline: inout [JetTimelineEntry]) -> Bool {
        guard timeline.count > entryCapacity else { return false }
        timeline.removeFirst(timeline.count - entryCapacity)
        return true
    }
}
