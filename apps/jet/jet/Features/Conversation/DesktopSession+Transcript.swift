import Foundation

/// The earlier-history row above the transcript. It mirrors WP13's
/// `TranscriptStore.HistoryNotice` case for case.
// WP6: once WP13 lands, replace this enum with
// `typealias TranscriptHistoryNotice = TranscriptStore.HistoryNotice`.
enum TranscriptHistoryNotice: Equatable, Sendable {
    /// Replaying earlier messages; nil progress is indeterminate.
    case loading(progress: Double?)
    /// Earlier messages can't be shown; `summary` reads "Started Sep 26 · …".
    case unavailable(summary: String?, canRetry: Bool)
    case noMessages

    var title: String {
        switch self {
        case .loading: String(localized: "Loading earlier messages…")
        case .unavailable(_, true): String(localized: "Couldn't load earlier messages.")
        case .unavailable: String(localized: "Earlier messages aren't available on this Mac.")
        case .noMessages: String(localized: "No Messages Yet")
        }
    }
}

// What the transcript reads from the session (design §6.5). Views call these
// instead of assembling session state themselves.
extension DesktopSession {
    /// The entries the transcript renders: replayed history, then the live timeline.
    var transcriptSourceEntries: [JetTimelineEntry] {
        // WP13: transcriptEntries
        timeline
    }

    /// The earlier-history row, or nil when the whole history is shown.
    var transcriptHistoryNotice: TranscriptHistoryNotice? {
        // WP13: historyNotice. Until the replay lands, this reads the cache's state.
        guard let conversationID = selectedConversationID,
              let snapshot = conversationSnapshot,
              snapshot.conversation.id == conversationID
        else { return nil }
        let isEmpty = transcriptSourceEntries.isEmpty
        if snapshot.runs.isEmpty { return isEmpty ? .noMessages : nil }
        switch transcripts.historyState(for: conversationID) {
        case let .loading(progress):
            return .loading(progress: progress)
        case .unavailable:
            return .unavailable(summary: interimHistorySummary(snapshot), canRetry: false)
        case .partial where isEmpty:
            return .unavailable(summary: interimHistorySummary(snapshot), canRetry: false)
        case .complete where isEmpty:
            return .noMessages
        case .partial, .complete, .notStarted:
            return nil
        }
    }

    /// Try Again on "Couldn't load earlier messages."
    func transcriptRetryEarlierMessages() {
        // WP13: retryEarlierMessages()
        Task { await loadSelectedConversation() }
    }

    func transcriptContext(showsTechnical: Bool) -> TranscriptContext {
        var queued: [String: QueuePlacement] = [:]
        for entry in transcriptSourceEntries where entry.kind == .user {
            if let placement = queuedEntry(forTimelineID: entry.id), placement.entry.state == .queued {
                queued[entry.id] = QueuePlacement(ordinal: placement.ordinal, entry: placement.entry)
            }
        }
        let lifecycle = selectedRun?.lifecycle
        return TranscriptContext(
            assistantName: selectedAssistantName,
            showsTechnical: showsTechnical,
            isReplyInProgress: canInterruptTurn || lifecycle == .starting || lifecycle == .stopping,
            // Cached execution facts, so the card stays while the computer is offline.
            needsPermission: runExecution?.activity == .waitingForApproval,
            queued: queued
        )
    }

    func transcriptRows(showsTechnical: Bool) -> [TranscriptRow] {
        TranscriptPresentation.rows(
            from: transcriptSourceEntries,
            context: transcriptContext(showsTechnical: showsTechnical)
        )
    }

    func transcriptTop(rowsAreEmpty: Bool) -> TranscriptTop {
        let hasSnapshot = conversationSnapshot.map { $0.conversation.id == selectedConversationID } ?? false
        return TranscriptPresentation.top(
            rowsAreEmpty: rowsAreEmpty,
            freshness: conversationFreshness,
            hasSnapshot: hasSnapshot
        )
    }

    /// The task whose Keep Changes progress closes the transcript.
    var transcriptConversationRef: ConversationRef? {
        selectedConversationRef
    }

    /// Try Again on "Couldn't Load This Task".
    func reloadSelectedTask() async {
        await loadSelectedConversation()
    }

    /// Send Again on a failed reply. It never replaces a draft: with text in the
    /// message box it only focuses it; otherwise it sends the earlier message again,
    /// or leaves it in the box when sending isn't possible right now.
    func sendAgain(_ text: String) async {
        guard draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            composerFocusRequest += 1
            return
        }
        draft = text
        if canSend {
            await submitDraft()
        } else {
            composerFocusRequest += 1
        }
    }

    private func interimHistorySummary(_ snapshot: JetConversationSnapshot) -> String {
        let started = TranscriptFormat.day(snapshot.conversation.createdAtUnixMilliseconds)
        guard let last = snapshot.runs.last, !last.lifecycle.isLive,
              let ended = snapshot.runs.compactMap(\.endedAtUnixMilliseconds).max()
        else {
            return String(localized: "Started \(started)")
        }
        return String(localized: "Started \(started) · Last run ended \(TranscriptFormat.day(ended))")
    }
}
