#if DEBUG
import SwiftUI

// WP13: DesktopPreviewScenes.all is frozen, so these scenes aren't listed yet.
// The lead adds `+ history` to `DesktopPreviewScenes.all` (or appends `history`
// to WP6's `transcript` group); the ids must stay unique across groups.
extension DesktopPreviewScenes {
    /// Transcript history (WP13): the replay's loading row, the summary card,
    /// replayed history before live entries, Try Again, a merged permission and
    /// a task with no messages.
    @MainActor static var history: [DesktopPreviewScene] {
        [
            historyScene("transcript-history-loading") { session in
                HistoryPreviewData.showLive(
                    Array(DesktopPreviewData.loginTranscript(finished: true).suffix(3)),
                    in: session
                )
                HistoryPreviewData.seed(session, phase: .running(progress: 0.4))
            },
            historyScene("transcript-history-summary") { session in
                HistoryPreviewData.showLive([], in: session)
                HistoryPreviewData.seed(session, phase: .finished(.beyondLimit))
            },
            historyScene("transcript-history-partial") { session in
                HistoryPreviewData.showLive(HistoryPreviewData.liveAfterFence, in: session)
                HistoryPreviewData.seed(
                    session,
                    phase: .finished(.journalTrimmed),
                    entries: HistoryPreviewData.earlierMessages,
                    floor: 1_000,
                    fence: 1_010
                )
            },
            historyScene("transcript-history-retry") { session in
                HistoryPreviewData.showLive([], in: session)
                HistoryPreviewData.seed(session, phase: .finished(.unreachable))
            },
            historyScene("transcript-history-complete") { session in
                HistoryPreviewData.showLive(HistoryPreviewData.permissionLive, in: session)
                HistoryPreviewData.seed(
                    session,
                    phase: .finished(nil),
                    entries: HistoryPreviewData.permissionHistory,
                    floor: 1_000,
                    fence: 1_010
                )
            },
            historyScene("transcript-no-messages") { session in
                HistoryPreviewData.showTaskWithoutRuns(session)
            },
            historyScene(
                "transcript-history-summary-large",
                size: CGSize(width: 900, height: 600),
                textScale: 2
            ) { session in
                HistoryPreviewData.showLive([], in: session)
                HistoryPreviewData.seed(session, phase: .finished(.beyondLimit))
            },
        ]
    }

    /// The main window on the login-redirect task, then `seed`.
    @MainActor private static func historyScene(
        _ id: String,
        size: CGSize = CGSize(width: 1280, height: 800),
        textScale: Double? = nil,
        seed: @escaping @MainActor (DesktopSession) -> Void
    ) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id, size: size) {
            let content = ContentView(session: .preview { session in
                DesktopPreviewData.waitingWithChanges(session)
                seed(session)
            })
            guard let textScale else { return AnyView(content) }
            let suiteName = "jet.preview.history-text"
            let defaults = UserDefaults(suiteName: suiteName) ?? .standard
            defaults.set(textScale, forKey: JetDesign.transcriptScaleKey)
            return AnyView(
                content
                    .defaultAppStorage(defaults)
                    .environment(\.transcriptScale, CGFloat(textScale))
            )
        }
    }
}

/// History for the WP13 scenes. The task started Sep 26 and its last Run ended Sep 27.
@MainActor
enum HistoryPreviewData {
    static let createdAt: Int64 = 1_790_424_000_000
    static let lastRunEndedAt: Int64 = 1_790_510_400_000
    static let earlierRun = DesktopPreviewData.runID(9)

    static func facts(_ session: DesktopSession, hasRuns: Bool = true) -> TranscriptStore.HistoryFacts {
        TranscriptStore.HistoryFacts(
            planeRegistryID: session.localPlaneRegistryID,
            createdAtUnixMilliseconds: createdAt,
            lastRunEndedAtUnixMilliseconds: hasRuns ? lastRunEndedAt : nil,
            hasRuns: hasRuns,
            fence: 1_040
        )
    }

    /// Seeds the open task's history as if this Mac joined it late.
    static func seed(
        _ session: DesktopSession,
        phase: TranscriptStore.ReplayPhase,
        entries: [JetTimelineEntry] = [],
        floor: UInt64 = 0,
        fence: UInt64 = 0
    ) {
        guard let task = session.selectedConversationID else { return }
        session.transcripts.seedPreviewHistory(
            task,
            facts: facts(session),
            phase: phase,
            entries: entries,
            floor: floor,
            fence: fence
        )
    }

    /// Replaces the open task's live transcript.
    static func showLive(_ entries: [JetTimelineEntry], in session: DesktopSession) {
        guard let task = session.selectedConversationID else { return }
        session.timeline = entries
        session.transcripts.save(entries, for: task)
    }

    /// "Add CSV export to reports", before its first message.
    static func showTaskWithoutRuns(_ session: DesktopSession) {
        let task = DesktopPreviewData.conversations[5]
        let snapshot = JetConversationSnapshot(
            cursor: DesktopPreviewData.headCursor,
            conversation: task,
            workspaceID: nil,
            workspaceRoot: nil,
            runs: []
        )
        session.sidebarSelection = .conversation
        session.selectedConversationID = task.id
        session.conversationSnapshot = snapshot
        session.runExecution = nil
        session.turnQueue = JetTurnQueue(cursor: DesktopPreviewData.headCursor, turns: [])
        session.resetWorkPanel()
        showLive([], in: session)
        session.transcripts.seedPreviewHistory(
            task.id,
            facts: facts(session, hasRuns: false),
            phase: .idle
        )
    }

    /// An earlier reply on Sep 26, replayed from the journal: sequences 1001–1010.
    static var earlierMessages: [JetTimelineEntry] {
        [
            entry("7a3c0000-0000-4000-8000-000000000611", .user, "Sign-in feels slow on staging. Can you find out why?", sequence: 1_002, minutes: 0),
            entry("1003-output-0", .agent, "`readSession` decodes the session cookie twice on every request. I'll decode it once and keep the result for the rest of the request.", sequence: 1_003, minutes: 1),
            entry("1005-output-0", .agent, "Read", sequence: 1_005, minutes: 2),
            entry("1006-output-0", .agent, "Edit", sequence: 1_006, minutes: 3),
            JetTimelineEntry(
                id: "1008-result",
                kind: .result,
                text: "Jet recorded the completed turn and its changes.",
                sequence: 1_008,
                rawCount: 0,
                recordedAtUnixMilliseconds: createdAt + 4 * DesktopPreviewData.minute,
                runID: earlierRun,
                checkpointTurn: 1
            ),
        ]
    }

    /// Today's login-redirect message, its steps, changes and reply, after the fence.
    static var liveAfterFence: [JetTimelineEntry] {
        let kept: Set<String> = ["u1", "t1", "t3", "313-result", "a2"]
        return DesktopPreviewData.loginTranscript(finished: true)
            .filter { kept.contains($0.id) }
            .enumerated()
            .map { index, entry in
                var entry = entry
                entry.sequence = 1_020 + UInt64(index)
                return entry
            }
    }

    /// A permission asked for before the fence and decided after it.
    static var permissionHistory: [JetTimelineEntry] {
        [
            entry("7a3c0000-0000-4000-8000-000000000612", .user, "Run the whole test suite before you finish.", sequence: 1_002, minutes: 0),
            entry("1003-output-0", .agent, "I'll run `npm test` in the working copy.", sequence: 1_003, minutes: 1),
            permission(.requested, sequence: 1_005, minutes: 2),
        ]
    }

    static var permissionLive: [JetTimelineEntry] {
        [
            permission(.allowed, sequence: 1_030, minutes: 3),
            entry("1031-output-0", .agent, "Bash", sequence: 1_031, minutes: 4),
            entry("1032-output-0", .agent, "All **42 tests** pass.", sequence: 1_032, minutes: 5),
        ]
    }

    private static func permission(
        _ state: JetApprovalState,
        sequence: UInt64,
        minutes: Int64
    ) -> JetTimelineEntry {
        let approval = JetApprovalPresentation(
            requestID: "req-7",
            reviewID: nil,
            runID: earlierRun,
            tool: "Bash",
            action: #"{"command":"npm test","cwd":"/Users/alex/.jet/workspaces/web-app-7f3a"}"#,
            target: DesktopPreviewData.workingCopyRoot,
            scope: "This action once",
            consequence: state == .allowed
                ? "The reviewer allowed this exact action once."
                : "The Run stays paused until this request is decided. Manual approval decisions are not available through the current client protocol.",
            rationale: nil,
            state: state,
            canAuthorizeRetry: false
        )
        return JetTimelineEntry(
            id: "approval-\(earlierRun.uuidString.lowercased())-req-7",
            kind: .approval,
            text: "Bash needs approval.",
            sequence: sequence,
            rawCount: 0,
            approval: approval,
            recordedAtUnixMilliseconds: createdAt + minutes * DesktopPreviewData.minute,
            runID: earlierRun
        )
    }

    private static func entry(
        _ id: String,
        _ kind: JetTimelineKind,
        _ text: String,
        sequence: UInt64,
        minutes: Int64
    ) -> JetTimelineEntry {
        JetTimelineEntry(
            id: id,
            kind: kind,
            text: text,
            sequence: sequence,
            rawCount: 0,
            recordedAtUnixMilliseconds: createdAt + minutes * DesktopPreviewData.minute,
            runID: earlierRun
        )
    }
}
#endif
