import Foundation
#if os(macOS)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

// The API views use to read and change the shell. Frozen after wave 1: later
// work packages add their own DesktopSession+*.swift files instead of editing it.
extension DesktopSession {
    // MARK: - Navigation

    /// The sidebar's selection. Setting it asks first when a file edit is unsaved.
    var sidebarItem: SidebarItem? {
        get {
            switch sidebarSelection {
            case .newTask: .newTask
            case .project: selectedProjectID.map(SidebarItem.project)
            case .trash: .trash
            case .conversation, .search, .needsAttention, .schedules, .planes:
                selectedConversationID.map(SidebarItem.task)
            }
        }
        set {
            guard let newValue, newValue != sidebarItem else { return }
            guardUnsavedEdits { [weak self] in self?.open(newValue) }
        }
    }

    /// Opens a sidebar destination without the unsaved-edit check.
    func open(_ item: SidebarItem) {
        switch item {
        case .newTask:
            beginNewTask()
        case let .task(conversationID):
            selectConversation(conversationID)
        case let .project(projectID):
            selectProject(projectID, on: planeRegistryID(forProject: projectID) ?? localPlaneRegistryID)
        case .trash:
            leaveConversation()
            sidebarSelection = .trash
            isWorkPanelPresented = false
            composerNotice = nil
        }
    }

    func showTrash() {
        sidebarItem = .trash
    }

    /// Reveals the sidebar and focuses Search Tasks (⌘K).
    func findTask() {
        revealSidebarRequest += 1
        searchFocusRequest += 1
    }

    var selectedConversationRef: ConversationRef? {
        selectedConversationID.map {
            ConversationRef(conversationID: $0, planeRegistryID: selectedPlaneRegistryID)
        }
    }

    // MARK: - Computers and projects

    func planeRegistryID(for conversationID: UUID) -> UUID? {
        conversationPlaneRegistryIDs[conversationID]
    }

    func planeRegistryID(forProject projectID: UUID) -> UUID? {
        allProjects.first { $0.project.id == projectID }?.planeRegistryID
    }

    func isLocalPlane(_ planeRegistryID: UUID) -> Bool {
        planeRegistryID == localPlaneRegistryID
    }

    func isPlaneConnected(_ planeRegistryID: UUID) -> Bool {
        if planeRegistryID == localPlaneRegistryID {
            if case .connected = connectionState { return true }
            return false
        }
        if case .connected = planes.first(where: { $0.id == planeRegistryID })?.connection {
            return true
        }
        return false
    }

    /// A computer is offline when its connection failed, or when it is
    /// disconnected or reconnecting after a recorded failure. Connecting and
    /// reconnecting blips alone never make its tasks read Offline.
    func isComputerOffline(_ planeRegistryID: UUID) -> Bool {
        guard let plane = planes.first(where: { $0.id == planeRegistryID }) else { return true }
        let connection = planeRegistryID == localPlaneRegistryID ? connectionState : plane.connection
        switch connection {
        case .connected, .connecting: return false
        case .failed: return true
        case .disconnected, .reconnecting: return plane.failure != nil
        }
    }

    /// Computer names appear in rows and subtitles only with two or more computers.
    var showsComputerNames: Bool { planes.count > 1 }

    func planeSetupSnapshot(for planeRegistryID: UUID) -> JetSetupSnapshot? {
        planes.first(where: { $0.id == planeRegistryID })?.snapshot
            ?? (planeRegistryID == localPlaneRegistryID ? setupSnapshot : nil)
    }

    /// The name of a task's project, when its computer reported it.
    func projectName(for conversationID: UUID) -> String? {
        guard let projectID = conversations.first(where: { $0.id == conversationID })?.projectID
        else { return nil }
        let planeRegistryID = conversationPlaneRegistryIDs[conversationID] ?? localPlaneRegistryID
        return planeSetupSnapshot(for: planeRegistryID)?.projects.projects
            .first { $0.id == projectID }?.name
    }

    func settingsAccess(for planeRegistryID: UUID) async throws -> any JetSettingsAccess {
        try await client(for: planeRegistryID)
    }

    // MARK: - Status

    /// The open task's status, derived only from its snapshot, execution,
    /// freshness and Git deliveries. Unlike its row, it also reads Offline while
    /// the task shows a saved view.
    var selectedTaskStatus: TaskStatus {
        guard usesLivePlane, selectedConversationID != nil else { return .unknown }
        let isOffline = !planeIsConnected || conversationFreshness == .cached
        guard let facts = selectedTaskFacts else { return isOffline ? .offline : .unknown }
        return TaskStatus.derive(facts: facts, isOffline: isOffline, phase: currentPhase)
    }

    /// The open task's facts from its loaded snapshot, or nil before it loads.
    private var selectedTaskFacts: TaskStatusFacts? {
        guard let conversationID = selectedConversationID,
              let snapshot = conversationSnapshot,
              snapshot.conversation.id == conversationID
        else { return nil }
        let execution = runExecution?.run.conversationID == conversationID ? runExecution : nil
        let latestRun = snapshot.runs.last
        let lifecycle = execution?.run.lifecycle ?? latestRun?.lifecycle
        // Until the Run's execution loads, keep the activity the store already knows
        // for the same Run, so a task waiting for permission doesn't flicker to Working.
        let stored = statusStore.facts[conversationID]
        let activity = execution?.activity
            ?? (execution == nil && stored?.runID != nil && stored?.runID == latestRun?.id ? stored?.activity : nil)
        return TaskStatusFacts(
            lifecycle: lifecycle,
            activity: lifecycle?.isLive == true ? activity : nil,
            runID: execution?.run.id ?? latestRun?.id,
            hasRuns: !snapshot.runs.isEmpty || execution != nil,
            hasRecordedChanges: hasKnownChanges,
            gitUnconfirmed: gitDeliveries.contains {
                $0.conversationID == conversationID && $0.needsAcknowledgement
            }
        )
    }

    /// Any task's status, as its sidebar row shows it. The open task uses its live
    /// state; others use the store. A task reads Offline only while its computer
    /// is offline (`isComputerOffline`), never during a connecting blip.
    func taskStatus(for conversationID: UUID) -> TaskStatus {
        guard usesLivePlane else { return .unknown }
        let planeRegistryID = conversationPlaneRegistryIDs[conversationID] ?? localPlaneRegistryID
        let isOffline = isComputerOffline(planeRegistryID)
        if conversationID == selectedConversationID, let facts = selectedTaskFacts {
            return TaskStatus.derive(facts: facts, isOffline: isOffline, phase: currentPhase)
        }
        return TaskStatus.derive(
            facts: statusStore.facts[conversationID],
            isOffline: isOffline,
            phase: nil
        )
    }

    /// A reply arrived that the person hasn't seen. The open task is never unread.
    func isUnread(_ conversationID: UUID) -> Bool {
        guard conversationID != selectedConversationID,
              let reply = statusStore.facts[conversationID]?.lastReplySequence
        else { return false }
        return reply > (memory.seenSequence(conversationID) ?? 0)
    }

    /// Listed tasks that need the person, in list order.
    var needsYouConversationIDs: [UUID] {
        conversations.map(\.id).filter { taskStatus(for: $0).needsYou }
    }

    var currentPhase: TaskPhase? {
        TaskPhase.latest(in: timeline)
    }

    /// A reply is running: Interrupt applies and Keep Changes waits.
    var isReplyRunning: Bool {
        canInterruptTurn || selectedTaskStatus.isInProgress
    }

    // MARK: - Assistant

    /// The product name for a Craft ID, such as Claude Code or Codex.
    static func assistantLabel(craftID: String, crafts: [JetInstalledCraft]) -> String {
        if let harness = crafts.first(where: { $0.id == craftID })?.harnesses.first {
            return harnessLabel(harness)
        }
        let lowered = craftID.lowercased()
        if lowered.contains("claude") { return harnessLabel("claude-code") }
        if lowered.contains("codex") { return harnessLabel("codex") }
        return harnessLabel(craftID)
    }

    /// The assistant this client started the task with. Runs don't expose their
    /// Craft, so tasks started elsewhere return nil.
    func assistantName(for conversationID: UUID) -> String? {
        guard let craftID = memory.assistant(for: conversationID) else { return nil }
        let planeRegistryID = conversationPlaneRegistryIDs[conversationID] ?? localPlaneRegistryID
        return Self.assistantLabel(
            craftID: craftID,
            crafts: planeSetupSnapshot(for: planeRegistryID)?.capabilities.crafts ?? []
        )
    }

    /// The assistant of the open task, or the one the next start will use.
    var selectedAssistantName: String? {
        if let conversationID = selectedConversationID,
           let name = assistantName(for: conversationID)
        {
            return name
        }
        guard selectedConversationID == nil || nextSendStartsNewRun,
              let craftID = selectedCraftID
        else { return nil }
        return Self.assistantLabel(
            craftID: craftID,
            crafts: selectedSetupSnapshot?.capabilities.crafts ?? []
        )
    }

    // MARK: - Composer

    static func sendRoute(hasLiveRun: Bool, hasRuns: Bool) -> SendRoute {
        hasLiveRun || hasRuns ? .submitTurn : .startRun
    }

    /// How the next message is sent, or nil while the open task's Runs are unknown.
    var sendRoute: SendRoute? {
        guard let conversationID = selectedConversationID else { return .startRun }
        if hasLiveRun { return .submitTurn }
        if let snapshot = conversationSnapshot, snapshot.conversation.id == conversationID {
            return Self.sendRoute(hasLiveRun: false, hasRuns: !snapshot.runs.isEmpty)
        }
        if let facts = statusStore.facts[conversationID] {
            return Self.sendRoute(hasLiveRun: facts.lifecycle?.isLive == true, hasRuns: facts.hasRuns)
        }
        return nil
    }

    var nextSendStartsNewRun: Bool { sendRoute == .startRun }

    /// New Task and a task without Runs choose an assistant; continuing never switches it.
    var showsAssistantPicker: Bool { usesLivePlane && nextSendStartsNewRun }

    /// Jet is still starting on This Mac for New Task.
    var isGettingReady: Bool {
        guard selectedConversationID == nil,
              newTaskPlaneRegistryID == localPlaneRegistryID,
              !planeIsConnected || setupSnapshot == nil
        else { return false }
        switch setupState {
        case .idle, .loading: return true
        case .ready, .failed: return false
        }
    }

    var gitStepUnconfirmed: Bool {
        guard let conversationID = selectedConversationID else { return false }
        return gitDeliveries.contains { $0.conversationID == conversationID && $0.needsAcknowledgement }
            || statusStore.facts[conversationID]?.gitUnconfirmed == true
    }

    /// The first reason the draft can't be sent. One value feeds the button, the
    /// menu item and the caption.
    var sendBlocker: SendBlocker? {
        if userOperation != nil { return .busy }
        let isNewTask = selectedConversationID == nil
        guard usesLivePlane else { return .notConnected(computer: selectedPlaneName) }
        if isGettingReady { return .gettingReady }
        if !planeIsConnected { return .notConnected(computer: selectedPlaneName) }
        if isNewTask, selectedProject == nil { return .noProject }
        guard let route = sendRoute else { return .busy }
        if route == .startRun,
           selectedSetupSnapshot?.capabilities.crafts.contains(where: { $0.id == selectedCraftID }) != true
        {
            return .noAssistant
        }
        if gitStepUnconfirmed { return .gitStepUnconfirmed }
        if queueIsFull { return .queueFull }
        if draftBytes > JetTurnQueue.maximumPromptBytes { return .tooLong }
        if draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return .empty }
        return nil
    }

    var canSend: Bool { sendBlocker == nil }

    /// Reads "Starting…" or "Sending…" only during the person's own action.
    var sendButtonTitle: String {
        switch userOperation {
        case .starting: return String(localized: "Starting…")
        case .sending: return String(localized: "Sending…")
        case .renaming, .none: break
        }
        return nextSendStartsNewRun ? String(localized: "Start Task") : String(localized: "Send")
    }

    var composerPlaceholder: String {
        if let composerPlaceholderOverride { return composerPlaceholderOverride }
        if selectedConversationID == nil || nextSendStartsNewRun {
            return String(localized: "Describe the change, bug, or question…")
        }
        if isReplyRunning { return String(localized: "Send a follow-up…") }
        if let name = selectedAssistantName { return String(localized: "Reply to \(name)…") }
        return String(localized: "Reply to the assistant…")
    }

    /// The line under the field: a blocker with its fix, otherwise what sending does.
    var composerCaption: String? {
        if let message = sendBlocker?.message { return message }
        guard let route = sendRoute else { return nil }
        if route == .submitTurn {
            if isReplyRunning { return String(localized: "Sends after the current reply finishes.") }
            if let name = selectedAssistantName {
                return String(localized: "\(name) continues where it left off.")
            }
            return String(localized: "The assistant continues where it left off.")
        }
        let project = selectedConversationID.flatMap(projectName(for:)) ?? selectedProject?.name
        guard let project else { return nil }
        return String(localized: "Jet works in a separate copy of \(project). Your folder doesn't change until you keep the changes.")
    }

    /// The notice above the composer while the open task waits for a sign-in or a
    /// usage reset (design §6.10). Sending a message continues the task afterwards.
    var statusNotice: ComposerNotice? {
        switch selectedTaskStatus {
        case .needsSignIn:
            let isLocal = isLocalPlane(selectedPlaneRegistryID)
            let computer = selectedPlaneName
            let text: String = switch (selectedAssistantName, isLocal) {
            case let (name?, true):
                String(localized: "Sign in to \(name) on this Mac, then send a message to continue.")
            case let (name?, false):
                String(localized: "Sign in to \(name) on \(computer), then send a message to continue.")
            case (nil, true):
                String(localized: "Sign in to the assistant on this Mac, then send a message to continue.")
            case (nil, false):
                String(localized: "Sign in to the assistant on \(computer), then send a message to continue.")
            }
            return ComposerNotice(kind: .warning, text: text, action: .openSettings(.agents))
        case .usageLimit:
            return ComposerNotice(
                kind: .warning,
                text: String(localized: "Usage limit reached. Send a message after it resets."),
                action: .showUsage
            )
        default:
            return nil
        }
    }

    /// Fills an empty composer with an example; it is never sent automatically.
    func insertExample(_ text: String) {
        guard draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        draft = text
        composerFocusRequest += 1
    }

    /// Puts an earlier message into the composer to edit; it is never sent automatically.
    func editAsNewMessage(_ text: String) {
        if draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            draft = text
        } else {
            draft += "\n\n" + text
        }
        composerFocusRequest += 1
    }

    func perform(_ action: ComposerNotice.Action) {
        switch action {
        case .tryAgainConnection:
            Task { await retryConnection() }
        case .addProject:
            presentAddProject()
        case .checkAssistantsAgain:
            Task { await loadSetup() }
        case .reviewGitStep:
            showDetails(.run)
        case let .openSettings(pane):
            requestSettings(pane)
            settingsOpenRequest += 1
        case .showUsage:
            requestSettings(.agents)
            settingsOpenRequest += 1
        case let .undoMoveToTrash(ref):
            Task { await undoMoveToTrash(ref) }
        }
    }

    /// Try Again: restarts the bounded setup retries and reloads the task list.
    func retryConnection() async {
        setupRetryTask?.cancel()
        setupRetryTask = nil
        setupRetryAttempt = 0
        if composerNotice?.action == .tryAgainConnection { composerNotice = nil }
        await loadSetup()
        await loadConversations()
    }

    // MARK: - Run control

    /// Any sheet or dialog is open; Interrupt (⌘.) waits until it closes.
    var isModalPresented: Bool {
        presentedSheet != nil
            || runControlConfirmation != nil
            || pendingNavigation != nil
            || removalPreview != nil
            || gitDeliveryConfirmation != nil
            || gitDeliveryAcknowledgementConfirmation != nil
            || isProjectImporterPresented
            || pairedClientPendingRevocation != nil
    }

    /// Asks to interrupt the current reply. With `thenReply`, the composer then asks
    /// what the assistant should do instead.
    func requestInterrupt(thenReply: Bool = false) {
        guard canInterruptTurn, supervisionOperation == nil, !isModalPresented else { return }
        interruptThenReply = thenReply
        runControlConfirmation = .interruptTurn
    }

    /// Asks to stop the assistant for this task (Stop Run). It has no shortcut.
    func requestStopAssistant() {
        guard canStopRun, supervisionOperation == nil, !isModalPresented else { return }
        interruptThenReply = false
        runControlConfirmation = .stopRun
    }

    // MARK: - Details

    /// Details apply only to an open task, never to New Task or Jet Trash.
    var canShowDetails: Bool {
        selectedConversationID != nil && sidebarSelection == .conversation
    }

    func toggleDetails() {
        if isWorkPanelPresented {
            isWorkPanelPresented = false
        } else if canShowDetails {
            isWorkPanelPresented = true
        }
    }

    func showDetails(_ tab: WorkPanelTab) {
        guard canShowDetails else { return }
        selectedWorkPanel = tab
        isWorkPanelPresented = true
    }

    /// Opens Details › Changes at the requested scope. All Changes is the
    /// `.current` checkpoint, whether or not the Run has finished.
    func showChanges(_ request: ChangesRequest) {
        guard canShowDetails else { return }
        guardUnsavedEdits { [weak self] in
            guard let self else { return }
            switch request {
            case .all:
                checkpointKind = .current
            case .lastReply:
                checkpointKind = .turn
                checkpointTurn = max(1, workDiff?.latestTurn ?? latestCheckpointTurn ?? 1)
            case let .reply(turn, runID):
                if let runID, let run = selectedRun, runID != run.id {
                    // Change scopes are per Run; an earlier Run's reply shows all changes.
                    checkpointKind = .current
                } else {
                    checkpointKind = .turn
                    checkpointTurn = max(1, turn)
                }
            }
            selectedWorkPanel = .changes
            isWorkPanelPresented = true
            selectedWorkFilePath = nil
            editableFile = nil
            fileDraft = ""
            keepsRequestedCheckpoint = true
            Task { await self.loadWorkPanel(preserveContinuity: false) }
        }
    }

    /// The newest checkpoint Turn the transcript knows about.
    var latestCheckpointTurn: UInt32? {
        timeline.last { $0.checkpointTurn != nil }?.checkpointTurn
    }

    // MARK: - Keep Changes

    var hasKnownChanges: Bool {
        guard let conversationID = selectedConversationID else { return false }
        if let workDiff, workDiff.totalFiles > 0 { return true }
        if statusStore.facts[conversationID]?.hasRecordedChanges == true { return true }
        return timeline.contains { $0.checkpointTurn != nil }
    }

    var keepChangesUnavailableReason: String? {
        guard usesLivePlane, selectedConversationID != nil else {
            return String(localized: "Open a task to keep its changes.")
        }
        guard selectedRun != nil else {
            return selectedAssistantName.map {
                String(localized: "Changes appear once \($0) starts.")
            } ?? String(localized: "Changes appear once the assistant starts.")
        }
        guard planeIsConnected else {
            return String(localized: "Not connected to \(selectedPlaneName).")
        }
        guard selectedSetupSnapshot?.capabilities.gitIsAvailable == true else {
            return String(localized: "Git isn't available on \(selectedPlaneName).")
        }
        if isReplyRunning {
            return selectedAssistantName.map {
                String(localized: "Available when \($0) finishes the current reply.")
            } ?? String(localized: "Available when the assistant finishes the current reply.")
        }
        return nil
    }

    var canKeepChanges: Bool { keepChangesUnavailableReason == nil }

    func presentKeepChanges(mode: KeepChangesMode = .plan) {
        guard let ref = selectedConversationRef else { return }
        presentedSheet = .keepChanges(ref, mode)
    }

    // MARK: - Sheets

    func presentRename(_ ref: ConversationRef? = nil) {
        guard let ref = ref ?? selectedConversationRef else { return }
        presentedSheet = .rename(ref)
    }

    func presentRepeatDaily(_ ref: ConversationRef? = nil) {
        guard let ref = ref ?? selectedConversationRef else { return }
        presentedSheet = .repeatDaily(ref)
    }

    func presentTaskSettings(_ ref: ConversationRef? = nil) {
        guard let ref = ref ?? selectedConversationRef else { return }
        presentedSheet = .taskSettings(ref)
    }

    func presentMoveToTrash(_ ref: ConversationRef? = nil, deleteEverywhere: Bool = false) {
        guard let ref = ref ?? selectedConversationRef else { return }
        presentedSheet = .moveToTrash(ref, deleteEverywhere: deleteEverywhere)
    }

    /// Opens Add Project for New Task's computer, optionally with a dropped folder.
    func presentAddProject(droppedURL: URL? = nil) {
        presentedSheet = .addProject(planeRegistryID: newTaskPlaneRegistryID, droppedURL: droppedURL)
    }

    func dismissSheet() {
        presentedSheet = nil
    }

    /// Compatibility bridge for views that predate `presentedSheet`.
    var isRenamePresented: Bool {
        get {
            if case .rename = presentedSheet { return true }
            return false
        }
        set {
            if newValue {
                presentRename()
            } else if case .rename = presentedSheet {
                presentedSheet = nil
            }
        }
    }

    // MARK: - Working copy

    /// The open task's separate working copy.
    var workingCopyPath: String? {
        guard let snapshot = conversationSnapshot,
              snapshot.conversation.id == selectedConversationID
        else { return nil }
        return snapshot.workspaceRoot
    }

    var canRevealWorkingCopy: Bool {
#if os(macOS)
        return workingCopyPath != nil && isLocalPlane(selectedPlaneRegistryID)
#else
        return false
#endif
    }

    func revealWorkingCopy() {
#if os(macOS)
        guard canRevealWorkingCopy, let workingCopyPath else { return }
        NSWorkspace.shared.activateFileViewerSelecting([
            URL(fileURLWithPath: workingCopyPath, isDirectory: true),
        ])
#endif
    }

    func copyWorkingCopyPath() {
        guard let workingCopyPath else { return }
        Self.copyToPasteboard(workingCopyPath)
    }

    static func copyToPasteboard(_ text: String) {
#if os(macOS)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
#elseif canImport(UIKit)
        UIPasteboard.general.string = text
#endif
    }

    // MARK: - Waiting messages

    /// The waiting queue entry for a "You" transcript entry, with its 1-based place.
    func queuedEntry(forTimelineID timelineID: String) -> (ordinal: Int, entry: JetTurnQueueEntry)? {
        let waiting = (turnQueue?.turns ?? [])
            .filter { $0.state == .queued }
            .sorted { $0.position < $1.position }
        let wanted = timelineID.lowercased()
        guard let index = waiting.firstIndex(where: { $0.id.uuidString.lowercased() == wanted })
        else { return nil }
        return (index + 1, waiting[index])
    }

    /// The text of a waiting message, joined from the transcript by Turn ID.
    func queuedText(for entry: JetTurnQueueEntry) -> String? {
        let id = entry.id.uuidString.lowercased()
        return timeline.first { $0.kind == .user && $0.id == id }?.text
    }

    /// Withdraws a waiting message, then puts its text back into an empty composer.
    func removeQueuedMessage(_ entry: JetTurnQueueEntry) async {
        guard let conversationID = selectedConversationID else { return }
        let text = queuedText(for: entry)
        guard await performWithdrawal(entry) else { return }
        let timelineID = entry.id.uuidString.lowercased()
        if selectedConversationID == conversationID {
            timeline.removeAll { $0.kind == .user && $0.id == timelineID }
            transcripts.save(timeline, for: conversationID)
        }
        let key = DraftKey.conversation(conversationID)
        if let text, (drafts[key] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            drafts[key] = text
            composerNotice = ComposerNotice(
                kind: .confirmation,
                text: String(localized: "Removed. The text is back in the message box.")
            )
            composerFocusRequest += 1
        } else {
            composerNotice = ComposerNotice(kind: .confirmation, text: String(localized: "Removed."))
        }
    }

    // MARK: - Unsaved edits

    var hasUnsavedFileEdit: Bool {
        guard let editableFile else { return false }
        return fileDraft != (editableFile.content ?? "")
    }

    /// Runs `action` now, or after the person decides about an unsaved file edit.
    func guardUnsavedEdits(_ action: @escaping @MainActor () -> Void) {
        guard hasUnsavedFileEdit, let editableFile else {
            action()
            return
        }
        pendingNavigation = PendingNavigation(
            fileName: URL(fileURLWithPath: editableFile.path).lastPathComponent,
            perform: action
        )
    }

    /// Save (true), Don't Save (false) or Cancel (nil) for a held navigation.
    func resolvePendingNavigation(save: Bool?) async {
        guard let pending = pendingNavigation else { return }
        pendingNavigation = nil
        switch save {
        case .none:
            return
        case .some(true):
            await saveSelectedWorkFile()
            // A failed save keeps the edit and the place; the file notice explains why.
            guard !hasUnsavedFileEdit else { return }
            pending.perform()
        case .some(false):
            fileDraft = editableFile?.content ?? ""
            pending.perform()
        }
    }

    // MARK: - Other

    /// Opens a task from outside the sidebar, such as a notification.
    func openTask(conversationID: UUID, planeRegistryID: UUID) {
        conversationPlaneRegistryIDs[conversationID] = planeRegistryID
        guardUnsavedEdits { [weak self] in self?.selectConversation(conversationID) }
    }

    /// Restores a task from Jet Trash. The Command ID is kept until the outcome is
    /// known, and an uncertain restore is never retried automatically.
    func undoMoveToTrash(_ ref: ConversationRef) async {
        let commandID = restoreCommandIDs[ref.conversationID] ?? UUID()
        restoreCommandIDs[ref.conversationID] = commandID
        do {
            try await recoveryAccess(for: ref.planeRegistryID)
                .restoreConversation(ref.conversationID, commandID: commandID)
            restoreCommandIDs.removeValue(forKey: ref.conversationID)
            trashedConversationIDs.remove(ref.conversationID)
            memory.forgetTrashedTitle(ref.conversationID)
            conversationPlaneRegistryIDs[ref.conversationID] = ref.planeRegistryID
            composerNotice = nil
            await loadConversations()
            selectConversation(ref.conversationID)
        } catch {
            if case JetClientFailure.commandOutcomeUnknown = error {
                // Keep the Command ID so a deliberate retry stays the same Command.
            } else {
                restoreCommandIDs.removeValue(forKey: ref.conversationID)
            }
            composerNotice = failureNotice(for: error)
        }
    }
}
