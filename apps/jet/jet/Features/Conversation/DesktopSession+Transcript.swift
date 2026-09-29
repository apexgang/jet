import Foundation

/// The earlier-history row above the transcript (WP13's replay).
typealias TranscriptHistoryNotice = TranscriptStore.HistoryNotice

// What the transcript reads from the session (design §6.5). Views call these
// instead of assembling session state themselves.
extension DesktopSession {
    /// The entries the transcript renders: replayed history, then the live timeline.
    var transcriptSourceEntries: [JetTimelineEntry] {
        transcriptEntries
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
}
