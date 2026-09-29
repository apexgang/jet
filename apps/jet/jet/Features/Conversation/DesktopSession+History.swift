import Foundation

/// The open task's transcript with its replayed history (design §6.5).
extension DesktopSession {
    /// Replayed history, then the live timeline; unique ids, the newest 256.
    /// Without an open task it's the live timeline.
    var transcriptEntries: [JetTimelineEntry] {
        historyPresentation?.entries ?? timeline
    }

    /// The history row above the transcript, if it needs one.
    var historyNotice: TranscriptStore.HistoryNotice? {
        historyPresentation?.notice
    }

    /// Try Again on "Couldn't load earlier messages."
    func retryEarlierMessages() {
        guard let selectedConversationID else { return }
        transcripts.retryReplay(for: selectedConversationID)
    }

    private var historyPresentation: TranscriptStore.HistoryPresentation? {
        guard let selectedConversationID else { return nil }
        return transcripts.presentation(for: selectedConversationID, live: timeline)
    }
}
