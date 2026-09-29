import Foundation
#if os(macOS)
import AppKit
#endif

// Details inspector state and actions (design §6.8). An extension only: it
// holds no stored state and nothing here opens the inspector.

/// The two segments of the Changes scope control.
enum ChangesSegment: Hashable, Sendable {
    case all
    case lastReply
}

/// What part of the task's changes Details shows, in the design's words. It maps
/// to and from the session's `WorkCheckpointKind` fields.
enum ChangesScopeChoice: Hashable, Sendable {
    case all
    case lastReply
    case reply(UInt32)
    case whenStopped
    case range(fromReply: UInt32, toReply: UInt32)

    /// The choice the checkpoint fields describe. A historical range starts after
    /// `fromTurn`, so Replies f+1…t.
    static func from(
        kind: WorkCheckpointKind,
        turn: UInt32,
        fromTurn: UInt32,
        toTurn: UInt32,
        latestTurn: UInt32
    ) -> ChangesScopeChoice {
        switch kind {
        case .current:
            return .all
        case .turn:
            return turn == latestTurn ? .lastReply : .reply(turn)
        case .final:
            return .whenStopped
        case .historical:
            return .range(fromReply: fromTurn + 1, toReply: toTurn)
        }
    }

    /// The selected segment, or nil when the scope comes from Earlier.
    var segment: ChangesSegment? {
        switch self {
        case .all: .all
        case .lastReply: .lastReply
        case .reply, .whenStopped, .range: nil
        }
    }

    /// "Reply 2", "Replies 2–3" or "When Claude Code Stopped".
    func title(assistant: String?) -> String {
        switch self {
        case .all:
            return String(localized: "All Changes")
        case .lastReply:
            return String(localized: "Last Reply")
        case let .reply(reply):
            return String(localized: "Reply \(reply)")
        case let .range(from, to):
            return from == to
                ? String(localized: "Reply \(to)")
                : String(localized: "Replies \(from)–\(to)")
        case .whenStopped:
            return String(localized: "When \(DetailsCopy.assistant(assistant, position: .title)) Stopped")
        }
    }

    /// The checkpoint fields for this choice, or nil when it isn't valid for the
    /// task: replies need 1…latest, a range needs 1 ≤ from ≤ to ≤ latest, and
    /// When Stopped needs the assistant to have stopped.
    func checkpoint(latestTurn: UInt32, runIsLive: Bool) -> DetailsCheckpoint? {
        switch self {
        case .all:
            return DetailsCheckpoint(kind: .current)
        case .lastReply:
            guard latestTurn >= 1 else { return nil }
            return DetailsCheckpoint(kind: .turn, turn: latestTurn)
        case let .reply(reply):
            guard reply >= 1, reply <= latestTurn else { return nil }
            return DetailsCheckpoint(kind: .turn, turn: reply)
        case .whenStopped:
            guard !runIsLive else { return nil }
            return DetailsCheckpoint(kind: .final)
        case let .range(from, to):
            guard from >= 1, from <= to, to <= latestTurn else { return nil }
            return DetailsCheckpoint(kind: .historical, fromTurn: from - 1, toTurn: to)
        }
    }
}

/// The session's checkpoint fields for one scope choice.
struct DetailsCheckpoint: Equatable, Sendable {
    var kind: WorkCheckpointKind
    var turn: UInt32 = 1
    var fromTurn: UInt32 = 0
    var toTurn: UInt32 = 1
}

/// A waiting message as Activity › Waiting to Send lists it.
struct DetailsQueuedMessage: Identifiable, Equatable {
    let entry: JetTurnQueueEntry
    let ordinal: String
    let text: String

    var id: UUID { entry.id }
}

/// The one line at the bottom of Details: a confirmation, a warning or an error.
struct DetailsNotice: Equatable {
    enum Kind: Equatable {
        case confirmation
        case warning
        case error
    }

    let kind: Kind
    let text: String
    let error: JetPresentationError?
}

/// Which request a failure belongs to, for its wording.
enum DetailsFailedAction: Equatable, Sendable {
    case save
    case comment
    case other
}

extension DesktopSession {
    // MARK: - Tabs and edit mode

    /// The Details segment. Edit mode (`files`) is part of Changes.
    var detailsTab: WorkPanelTab {
        selectedWorkPanel == .files ? .changes : selectedWorkPanel
    }

    /// Switches segments. Leaving edit mode asks first when an edit is unsaved;
    /// `reloadChanges` refreshes Changes afterwards (a save happened while editing).
    func selectDetailsTab(_ tab: WorkPanelTab, reloadChanges: Bool = false) {
        let target: WorkPanelTab = tab == .files ? .changes : tab
        guard target != detailsTab else { return }
        guard selectedWorkPanel == .files else {
            selectedWorkPanel = target
            return
        }
        let revision = editableFile?.revision
        guardUnsavedEdits { [weak self] in
            guard let self else { return }
            let savedInAlert = self.editableFile != nil && self.editableFile?.revision != revision
            self.leaveEditMode(to: target, reload: reloadChanges || savedInAlert)
        }
    }

    /// Changes shows a file editor instead of the list.
    var isEditingFile: Bool {
        selectedWorkPanel == .files
            && selectedWorkFilePath != nil
            && (editableFile != nil || workOperation == "file")
    }

    /// Opens a changed file for editing, after the unsaved-edit check.
    func beginEditingFile(_ path: String) async {
        if hasUnsavedFileEdit {
            guardUnsavedEdits { [weak self] in
                Task { await self?.selectWorkFile(path) }
            }
        } else {
            await selectWorkFile(path)
        }
    }

    /// Done: back to the list, after the unsaved-edit check. `reload` refreshes the
    /// changes when the file was saved while editing.
    func finishEditingFile(reload: Bool) {
        let revision = editableFile?.revision
        guardUnsavedEdits { [weak self] in
            guard let self else { return }
            let savedInAlert = self.editableFile != nil && self.editableFile?.revision != revision
            self.leaveEditMode(to: .changes, reload: reload || savedInAlert)
        }
    }

    private func leaveEditMode(to tab: WorkPanelTab, reload: Bool) {
        selectedWorkPanel = tab
        editableFile = nil
        fileDraft = ""
        if reload { Task { await self.loadWorkPanel() } }
    }

    /// Discards the unsaved edit.
    func revertFileEdits() {
        fileDraft = editableFile?.content ?? ""
    }

    /// Saves the open file against the revision it was opened at. Returns true once
    /// the save is confirmed.
    @discardableResult
    func saveEditedFile() async -> Bool {
        guard let file = editableFile else { return false }
        await saveSelectedWorkFile()
        guard workNoticeError == nil,
              let saved = editableFile,
              saved.path == file.path,
              saved.revision != file.revision
        else { return false }
        let name = Self.detailsFileName(file.path)
        workNotice = String(localized: "Saved \(name).")
        return true
    }

    /// Sends the line comment as a message. The draft and its retained Command ID
    /// stay in the session, so a failed or uncertain send can be sent again as the
    /// same Command.
    func sendLineComment(path: String) async -> Bool {
        selectedWorkFilePath = path
        await submitSelectedReview()
        let sent = workNoticeError == nil && reviewComment.isEmpty
        if sent {
            let assistant = DetailsCopy.assistant(selectedAssistantName, position: .mid)
            workNotice = String(localized: "Comment sent to \(assistant).")
        }
        return sent
    }

    static func detailsFileName(_ path: String) -> String {
        let name = URL(fileURLWithPath: path).lastPathComponent
        return name.isEmpty ? path : name
    }

    // MARK: - Changes scope

    var changesScopeChoice: ChangesScopeChoice {
        .from(
            kind: checkpointKind,
            turn: checkpointTurn,
            fromTurn: checkpointFromTurn,
            toTurn: checkpointToTurn,
            latestTurn: latestReplyNumber
        )
    }

    /// The newest finished reply of the open Run.
    var latestReplyNumber: UInt32 { workDiff?.latestTurn ?? 0 }

    /// Changes cover only the latest Run, so a task that started more than once
    /// says so.
    var changesSpanSeveralRuns: Bool { (detailsSnapshot?.runs.count ?? 0) > 1 }

    /// Every byte of the change patch is loaded.
    var isWorkPatchComplete: Bool {
        guard let workDiff else { return true }
        return !workDiff.patchTruncated || workPatchBytesLoaded >= workDiff.artifact.size
    }

    /// Applies a scope choice at once. Invalid choices do nothing.
    func selectChangesScope(_ choice: ChangesScopeChoice) async {
        guard let run = selectedRun,
              let checkpoint = choice.checkpoint(
                latestTurn: latestReplyNumber,
                runIsLive: run.lifecycle.isLive
              )
        else { return }
        checkpointKind = checkpoint.kind
        checkpointTurn = checkpoint.turn
        checkpointFromTurn = checkpoint.fromTurn
        checkpointToTurn = checkpoint.toTurn
        selectedWorkFilePath = nil
        editableFile = nil
        fileDraft = ""
        // Keeps the choice even when the load starts before the Run's first diff.
        keepsRequestedCheckpoint = true
        await loadWorkPanel(preserveContinuity: false)
    }

    /// When a reply finished its changes, from the transcript's change entries of
    /// the open Run.
    func replyRecordedAt(_ reply: UInt32) -> Int64? {
        guard let runID = selectedRun?.id else { return nil }
        return timeline.last {
            $0.checkpointTurn == reply && $0.runID == runID && $0.recordedAtUnixMilliseconds != nil
        }?.recordedAtUnixMilliseconds
    }

    // MARK: - Task context

    /// The open task's snapshot, once it matches the selection.
    var detailsSnapshot: JetConversationSnapshot? {
        guard let snapshot = conversationSnapshot,
              snapshot.conversation.id == selectedConversationID
        else { return nil }
        return snapshot
    }

    /// Details can't reach the task's computer, so it shows the saved view.
    var detailsIsOffline: Bool {
        selectedTaskStatus == .offline || (usesLivePlane && !planeIsConnected)
    }

    var detailsConversationRef: ConversationRef? {
        selectedConversationID.map {
            ConversationRef(conversationID: $0, planeRegistryID: selectedPlaneRegistryID)
        }
    }

    /// The task's project name, when its computer reported it.
    var detailsProjectName: String? {
        guard let conversationID = selectedConversationID else { return nil }
        if let name = projectName(for: conversationID) { return name }
        guard let projectID = selectedConversation?.projectID else { return nil }
        return selectedSetupSnapshot?.projects.projects.first { $0.id == projectID }?.name
    }

    private var detailsProjectRoot: String? {
        guard let projectID = selectedConversation?.projectID else { return nil }
        return selectedSetupSnapshot?.projects.projects.first { $0.id == projectID }?.root
    }

    /// The folder the assistant works in: the separate working copy, or the project
    /// folder for tasks that work there directly.
    var detailsWorkingCopyPath: String? {
        detailsSnapshot?.workspaceRoot ?? detailsProjectRoot
    }

    /// The task has a Run that works directly in the project folder.
    var detailsWorksInProjectFolder: Bool {
        guard selectedRun != nil else { return false }
        if let snapshot = detailsSnapshot {
            return snapshot.workspaceID == nil && workDiff?.workspaceID == nil
        }
        return workDiff != nil && workDiff?.workspaceID == nil
    }

    var canRevealDetailsFolder: Bool {
#if os(macOS)
        return detailsWorkingCopyPath != nil && isLocalPlane(selectedPlaneRegistryID)
#else
        return false
#endif
    }

    func revealDetailsFolder() {
#if os(macOS)
        guard canRevealDetailsFolder, let path = detailsWorkingCopyPath else { return }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path, isDirectory: true)])
#endif
    }

    func copyDetailsFolderPath() {
        guard let path = detailsWorkingCopyPath else { return }
        Self.copyToPasteboard(path)
    }

    func copyTaskID() {
        guard let conversationID = selectedConversationID else { return }
        Self.copyToPasteboard(conversationID.uuidString.lowercased())
    }

    // MARK: - Changed files

    func canEditChangedFile(_ file: JetChangedFile) -> Bool {
        file.status != "deleted" && file.contentAvailable && !detailsIsOffline
    }

    /// The file on this Mac, for Show in Finder. Deleted files and other computers
    /// have none.
    func changedFileURL(_ file: JetChangedFile) -> URL? {
        guard file.status != "deleted",
              isLocalPlane(selectedPlaneRegistryID),
              let root = detailsWorkingCopyPath
        else { return nil }
        return URL(fileURLWithPath: root, isDirectory: true).appending(path: file.path)
    }

    func revealChangedFile(_ file: JetChangedFile) {
#if os(macOS)
        guard let url = changedFileURL(file) else { return }
        NSWorkspace.shared.activateFileViewerSelecting([url])
#endif
    }

    func copyChangedFilePath(_ file: JetChangedFile) {
        Self.copyToPasteboard(file.path)
    }

    // MARK: - Terminals

    /// "Terminal 1", "Terminal 2 (Closed)", in list order.
    var terminalTitles: [UUID: String] {
        var titles: [UUID: String] = [:]
        for (index, terminal) in workTerminals.enumerated() {
            let number = index + 1
            switch terminal.state {
            case .closed, .unavailable:
                titles[terminal.id] = String(localized: "Terminal \(number) (Closed)")
            case .opening, .open, .closing:
                titles[terminal.id] = String(localized: "Terminal \(number)")
            }
        }
        return titles
    }

    var selectedDetailsTerminal: JetWorkspaceTerminal? {
        guard let selectedTerminalID else { return nil }
        return workTerminals.first { $0.id == selectedTerminalID }
    }

    /// Shows a terminal and attaches to it when it is open and reachable.
    func selectTerminal(_ id: UUID) async {
        selectedTerminalID = id
        guard attachedTerminalID != id,
              workTerminals.contains(where: { $0.id == id && $0.state == .open }),
              !detailsIsOffline
        else { return }
        await attachSelectedTerminal()
    }

    /// Closes a terminal after its confirmation. The session reports "Terminal
    /// closed." only once the computer confirms it.
    func closeTerminal(_ id: UUID) async {
        selectedTerminalID = id
        await closeSelectedTerminal()
    }

    // MARK: - Waiting messages

    /// Messages waiting to send, in order, with their text from the transcript.
    var queuedMessages: [DetailsQueuedMessage] {
        (turnQueue?.turns ?? [])
            .filter { $0.state == .queued }
            .sorted { $0.position < $1.position }
            .enumerated()
            .map { index, entry in
                DetailsQueuedMessage(
                    entry: entry,
                    ordinal: JetCopy.ordinal(index + 1),
                    text: queuedText(for: entry) ?? DetailsCopy.queuedFallback(entry.source)
                )
            }
    }

    // MARK: - Notice

    /// The bottom line of Details, from the last work notice or failure.
    var detailsNotice: DetailsNotice? {
        if let error = workNoticeError {
            // A cancelled request isn't a failure; whoever cancelled it moved on.
            guard error.category != .cancelled else { return nil }
            let text = DetailsCopy.error(
                error,
                computer: selectedPlaneName,
                assistant: selectedAssistantName,
                action: detailsFailedAction,
                fallback: workNotice
            )
            let kind: DetailsNotice.Kind = switch error.category {
            case .offline, .conflict, .outcomeUnknown: .warning
            default: .error
            }
            return DetailsNotice(kind: kind, text: text, error: error)
        }
        guard let workNotice, !workNotice.isEmpty else { return nil }
        return DetailsNotice(kind: .confirmation, text: workNotice, error: nil)
    }

    /// The request an uncertain failure belongs to: the one that kept its Command.
    private var detailsFailedAction: DetailsFailedAction {
        if pendingReview != nil { return .comment }
        if pendingWorkEdit != nil { return .save }
        return .other
    }
}

extension JetRecoveryAction {
    /// The recovery button's title in Details.
    var detailsTitle: String {
        switch self {
        case .refreshFile: String(localized: "Reload File")
        case .refreshConversation: String(localized: "Refresh Task")
        case .refreshRun: String(localized: "Refresh")
        case .resumeEvents: String(localized: "Reconnect")
        }
    }
}

/// Details copy (design §4 lexicon). Casual strings never need Jet domain words.
enum DetailsCopy {
    enum Position {
        /// Inside a sentence: "the assistant".
        case mid
        /// At the start of a sentence: "The assistant".
        case start
        /// In a title: "the Assistant".
        case title
    }

    /// The assistant's product name, or a generic fallback for its position.
    static func assistant(_ name: String?, position: Position) -> String {
        if let name, !name.isEmpty { return name }
        switch position {
        case .mid: return String(localized: "the assistant")
        case .start: return String(localized: "The assistant")
        case .title: return String(localized: "the Assistant")
        }
    }

    /// A waiting message whose text isn't in this app's transcript.
    static func queuedFallback(_ source: JetTurnSource) -> String {
        switch source {
        case .schedule: String(localized: "A scheduled message")
        case .autoContinue: String(localized: "An automatic follow-up")
        case .user: String(localized: "Message from another Jet app")
        }
    }

    /// A failure in casual words. Codes and categories choose the sentence; native
    /// error text is never parsed. `fallback` is the session's own plain message.
    static func error(
        _ error: JetPresentationError,
        computer: String,
        assistant: String?,
        action: DetailsFailedAction,
        fallback: String? = nil
    ) -> String {
        if error.code == "user_edit.stale_revision" {
            return String(localized: "This file changed since you opened it. Reload it to get the latest version.")
        }
        if error.code == "checkpoint.run_active" {
            return String(localized: "Available after \(DetailsCopy.assistant(assistant, position: .mid)) stops.")
        }
        switch error.category {
        case .offline:
            return String(localized: "Jet can't reach \(computer) right now.")
        case .outcomeUnknown where action == .save:
            return String(localized: "Jet couldn't confirm the save. Click Save to check; Jet sends the same request.")
        case .outcomeUnknown where action == .comment:
            return String(localized: "Jet couldn't confirm the comment was sent. Send it again to check; it won't be sent twice.")
        case .notFound:
            return String(localized: "These changes are no longer available.")
        default:
            if let fallback, !fallback.isEmpty { return fallback }
            return error.message
        }
    }

    /// Every fixed casual string in Details, with sample names for the templates,
    /// for the Avoid-word lint. Includes the session's work notices shown here.
    static var casualStrings: [String] {
        let assistant = "Claude Code"
        let project = "web-app"
        let computer = "This Mac"
        let file = "login.ts"
        var strings: [String] = [
            // Container
            String(localized: "Details"), String(localized: "Changes"), String(localized: "Terminal"),
            String(localized: "Activity"), String(localized: "No Task Selected"),
            String(localized: "Details appear once a task starts."),
            // Scope
            String(localized: "All Changes"), String(localized: "Last Reply"), String(localized: "Earlier"),
            String(localized: "Reply \(UInt32(2))"), String(localized: "Replies \(UInt32(2))–\(UInt32(3))"),
            ChangesScopeChoice.whenStopped.title(assistant: assistant),
            ChangesScopeChoice.whenStopped.title(assistant: nil),
            String(localized: "Custom Range…"), String(localized: "No finished replies yet"),
            String(localized: "From Reply \(UInt32(1))"), String(localized: "To Reply \(UInt32(3))"),
            String(localized: "Show Changes"), String(localized: "Cancel"),
            String(localized: "No Finished Replies Yet"),
            String(localized: "Changes appear here after \(assistant) finishes a reply."),
            // Captions and summary
            String(localized: "Showing changes since \(assistant) last started for this task."),
            String(localized: "Some very large files are listed without their changes."),
            String(localized: "Offline · showing saved view"),
            String(localized: "Couldn't refresh changes."), String(localized: "Try Again"),
            String(localized: "at least"), String(localized: "Show More Files"),
            // File rows
            String(localized: "Added"), String(localized: "Modified"), String(localized: "Deleted"),
            String(localized: "changed by \(assistant)"), String(localized: "changed by you"),
            String(localized: "changed in a terminal"), String(localized: "changed by more than one source"),
            String(localized: "changed outside Jet"), String(localized: "Expanded"), String(localized: "Collapsed"),
            String(localized: "Edit File"), String(localized: "Comment on Line…"), String(localized: "Copy Path"),
            String(localized: "Show in Finder"), String(localized: "File Actions"),
            String(localized: "Comment on Line \(UInt32(12))…"), String(localized: "Changed Files"),
            // States
            String(localized: "No Changes Yet"), String(localized: "Changes \(assistant) makes appear here."),
            String(localized: "Loading changes…"), String(localized: "Couldn't Load Changes"),
            String(localized: "No Changes"), String(localized: "No files changed in this part of the task."),
            // Diff
            String(localized: "Binary file changed"), String(localized: "This file's changes are cut off."),
            String(localized: "Load More"), String(localized: "This file is too large to show."),
            String(localized: "No text changes to show."), String(localized: "Show \(1_200) More Lines"),
            String(localized: "Lines \(10)–\(18)"), String(localized: "Line \(10)"),
            String(localized: "Removed at line \(10)"),
            String(localized: "Added line \(12): \("return")"), String(localized: "Removed line \(10): \("return")"),
            String(localized: "Line \(11): \("return")"),
            // Comment
            String(localized: "Comment on \(file)"), String(localized: "Line"), String(localized: "Comment"),
            String(localized: "\(assistant) gets this as a message."),
            String(localized: "\(7_000) of \(8_192) bytes"), String(localized: "Send Comment"),
            // Keep
            String(localized: "Keep Changes…"), String(localized: "Review the changes and choose how to keep them."),
            // Edit mode
            String(localized: "\(file) — Edited"), String(localized: "Done"),
            String(localized: "You're editing this file in your \(project) folder."),
            String(localized: "You're editing this file in your project folder."),
            String(localized: "You're editing the working copy. Your \(project) folder doesn't change until you keep the changes."),
            String(localized: "You're editing the working copy. Your project folder doesn't change until you keep the changes."),
            String(localized: "Contents of \(file)"), String(localized: "Opening \(file)…"),
            String(localized: "Can't Edit This File"), String(localized: "Jet can edit text files up to 128 KB."),
            String(localized: "Revert…"), String(localized: "Discard your edits to \(file)?"),
            String(localized: "This can't be undone."), String(localized: "Discard Edits"),
            String(localized: "Save"), String(localized: "Saving…"), String(localized: "Reconnect to save."),
            String(localized: "Saved \(file)."), String(localized: "Comment sent to \(assistant)."),
            // Terminal
            String(localized: "New Terminal"), String(localized: "Terminal \(1)"), String(localized: "Terminal \(2) (Closed)"),
            String(localized: "Attach to see live output."), String(localized: "This terminal is closed."),
            String(localized: "Type a command"), String(localized: "Send"), String(localized: "Attach"),
            String(localized: "Detach"), String(localized: "Stop showing live output. The terminal keeps running."),
            String(localized: "Close Terminal…"), String(localized: "Close Terminal"),
            String(localized: "Close \("Terminal 1")?"), String(localized: "Commands running in it stop."),
            String(localized: "No Terminal Yet"),
            String(localized: "Terminals open in this task's working copy after \(assistant) starts."),
            String(localized: "Terminal Unavailable"),
            String(localized: "This task works directly in your \(project) folder. Use Terminal on your Mac instead."),
            String(localized: "This task works directly in your project folder. Use Terminal on your Mac instead."),
            String(localized: "No Terminals"), String(localized: "Terminals open in this task's working copy."),
            String(localized: "Terminal closed."),
            // Activity
            String(localized: "Now"), String(localized: "Started \("12 min. ago")"), String(localized: "Ended \("12 min. ago")"),
            String(localized: "Interrupt…"), String(localized: "Stop Assistant…"),
            String(localized: "\(assistant) hasn't started on this task yet."),
            String(localized: "Send a message to continue."), String(localized: "Waiting to Send"),
            String(localized: "Remove"), String(localized: "Working Copy"), String(localized: "Location"),
            String(localized: "\(assistant) works in this separate copy. Your \(project) folder doesn't change until you keep the changes."),
            String(localized: "\(assistant) works in this separate copy. Your project folder doesn't change until you keep the changes."),
            String(localized: "\(assistant) works directly in your \(project) folder."),
            String(localized: "\(assistant) works directly in your project folder."),
            String(localized: "Jet creates a working copy when \(assistant) starts."),
            String(localized: "Technical Details"), String(localized: "Copy Task ID"),
            // Recovery
            String(localized: "Reload File"), String(localized: "Refresh Task"), String(localized: "Refresh"),
            String(localized: "Reconnect"),
            // The session's work notices shown in Details (WP1 copy)
            String(localized: "Saved."), String(localized: "Comment sent as a message."),
            String(localized: "Terminal session ended."), String(localized: "All changes loaded."),
            String(localized: "Activity reconnected."),
            String(localized: "Changes loaded, but terminals aren't available right now."),
        ]
        for source in [JetTurnSource.user, .schedule, .autoContinue] {
            strings.append(queuedFallback(source))
        }
        for position in [Position.mid, .start, .title] {
            strings.append(Self.assistant(nil, position: position))
        }
        let sampleError = JetPresentationError(category: .unavailable, code: "sample", message: "", retryable: false)
        for (category, code, action) in [
            (JetPresentationErrorCategory.offline, "transport.offline", DetailsFailedAction.other),
            (.conflict, "user_edit.stale_revision", .save),
            (.outcomeUnknown, "command.outcome_unknown", .save),
            (.outcomeUnknown, "command.outcome_unknown", .comment),
            (.conflict, "checkpoint.run_active", .other),
            (.notFound, "checkpoint.not_found", .other),
        ] {
            let error = JetPresentationError(
                category: category, code: code, message: sampleError.message, retryable: false
            )
            strings.append(Self.error(error, computer: computer, assistant: assistant, action: action))
        }
        for reason in [JetArtifactAvailability.diskPressure, .runBudgetExceeded, .artifactSizeExceeded] {
            if let text = DiffSplit.cutOffReason(reason) { strings.append(text) }
        }
        return strings
    }
}
