import Foundation
import Testing
@testable import jet

@MainActor
struct SessionSelectionTests {
    // MARK: - Restoration and refresh

    @Test(arguments: ["search", "needsAttention", "schedules", "planes"])
    func retiredDestinationsRestoreAsTheLastTask(_ raw: String) {
        let session = Self.liveSession()

        session.restore(selection: raw, workPanel: "run", panelPresented: false)

        #expect(session.sidebarSelection == .conversation)
        #expect(session.selectedWorkPanel == .run)
    }

    @Test
    func currentDestinationsRestoreAsThemselves() {
        let cases: [(String, SidebarDestination)] = [
            ("newTask", .newTask), ("project", .project), ("conversation", .conversation), ("trash", .trash),
        ]
        for (raw, expected) in cases {
            let session = Self.liveSession()
            session.restore(selection: raw, workPanel: "retired-panel", panelPresented: false)
            #expect(session.sidebarSelection == expected)
            #expect(session.selectedWorkPanel == .changes)
        }
    }

    @Test
    func aRefreshNeverPicksAnotherTask() {
        let first = UUID()
        let second = UUID()

        #expect(DesktopSession.resolveSelection(
            candidate: first, previous: nil, available: [second, first], isNewTask: true
        ) == nil)
        #expect(DesktopSession.resolveSelection(
            candidate: nil, previous: nil, available: [first, second], isNewTask: false
        ) == nil)
        #expect(DesktopSession.resolveSelection(
            candidate: UUID(), previous: nil, available: [first, second], isNewTask: false
        ) == nil)
        #expect(DesktopSession.resolveSelection(
            candidate: UUID(), previous: second, available: [first, second], isNewTask: false
        ) == second)
        #expect(DesktopSession.resolveSelection(
            candidate: first, previous: second, available: [first, second], isNewTask: false
        ) == first)
    }

    @Test
    func sidebarItemsMapOntoTheSessionSelection() {
        let session = Self.connected()
        let task = Self.addTask(session, "Fix login redirect loop")

        session.sidebarItem = .task(task.id)
        #expect(session.sidebarSelection == .conversation)
        #expect(session.selectedConversationID == task.id)
        #expect(session.sidebarItem == .task(task.id))

        session.sidebarItem = .project(Self.project.id)
        #expect(session.sidebarSelection == .project)
        #expect(session.selectedProjectID == Self.project.id)
        #expect(session.selectedConversationID == nil)

        session.isWorkPanelPresented = true
        session.showTrash()
        #expect(session.sidebarSelection == .trash)
        #expect(session.sidebarItem == .trash)
        #expect(!session.isWorkPanelPresented)

        session.sidebarItem = .newTask
        #expect(session.sidebarSelection == .newTask)
        #expect(session.sidebarItem == .newTask)
    }

    @Test
    func needsAttentionOpensTheFirstTaskThatNeedsYou() {
        let session = Self.connected()
        _ = Self.addTask(session, "Calm task")
        let blocked = Self.addTask(session, "Blocked task")
        let run = Self.run(for: blocked, lifecycle: .active)
        session.statusStore.record(
            snapshot: Self.snapshot(blocked, runs: [run]),
            execution: JetRunExecution(cursor: 5, run: run, activity: .waitingForApproval, needsAttention: true, termination: nil),
            cursor: 5
        )

        session.sidebarSelection = .needsAttention
        session.applySidebarSelection()

        #expect(session.sidebarSelection == .conversation)
        #expect(session.selectedConversationID == blocked.id)
        #expect(session.composerNotice == nil)

        let calm = Self.connected()
        _ = Self.addTask(calm, "Calm task")
        calm.sidebarSelection = .needsAttention
        calm.applySidebarSelection()
        #expect(calm.sidebarSelection == .newTask)
    }

    @Test
    func selectingNewTaskLeavesFocusInTheSidebar() {
        let session = Self.connected()
        _ = Self.openTask(session, lifecycle: .completed)
        let focusRequest = session.composerFocusRequest

        // Arrowing onto the New Task row goes through the sidebar selection.
        session.sidebarItem = .newTask
        #expect(session.sidebarSelection == .newTask)
        #expect(session.composerFocusRequest == focusRequest)

        _ = Self.openTask(session, lifecycle: .completed)
        session.beginNewTask()
        #expect(session.sidebarSelection == .newTask)
        #expect(session.composerFocusRequest == focusRequest)

        session.sidebarSelection = .newTask
        session.applySidebarSelection()
        #expect(session.composerFocusRequest == focusRequest)
    }

    // MARK: - Drafts

    @Test
    func draftsBelongToTheirDestination() {
        let session = Self.connected()
        let first = Self.addTask(session, "First")
        let second = Self.addTask(session, "Second")

        session.beginNewTask()
        session.draft = "Start something new"
        session.selectConversation(first.id)
        #expect(session.draft.isEmpty)

        session.draft = "Reply to the first task"
        session.selectConversation(second.id)
        #expect(session.draft.isEmpty)

        session.selectConversation(first.id)
        #expect(session.draft == "Reply to the first task")

        session.beginNewTask()
        #expect(session.draft == "Start something new")
        #expect(session.hasNewTaskDraft)
    }

    @Test
    func aStartedTaskTakesTheNewTaskDraft() {
        let session = Self.connected()
        session.beginNewTask()
        session.draft = "Fix the login redirect"
        let created = UUID()

        session.moveNewTaskDraft(to: created)

        #expect(!session.hasNewTaskDraft)
        #expect(session.drafts[.conversation(created)] == "Fix the login redirect")

        session.clearSentDraft("An older message", for: created)
        #expect(session.drafts[.conversation(created)] == "Fix the login redirect")

        session.clearSentDraft("Fix the login redirect", for: created)
        #expect(session.drafts[.conversation(created)] == nil)
    }

    @Test
    func confirmationsClearOnTheNextEdit() {
        let session = Self.connected()
        session.composerNotice = ComposerNotice(kind: .confirmation, text: "Removed.")
        session.draft = "a"
        #expect(session.composerNotice == nil)

        session.composerNotice = ComposerNotice(kind: .warning, text: "Kept")
        session.draft = "ab"
        #expect(session.composerNotice?.text == "Kept")

        session.actionNotice = "Bridged"
        #expect(session.composerNotice == ComposerNotice(kind: .info, text: "Bridged"))
        session.actionNotice = nil
        #expect(session.composerNotice == nil)
    }

    // MARK: - Send blockers

    @Test
    func withoutAComputerTheMessageIsKept() {
        let fixture = DesktopSession()
        fixture.draft = "Hello"

        #expect(fixture.sendBlocker == .notConnected(computer: "This Mac"))
        #expect(fixture.sendBlocker?.message == "Not connected to This Mac. Your message is kept.")
        #expect(fixture.sendBlocker?.action == .tryAgainConnection)
        #expect(!fixture.canSend)

        let offline = Self.connected()
        offline.beginNewTask()
        offline.draft = "Hello"
        offline.connectionState = .disconnected
        #expect(offline.sendBlocker == .notConnected(computer: "This Mac"))
    }

    @Test
    func startingUpBlocksNewTaskUntilJetIsReady() {
        let session = Self.liveSession()
        session.setupState = .loading
        session.beginNewTask()
        session.draft = "Hello"

        #expect(session.sendBlocker == .gettingReady)
        #expect(session.sendBlocker?.message == "Jet is getting ready on this Mac…")
        #expect(session.sendBlocker?.action == nil)
    }

    @Test
    func newTaskNeedsAProjectAndAnAssistant() {
        let noProject = Self.connected(projects: [])
        noProject.beginNewTask()
        noProject.draft = "Hello"
        #expect(noProject.sendBlocker == .noProject)
        #expect(noProject.sendBlocker?.message == "Choose a project to start.")
        #expect(noProject.sendBlocker?.action == .addProject)

        let noAssistant = Self.connected(crafts: [])
        noAssistant.beginNewTask()
        noAssistant.draft = "Hello"
        #expect(noAssistant.sendBlocker == .noAssistant)
        #expect(noAssistant.sendBlocker?.message == "Install Claude Code or Codex to start.")
        #expect(noAssistant.sendBlocker?.action == .checkAssistantsAgain)
    }

    @Test
    func draftSizeAndEmptinessBlockSending() {
        let session = Self.connected()
        session.beginNewTask()

        #expect(session.sendBlocker == .empty)
        #expect(session.sendBlocker?.message == nil)

        session.draft = String(repeating: "a", count: JetTurnQueue.maximumPromptBytes + 1)
        #expect(session.sendBlocker == .tooLong)
        #expect(session.sendBlocker?.message == "Message is too long.")

        session.draft = "Fix the login redirect"
        #expect(session.sendBlocker == nil)
        #expect(session.canSend)

        session.userOperation = .starting
        #expect(session.sendBlocker == .busy)
        #expect(session.sendBlocker?.message == nil)
    }

    @Test
    func aFullQueueBlocksTheNextMessage() {
        let session = Self.connected()
        let task = Self.openTask(session, lifecycle: .active, activity: .working)
        session.draft = "One more thing"
        #expect(session.sendBlocker == nil)

        session.turnQueue = JetTurnQueue(
            cursor: 1,
            turns: (1 ... JetTurnQueue.maximumEntries).map { position in
                JetTurnQueueEntry(
                    id: UUID(), sequence: UInt64(position), position: position, source: .user,
                    state: .queued, runID: session.selectedRun?.id, withdrawable: true
                )
            }
        )
        #expect(session.sendBlocker == .queueFull)
        #expect(session.sendBlocker?.message == "Too many messages are waiting. Remove one or wait.")
        #expect(session.selectedConversationID == task.id)
    }

    @Test
    func anUnconfirmedGitStepPausesTheTask() {
        let session = Self.connected()
        let task = Self.openTask(session, lifecycle: .completed)
        session.draft = "Continue"
        #expect(session.sendBlocker == nil)

        session.gitDeliveries = [Self.unknownDelivery(for: task.id)]

        #expect(session.sendBlocker == .gitStepUnconfirmed)
        #expect(session.sendBlocker?.message == "Jet paused this task until you check a Git step.")
        #expect(session.sendBlocker?.action == .reviewGitStep)
        #expect(session.selectedTaskStatus == .gitUnconfirmed)
    }

    @Test
    func aTaskStillLoadingCanNotChooseHowToSend() {
        let session = Self.connected()
        let task = Self.addTask(session, "Loading")
        session.sidebarSelection = .conversation
        session.selectedConversationID = task.id
        session.draft = "Hello"

        #expect(session.sendRoute == nil)
        #expect(session.sendBlocker == .busy)
    }

    // MARK: - Continuation

    @Test
    func continuingATaskNeverStartsAnotherRun() {
        #expect(DesktopSession.sendRoute(hasLiveRun: true, hasRuns: true) == .submitTurn)
        #expect(DesktopSession.sendRoute(hasLiveRun: false, hasRuns: true) == .submitTurn)
        #expect(DesktopSession.sendRoute(hasLiveRun: false, hasRuns: false) == .startRun)

        let session = Self.connected()
        session.beginNewTask()
        #expect(session.sendRoute == .startRun)
        #expect(session.showsAssistantPicker)

        _ = Self.openTask(session, lifecycle: .active, activity: .working)
        #expect(session.sendRoute == .submitTurn)
        #expect(!session.showsAssistantPicker)

        _ = Self.openTask(session, lifecycle: .completed)
        #expect(session.sendRoute == .submitTurn)
        #expect(!session.nextSendStartsNewRun)

        _ = Self.openTask(session, lifecycle: nil)
        #expect(session.sendRoute == .startRun)
        #expect(session.showsAssistantPicker)
    }

    @Test
    func sendTitleReflectsOnlyThePersonsOwnAction() {
        let session = Self.connected()
        session.beginNewTask()
        #expect(session.sendButtonTitle == "Start Task")

        session.isRefreshingConversation = true
        session.isLoadingMoreConversations = true
        #expect(session.sendButtonTitle == "Start Task")

        session.userOperation = .starting
        #expect(session.sendButtonTitle == "Starting…")
        #expect(session.conversationOperation == "starting")
        session.userOperation = nil

        _ = Self.openTask(session, lifecycle: .active, activity: .waitingForUser)
        session.isRefreshingConversation = true
        #expect(session.sendButtonTitle == "Send")
        session.userOperation = .sending
        #expect(session.sendButtonTitle == "Sending…")
        session.userOperation = .renaming
        #expect(session.sendButtonTitle == "Send")
    }

    @Test
    func aFailedSendKeepsARenameStartedDuringItsRefresh() async {
        let factory = ClientFactoryProbe()
        let session = Self.connected(isPreviewSession: false) { try await factory.makeClient() }
        factory.onCall = { [weak session] call in
            // The first call is the send; the second is the task-list refresh after it failed.
            if call == 2 { session?.userOperation = .renaming }
        }
        _ = Self.openTask(session, lifecycle: .active, activity: .waitingForUser)
        session.draft = "Thanks"

        await session.submitDraft()

        #expect(factory.calls >= 2)
        #expect(session.actionError == .offline)
        #expect(session.userOperation == .renaming)
    }

    @Test
    func aFailedSendReleasesItsOwnOperation() async {
        let factory = ClientFactoryProbe()
        let session = Self.connected(isPreviewSession: false) { try await factory.makeClient() }
        _ = Self.openTask(session, lifecycle: .active, activity: .waitingForUser)
        session.draft = "Thanks"

        await session.submitDraft()

        #expect(session.actionError == .offline)
        #expect(session.userOperation == nil)
        #expect(session.draft == "Thanks")
    }

    @Test
    func composerCopyFollowsTheTask() {
        let session = Self.connected()
        session.beginNewTask()
        #expect(session.composerPlaceholder == "Describe the change, bug, or question…")
        session.draft = "Fix it"
        #expect(session.composerCaption == "Jet works in a separate copy of web-app. Your folder doesn't change until you keep the changes.")

        let task = Self.openTask(session, lifecycle: .active, activity: .waitingForUser)
        session.memory.recordAssistant("jet-craft-claude", for: task.id)
        session.draft = "Thanks"
        #expect(session.composerPlaceholder == "Reply to Claude Code…")
        #expect(session.composerCaption == "Claude Code continues where it left off.")

        session.runExecution = JetRunExecution(
            cursor: 2, run: session.selectedRun!, activity: .working, needsAttention: false, termination: nil
        )
        #expect(session.composerPlaceholder == "Send a follow-up…")
        #expect(session.composerCaption == "Sends after the current reply finishes.")
    }

    @Test
    func signInAndUsageLimitsExplainHowToContinue() {
        let session = Self.connected()
        let task = Self.openTask(session, lifecycle: .active, activity: .waitingForAuth)
        session.memory.recordAssistant("jet-craft-claude", for: task.id)

        #expect(session.statusNotice == ComposerNotice(
            kind: .warning,
            text: "Sign in to Claude Code on this Mac, then send a message to continue.",
            action: .openSettings(.agents)
        ))
        #expect(session.statusNotice?.action?.title == "Assistant Settings…")

        _ = Self.openTask(session, lifecycle: .active, activity: .waitingForQuota)
        #expect(session.statusNotice == ComposerNotice(
            kind: .warning,
            text: "Usage limit reached. Send a message after it resets.",
            action: .showUsage
        ))
        #expect(session.statusNotice?.action?.title == "Usage…")

        _ = Self.openTask(session, lifecycle: .active, activity: .working)
        #expect(session.statusNotice == nil)
    }

    // MARK: - Offline computers

    @Test
    func connectionBlipsNeverTurnTasksOffline() {
        let session = Self.connected()
        let local = session.localPlaneRegistryID
        let blocked = Self.addTask(session, "Blocked task")
        Self.recordNeedsPermission(blocked, in: session)

        for state in [JetConnectionState.connecting, .reconnecting(attempt: 1), .disconnected] {
            Self.setLocalConnection(state, in: session)
            #expect(!session.isComputerOffline(local), "\(state)")
            #expect(session.taskStatus(for: blocked.id) == .needsPermission, "\(state)")
            #expect(session.needsYouConversationIDs == [blocked.id], "\(state)")
        }
    }

    @Test
    func aComputerIsOfflineOnlyAfterAFailure() {
        let session = Self.connected()
        let local = session.localPlaneRegistryID
        let blocked = Self.addTask(session, "Blocked task")
        Self.recordNeedsPermission(blocked, in: session)

        Self.setLocalConnection(.reconnecting(attempt: 2), in: session)
        session.updatePlane(local) { $0.failure = .offline }
        #expect(session.isComputerOffline(local))
        #expect(session.taskStatus(for: blocked.id) == .offline)
        #expect(session.needsYouConversationIDs.isEmpty)

        session.updatePlane(local) { $0.failure = nil }
        Self.setLocalConnection(.failed(.offline), in: session)
        #expect(session.isComputerOffline(local))
        #expect(session.taskStatus(for: blocked.id) == .offline)

        Self.setLocalConnection(.connected(Self.negotiation), in: session)
        session.updatePlane(local) { $0.failure = .offline }
        #expect(!session.isComputerOffline(local))
        #expect(session.taskStatus(for: blocked.id) == .needsPermission)
        // A computer Jet no longer knows can't be reached.
        #expect(session.isComputerOffline(UUID()))
    }

    @Test
    func eachComputerDecidesItsOwnTasksOffline() {
        let session = Self.connected()
        let studio = JetPlanePresentation(
            id: UUID(), name: "Studio Mac", endpoint: "alex@studio.example", isLocal: false,
            planeID: UUID(), connection: .reconnecting(attempt: 1), snapshot: nil, failure: nil,
            conversationCursor: nil
        )
        session.planes.append(studio)
        let remote = Self.addTask(session, "Remote task")
        session.conversationPlaneRegistryIDs[remote.id] = studio.id
        Self.recordNeedsPermission(remote, in: session)
        let local = Self.addTask(session, "Local task")
        Self.recordNeedsPermission(local, in: session)

        #expect(!session.isComputerOffline(studio.id))
        #expect(session.taskStatus(for: remote.id) == .needsPermission)

        session.updatePlane(studio.id) { $0.failure = .offline }
        #expect(session.isComputerOffline(studio.id))
        #expect(session.taskStatus(for: remote.id) == .offline)
        #expect(session.taskStatus(for: local.id) == .needsPermission)
        #expect(session.needsYouConversationIDs == [local.id])
    }

    @Test
    func theOpenTasksRowStaysLiveWhileItsViewIsSaved() {
        let session = Self.connected()
        let open = Self.openTask(session, lifecycle: .active, activity: .waitingForApproval)
        Self.setLocalConnection(.reconnecting(attempt: 1), in: session)
        session.conversationFreshness = .cached

        // The open task shows its saved view; its row keeps the live status.
        #expect(session.selectedTaskStatus == .offline)
        #expect(session.taskStatus(for: open.id) == .needsPermission)
        #expect(session.needsYouConversationIDs == [open.id])

        session.updatePlane(session.localPlaneRegistryID) { $0.failure = .offline }
        #expect(session.taskStatus(for: open.id) == .offline)
    }

    // MARK: - Loads

    @Test
    func aCancelledLoadLeavesTheTaskAsItWas() async {
        let session = Self.connected(isPreviewSession: false)
        _ = Self.openTask(session, lifecycle: .completed)

        await session.loadSelectedConversation()
        #expect(session.conversationFreshness == .live)
        #expect(session.composerNotice == nil)

        await session.loadRunSupervision()
        #expect(session.conversationFreshness == .live)
        #expect(session.composerNotice == nil)
    }

    @Test
    func aFailedLoadShowsTheSavedView() async {
        let session = Self.connected(isPreviewSession: false) {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        _ = Self.openTask(session, lifecycle: .completed)

        await session.loadSelectedConversation()
        #expect(session.conversationFreshness == .cached)
        #expect(session.composerNotice?.text == "Jet couldn't refresh this task.")

        session.composerNotice = nil
        await session.loadRunSupervision()
        #expect(session.composerNotice?.text == "Jet couldn't refresh this task.")
    }

    // MARK: - Setup

    @Test
    func planeLabelSaysStartingWhileSetupLoads() {
        let session = Self.liveSession()
        session.setupState = .loading

        #expect(session.planeConnectionLabel == "Starting")

        session.connectionState = .reconnecting(attempt: 1)
        #expect(session.planeConnectionLabel == "Reconnecting")
    }

    @Test
    func setupRetriesAreBoundedAndThenFail() {
        let session = Self.liveSession()
        let failure = JetPresentationError.offline

        for attempt in 1 ... 3 {
            session.scheduleSetupRetry(after: failure)
            #expect(session.connectionState == .reconnecting(attempt: attempt))
            #expect(session.setupRetryAttempt == attempt)
            #expect(session.setupRetryTask != nil)
        }
        #expect(session.setupRetryLimit == 3)

        session.scheduleSetupRetry(after: failure)
        #expect(session.connectionState == .failed(failure))
        #expect(session.setupRetryTask == nil)

        let fatal = Self.liveSession()
        fatal.scheduleSetupRetry(after: .invalidResponse)
        #expect(fatal.connectionState == .failed(.invalidResponse))
        #expect(fatal.setupRetryAttempt == 0)

        let torn = Self.liveSession()
        torn.scheduleSetupRetry(after: failure)
        torn.teardown()
        #expect(torn.setupRetryTask == nil)
    }

#if os(macOS)
    @Test
    func installerFailuresMapToStableCodes() {
        let session = Self.liveSession()

        let incomplete = session.presentationError(JetLocalCoreInstaller.InstallerError.invalidPayload)
        #expect(incomplete.code == "core.install_incomplete")
        #expect(incomplete.message == "Jet's helper is incomplete. Reinstall Jet from its latest release.")
        #expect(!incomplete.retryable)

        let owned = session.presentationError(JetLocalCoreInstaller.InstallerError.otherChannel("brew"))
        #expect(owned.code == "core.owned_by_other_channel")
        #expect(owned.message == "Another copy of Jet already runs this Mac's helper.")
        #expect(!owned.message.contains("brew"))
        #expect(!owned.retryable)

        let failed = session.presentationError(
            JetLocalCoreInstaller.InstallerError.commandFailed("launchctl", "Bootstrap failed: 5: Input/output error")
        )
        #expect(failed.code == "core.start_failed")
        #expect(failed.message == "Jet couldn't start its helper on this Mac.")
        #expect(failed.retryable)
        #expect(!failed.message.contains("launchctl"))
    }
#endif

    // MARK: - Run control, Details and edits

    @Test
    func interruptAndStopAskWithoutOpeningDetails() {
        let session = Self.connected()
        let task = Self.openTask(session, lifecycle: .active, activity: .working)
        session.turnQueue = JetTurnQueue(cursor: 1, turns: [
            JetTurnQueueEntry(
                id: UUID(), sequence: 1, position: 1, source: .user,
                state: .active, runID: session.selectedRun?.id, withdrawable: false
            ),
        ])

        session.requestInterrupt(thenReply: true)
        #expect(session.runControlConfirmation == .interruptTurn)
        #expect(session.interruptThenReply)
        #expect(!session.isWorkPanelPresented)

        session.cancelRunControl()
        #expect(!session.interruptThenReply)

        session.presentedSheet = .rename(ConversationRef(conversationID: task.id, planeRegistryID: session.localPlaneRegistryID))
        session.requestInterrupt()
        #expect(session.runControlConfirmation == nil)
        session.dismissSheet()

        session.requestRunControl(.stopRun)
        #expect(session.runControlConfirmation == .stopRun)
        #expect(!session.isWorkPanelPresented)
    }

    @Test
    func detailsOpenOnlyForATask() {
        let session = Self.connected()
        session.beginNewTask()
        session.toggleDetails()
        #expect(!session.isWorkPanelPresented)

        _ = Self.openTask(session, lifecycle: .completed)
        session.showDetails(.terminal)
        #expect(session.isWorkPanelPresented)
        #expect(session.selectedWorkPanel == .terminal)
        session.toggleDetails()
        #expect(!session.isWorkPanelPresented)
        #expect(WorkPanelTab.inspectorTabs.map(\.title) == ["Changes", "Terminal", "Activity"])
    }

    @Test
    func allChangesIsTheCurrentCheckpointForFinishedTasks() async {
        let session = Self.connected()
        _ = Self.openTask(session, lifecycle: .completed)
        session.checkpointKind = .turn

        session.showChanges(.all)
        #expect(session.checkpointKind == .current)
        #expect(session.selectedWorkPanel == .changes)
        #expect(session.isWorkPanelPresented)

        session.showChanges(.reply(turn: 2, runID: session.selectedRun?.id))
        #expect(session.checkpointKind == .turn)
        #expect(session.checkpointTurn == 2)

        // An earlier Run's reply shows all changes.
        session.showChanges(.reply(turn: 2, runID: UUID()))
        #expect(session.checkpointKind == .current)

        // A preview session never loads, so the requested scope isn't kept for later.
        #expect(session.keepsRequestedCheckpoint)
        await session.loadWorkPanel(preserveContinuity: false)
        #expect(!session.keepsRequestedCheckpoint)
    }

    @Test
    func anotherRunsChangesStartAtAllChanges() async {
        let session = Self.connected(isPreviewSession: false)
        _ = Self.openTask(session, lifecycle: .completed)
        session.checkpointKind = .final

        await session.loadWorkPanel()
        #expect(session.checkpointKind == .current)

        // A task without Runs has no changes to load; the request isn't kept.
        _ = Self.openTask(session, lifecycle: nil)
        session.keepsRequestedCheckpoint = true
        await session.loadWorkPanel()
        #expect(!session.keepsRequestedCheckpoint)
    }

    @Test
    func unsavedEditsHoldNavigationUntilDecided() async {
        let session = Self.connected()
        _ = Self.openTask(session, lifecycle: .completed)
        session.editableFile = JetEditableFile(
            cursor: 1,
            target: .workspace(UUID()),
            path: "src/auth/login.ts",
            content: "old",
            revision: JetFileRevision(object: "a1", mode: "100644")
        )
        session.fileDraft = "new"
        #expect(session.hasUnsavedFileEdit)

        session.sidebarItem = .newTask
        #expect(session.sidebarSelection == .conversation)
        #expect(session.pendingNavigation?.fileName == "login.ts")

        await session.resolvePendingNavigation(save: nil)
        #expect(session.pendingNavigation == nil)
        #expect(session.sidebarSelection == .conversation)
        #expect(session.fileDraft == "new")

        session.sidebarItem = .newTask
        await session.resolvePendingNavigation(save: false)
        #expect(session.sidebarSelection == .newTask)
        #expect(!session.hasUnsavedFileEdit)
    }

    @Test
    func waitingMessagesJoinTheTranscriptByTurn() {
        let session = Self.connected()
        _ = Self.openTask(session, lifecycle: .active, activity: .working)
        let active = UUID()
        let next = UUID()
        let later = UUID()
        session.timeline = [
            JetTimelineEntry(id: next.uuidString.lowercased(), kind: .user, text: "Also add a test.", sequence: 3, rawCount: 0),
        ]
        session.turnQueue = JetTurnQueue(cursor: 1, turns: [
            JetTurnQueueEntry(id: later, sequence: 4, position: 3, source: .user, state: .queued, runID: nil, withdrawable: true),
            JetTurnQueueEntry(id: active, sequence: 2, position: 1, source: .user, state: .active, runID: nil, withdrawable: false),
            JetTurnQueueEntry(id: next, sequence: 3, position: 2, source: .user, state: .queued, runID: nil, withdrawable: true),
        ])

        #expect(session.queuedEntry(forTimelineID: next.uuidString.lowercased())?.ordinal == 1)
        #expect(session.queuedEntry(forTimelineID: later.uuidString.lowercased())?.ordinal == 2)
        #expect(session.queuedEntry(forTimelineID: active.uuidString.lowercased()) == nil)
        if let entry = session.queuedEntry(forTimelineID: next.uuidString.lowercased())?.entry {
            #expect(session.queuedText(for: entry) == "Also add a test.")
        } else {
            Issue.record("The waiting message was not joined to its transcript entry.")
        }
    }

    // MARK: - Events

    @Test
    func eventsFeedTheOpenTaskAndTheCacheSeparately() async {
        let session = Self.connected()
        let open = Self.openTask(session, lifecycle: .active, activity: .working)
        let other = UUID()
        let runID = session.selectedRun?.id
        let local = session.localPlaneRegistryID

        await session.receive(
            Self.event(sequence: 10, conversationID: other, kind: "turn.input",
                       payload: #"{"turn_id":"\#(UUID().uuidString)","text":"Hi"}"#),
            from: local
        )
        #expect(session.timeline.isEmpty)
        #expect(session.transcripts.entries(for: other).map(\.text) == ["Hi"])

        await session.receive(
            Self.event(sequence: 11, conversationID: open.id, runID: runID, kind: "run.output",
                       payload: #"{"presentation_json":["{\"kind\":\"markdown\",\"text\":\"Done\"}"]}"#),
            from: local
        )
        #expect(session.timeline.map(\.text) == ["Done"])
        #expect(session.timeline.first?.runID == runID)
        #expect(session.timeline.first?.recordedAtUnixMilliseconds == 1_000)
        #expect(session.transcripts.entries(for: open.id) == session.timeline)
        #expect(session.transcripts.entries(for: other).count == 1)

        await session.receive(
            Self.event(sequence: 12, conversationID: other, kind: "conversation.trashed", payload: "{}"),
            from: local
        )
        #expect(session.transcripts.entries(for: other).isEmpty)
        // A trashed task can still be queried by ID; a refresh must not reopen it.
        #expect(session.trashedConversationIDs.contains(other))

        await session.receive(
            Self.event(sequence: 13, conversationID: other, kind: "conversation.restored", payload: "{}"),
            from: local
        )
        #expect(!session.trashedConversationIDs.contains(other))
    }

    @Test
    func checkpointEntriesCarryTheirTurn() {
        let event = Self.event(
            sequence: 20, conversationID: UUID(), runID: UUID(), kind: "change.checkpoint_recorded",
            payload: #"{"turn":2,"outcome":"completed"}"#
        )
        let stamped = DesktopSession.stamped(event.timelineProjections(), from: event)

        #expect(stamped.count == 1)
        #expect(stamped.first?.checkpointTurn == 2)
        #expect(stamped.first?.runID == event.runID)
    }

#if DEBUG
    @Test
    func previewSessionsRenderTheLivePath() {
        let working = DesktopSession.preview { DesktopPreviewData.working($0) }
        guard case .working = working.selectedTaskStatus else {
            Issue.record("Expected a working status, got \(working.selectedTaskStatus)")
            return
        }
        #expect(!working.timeline.isEmpty)
        #expect(working.isPreviewSession)
        #expect(working.usesLivePlane)
        #expect(working.currentPhase == .editingFiles)
        #expect(working.canInterruptTurn)
        #expect(working.selectedAssistantName == "Claude Code")
        #expect(working.queuedEntry(forTimelineID: DesktopPreviewData.followUpTurnID.uuidString.lowercased())?.ordinal == 1)
        #expect(working.taskStatus(for: DesktopPreviewData.paymentDependencies.id) == .needsPermission)
        #expect(working.isUnread(DesktopPreviewData.conversations[2].id))
        #expect(working.taskStatus(for: DesktopPreviewData.conversations[5].id) == .unknown)
        #expect(working.needsYouConversationIDs == [DesktopPreviewData.paymentDependencies.id])

        let waiting = DesktopSession.preview { DesktopPreviewData.waitingWithChanges($0) }
        #expect(waiting.selectedTaskStatus == .waitingForReply)
        #expect(waiting.hasKnownChanges)
        #expect(waiting.canKeepChanges)

        let permission = DesktopSession.preview { DesktopPreviewData.needsPermission($0) }
        #expect(permission.selectedTaskStatus == .needsPermission)

        let offline = DesktopSession.preview { DesktopPreviewData.offline($0) }
        #expect(offline.selectedTaskStatus == .offline)
        #expect(offline.sendBlocker == .notConnected(computer: "This Mac"))
        #expect(offline.isComputerOffline(offline.localPlaneRegistryID))
        #expect(offline.taskStatus(for: DesktopPreviewData.paymentDependencies.id) == .offline)
    }
#endif

    // MARK: - Helpers

    nonisolated static let project = JetProjectSummary(id: UUID(), root: "/Users/alex/code/web-app")
    nonisolated static let claude = JetInstalledCraft(id: "jet-craft-claude", version: "1", harnesses: ["claude-code"])
    static let negotiation = JetNegotiation(
        protocolVersion: 1, minorVersion: 43, codec: "json-v1", frameLimits: .protocolMaximum
    )

    /// A session on the live path. A preview session never reaches a Plane; with
    /// `isPreviewSession: false`, every client request goes through `makeJetClient`.
    static func liveSession(
        isPreviewSession: Bool = true,
        makeJetClient: @escaping JetClientFactory = { throw CancellationError() }
    ) -> DesktopSession {
        let suite = "jet.tests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        return DesktopSession(
            makeJetClient: makeJetClient,
            notificationPreference: { _ in false },
            memory: ClientMemory(defaults: defaults),
            isPreviewSession: isPreviewSession
        )
    }

    static func connected(
        projects: [JetProjectSummary] = [project],
        crafts: [JetInstalledCraft] = [claude],
        isPreviewSession: Bool = true,
        makeJetClient: @escaping JetClientFactory = { throw CancellationError() }
    ) -> DesktopSession {
        let session = liveSession(isPreviewSession: isPreviewSession, makeJetClient: makeJetClient)
        let snapshot = JetSetupSnapshot(
            status: JetPlaneStatus(
                cursor: 1, planeID: UUID(), daemonStarts: 1, startedAtUnixMilliseconds: 1,
                coreVersion: "test", security: nil, recovery: nil
            ),
            capabilities: JetCapabilitySummary(
                coreVersion: "test",
                platform: "macos",
                externalTools: [JetExternalToolSummary(tool: "git", availability: .present(version: "2.50"))],
                harnesses: crafts.flatMap(\.harnesses),
                crafts: crafts,
                credentialStore: .available,
                degraded: []
            ),
            projects: JetProjectList(cursor: 1, projects: projects),
            accounts: JetAccountBindingList(cursor: 1, bindings: []),
            pairing: JetPairingSummary(cursor: 1, gate: "closed", pairedClients: 0, hasPendingOffer: false)
        )
        session.setupState = .ready(snapshot)
        session.connectionState = .connected(negotiation)
        session.updatePlane(session.localPlaneRegistryID) { plane in
            plane.connection = .connected(negotiation)
            plane.snapshot = snapshot
        }
        session.selectedProjectID = projects.first?.id
        return session
    }

    static func addTask(_ session: DesktopSession, _ title: String) -> JetConversationSummary {
        let conversation = JetConversationSummary(
            id: UUID(), revision: 1, title: title,
            createdAtUnixMilliseconds: Int64(session.conversations.count + 1), projectID: project.id
        )
        let local = session.localPlaneRegistryID
        session.planeConversations[local, default: []].append(conversation)
        session.conversationPlaneRegistryIDs[conversation.id] = local
        session.rebuildConversationAggregation()
        return conversation
    }

    static func run(for conversation: JetConversationSummary, lifecycle: JetRunLifecycle) -> JetRunSummary {
        JetRunSummary(
            id: UUID(), conversationID: conversation.id, revision: 1, lifecycle: lifecycle,
            title: conversation.title, createdAtUnixMilliseconds: 1,
            endedAtUnixMilliseconds: lifecycle.isLive ? nil : 2
        )
    }

    static func snapshot(_ conversation: JetConversationSummary, runs: [JetRunSummary]) -> JetConversationSnapshot {
        JetConversationSnapshot(
            cursor: 1, conversation: conversation, workspaceID: nil,
            workspaceRoot: "/tmp/working-copy", runs: runs
        )
    }

    /// Opens a new task whose latest Run has `lifecycle`, or no Runs when nil.
    @discardableResult
    static func openTask(
        _ session: DesktopSession,
        lifecycle: JetRunLifecycle?,
        activity: JetRunActivity? = nil
    ) -> JetConversationSummary {
        let conversation = addTask(session, "Task \(session.conversations.count + 1)")
        let runs = lifecycle.map { [run(for: conversation, lifecycle: $0)] } ?? []
        session.sidebarSelection = .conversation
        session.selectedConversationID = conversation.id
        session.conversationSnapshot = snapshot(conversation, runs: runs)
        session.runExecution = runs.first.map {
            JetRunExecution(cursor: 1, run: $0, activity: activity, needsAttention: false, termination: nil)
        }
        session.turnQueue = nil
        session.gitDeliveries = []
        session.conversationFreshness = .live
        return conversation
    }

    /// Records that a task's live Run waits for a permission.
    static func recordNeedsPermission(_ conversation: JetConversationSummary, in session: DesktopSession) {
        let activeRun = Self.run(for: conversation, lifecycle: .active)
        session.statusStore.record(
            snapshot: Self.snapshot(conversation, runs: [activeRun]),
            execution: JetRunExecution(
                cursor: 5, run: activeRun, activity: .waitingForApproval, needsAttention: true, termination: nil
            ),
            cursor: 5
        )
    }

    /// Sets This Mac's connection the way the client's connection stream does.
    static func setLocalConnection(_ state: JetConnectionState, in session: DesktopSession) {
        session.connectionState = state
        session.updatePlane(session.localPlaneRegistryID) { $0.connection = state }
    }

    static func unknownDelivery(for conversationID: UUID) -> JetGitDelivery {
        JetGitDelivery(
            id: UUID(),
            conversationID: conversationID,
            checkpoint: JetGitCheckpoint(runID: UUID(), turn: 1),
            operation: .push(remote: "origin"),
            policy: JetGitDeliveryPolicy(
                automatic: false, branch: true, commit: true, push: true,
                draftPullRequest: false, branchPrefix: "jet/"
            ),
            utilityJobID: nil,
            message: nil,
            acknowledgedBy: nil,
            outcome: .outcomeUnknown
        )
    }

    static func event(
        sequence: UInt64,
        conversationID: UUID,
        runID: UUID? = nil,
        kind: String,
        payload: String
    ) -> JetEvent {
        JetEvent(
            sequence: sequence,
            eventID: UUID(),
            actor: JetRawJSON(source: #"{"interactive_client":{}}"#),
            origin: nil,
            recordedAtUnixMilliseconds: 1_000,
            conversationID: conversationID,
            runID: runID,
            kind: kind,
            payloadVersion: 1,
            payload: JetRawJSON(source: payload)
        )
    }
}

/// Stands in for the client factory: every call fails as offline after
/// `onCall` runs, so a test can act between a request and its follow-up.
@MainActor
final class ClientFactoryProbe {
    private(set) var calls = 0
    var onCall: (Int) -> Void = { _ in }

    func makeClient() throws -> JetClient {
        calls += 1
        onCall(calls)
        throw JetClientFailure.presentation(.offline)
    }
}
