import Foundation
import SwiftUI
import Testing
@testable import jet

@MainActor
struct ComposerTests {
    // MARK: - Keys

    nonisolated enum Modifier: CaseIterable, Sendable {
        case none, shift, option, control, command

        var modifiers: EventModifiers {
            switch self {
            case .none: []
            case .shift: .shift
            case .option: .option
            case .control: .control
            case .command: .command
            }
        }
    }

    @Test(arguments: [false, true], Modifier.allCases)
    func returnKeyRouting(returnSends: Bool, modifier: Modifier) {
        for hasMarkedText in [false, true] {
            let action = ComposerKeyRouting.returnAction(
                returnSends: returnSends,
                modifiers: modifier.modifiers,
                hasMarkedText: hasMarkedText
            )
            let expected: ComposerReturnAction =
                !hasMarkedText && returnSends && modifier == .none ? .send : .passThrough
            #expect(action == expected, "returnSends \(returnSends), \(modifier), marked \(hasMarkedText)")
        }
    }

    @Test
    func capsLockAndFunctionKeysDontBlockSending() {
        #expect(ComposerKeyRouting.returnAction(
            returnSends: true, modifiers: [.capsLock, .numericPad], hasMarkedText: false
        ) == .send)
        #expect(ComposerKeyRouting.returnAction(
            returnSends: true, modifiers: [.shift, .capsLock], hasMarkedText: false
        ) == .passThrough)
    }

    // MARK: - Size, hint and caption

    @Test
    func sizeLimitThresholds() {
        #expect(ComposerSizeLimit.state(bytes: 0) == .hidden)
        #expect(ComposerSizeLimit.state(bytes: 52_428) == .hidden)
        #expect(ComposerSizeLimit.state(bytes: 52_429) == .near(52_429))
        #expect(ComposerSizeLimit.state(bytes: 65_536) == .near(65_536))
        #expect(ComposerSizeLimit.state(bytes: 65_537) == .over(65_537))
        #expect(ComposerSizeLimit.label(bytes: 70_000)
            == "\(JetCopy.number(70_000)) of \(JetCopy.number(65_536)) bytes")
    }

    @Test
    func placementMetrics() {
        #expect(ComposerPlacement.task.lineRange == 1 ... 8)
        #expect(ComposerPlacement.newTask.lineRange == 3 ... 8)
        #expect(ComposerPlacement.task.maxWidth == JetDesign.readingWidth)
        #expect(ComposerPlacement.newTask.maxWidth == JetDesign.writingWidth)
        #expect(ComposerPreferences.returnSendsKey == "jet.composer.return-sends")
    }

    @Test
    func hintText() {
        #expect(ComposerHint.text(returnSends: false, startsTask: false) == "⌘↩ Send")
        #expect(ComposerHint.text(returnSends: true, startsTask: false) == "↩ Send")
        #expect(ComposerHint.text(returnSends: false, startsTask: true) == "⌘↩ Start Task")
        #expect(ComposerHint.text(returnSends: true, startsTask: true) == "↩ Start Task")
    }

    @Test
    func sendHelpNamesTheShortcutOrTheReason() {
        #expect(ComposerHint.sendHelp(blocker: nil, returnSends: false, startsTask: false) == "Send (⌘↩)")
        #expect(ComposerHint.sendHelp(blocker: nil, returnSends: true, startsTask: false) == "Send (↩)")
        #expect(ComposerHint.sendHelp(blocker: nil, returnSends: false, startsTask: true) == "Start Task (⌘↩)")
        #expect(ComposerHint.sendHelp(blocker: nil, returnSends: true, startsTask: true) == "Start Task (↩)")
        #expect(ComposerHint.sendHelp(blocker: .empty, returnSends: false, startsTask: false) == "Type a message first.")
        #expect(ComposerHint.sendHelp(blocker: .busy, returnSends: false, startsTask: false)
            == "Wait until Jet finishes your last action.")
        #expect(ComposerHint.sendHelp(blocker: .tooLong, returnSends: false, startsTask: false)
            == "Message is too long.")
    }

    @Test
    func captionPriority() {
        // A blocker beats what sending does.
        #expect(ComposerCaption.resolve(
            blocker: .queueFull,
            sessionCaption: "Sends after the current reply finishes.",
            placement: .task,
            checklistVisible: false,
            newTaskProjectName: nil
        ) == ComposerCaption(tone: .warning, text: "Too many messages are waiting. Remove one or wait."))

        #expect(ComposerCaption.resolve(
            blocker: .notConnected(computer: "This Mac"),
            sessionCaption: nil,
            placement: .task,
            checklistVisible: false,
            newTaskProjectName: nil
        ) == ComposerCaption(
            tone: .warning,
            text: "Not connected to This Mac. Your message is kept.",
            action: .tryAgainConnection
        ))

        #expect(ComposerCaption.resolve(
            blocker: .tooLong, sessionCaption: nil, placement: .task, checklistVisible: false, newTaskProjectName: nil
        )?.tone == .error)
        #expect(ComposerCaption.resolve(
            blocker: .gettingReady, sessionCaption: nil, placement: .newTask, checklistVisible: false, newTaskProjectName: nil
        )?.tone == .progress)
        #expect(ComposerCaption.resolve(
            blocker: .gitStepUnconfirmed, sessionCaption: nil, placement: .task, checklistVisible: false, newTaskProjectName: nil
        ) == ComposerCaption(
            tone: .warning,
            text: "Jet paused this task until you check a Git step.",
            action: .reviewGitStep
        ))

        // Without a blocker message, the session's caption shows plainly.
        #expect(ComposerCaption.resolve(
            blocker: .empty,
            sessionCaption: "Claude Code continues where it left off.",
            placement: .task,
            checklistVisible: false,
            newTaskProjectName: nil
        ) == ComposerCaption(tone: .plain, text: "Claude Code continues where it left off."))
        #expect(ComposerCaption.resolve(
            blocker: .busy, sessionCaption: nil, placement: .task, checklistVisible: false, newTaskProjectName: nil
        ) == nil)
    }

    nonisolated static let coveredBlockers: [SendBlocker] = [
        .notConnected(computer: "This Mac"), .gettingReady, .noProject, .noAssistant,
    ]

    @Test(arguments: coveredBlockers)
    func checklistCoveredBlockersAreQuietOnNewTask(_ blocker: SendBlocker) {
        let separateCopy = ComposerCaption(
            tone: .plain,
            text: "Jet works in a separate copy of web-app. Your folder doesn't change until you keep the changes."
        )
        #expect(ComposerCaption.resolve(
            blocker: blocker,
            sessionCaption: blocker.message,
            placement: .newTask,
            checklistVisible: true,
            newTaskProjectName: "web-app"
        ) == separateCopy)
        #expect(ComposerCaption.resolve(
            blocker: blocker,
            sessionCaption: blocker.message,
            placement: .newTask,
            checklistVisible: true,
            newTaskProjectName: nil
        ) == ComposerCaption(
            tone: .plain,
            text: "Jet works in a separate copy of your project. Your folder doesn't change until you keep the changes."
        ))
        // A task has no checklist, so the blocker shows.
        #expect(ComposerCaption.resolve(
            blocker: blocker,
            sessionCaption: nil,
            placement: .task,
            checklistVisible: false,
            newTaskProjectName: nil
        )?.text == blocker.message)
    }

    @Test
    func tooLongStillShowsOnNewTaskWithTheChecklist() {
        #expect(ComposerCaption.resolve(
            blocker: .tooLong,
            sessionCaption: nil,
            placement: .newTask,
            checklistVisible: true,
            newTaskProjectName: "web-app"
        ) == ComposerCaption(tone: .error, text: "Message is too long."))
    }

    // MARK: - Notice slot

    @Test
    func noticeSlotPriority() {
        let error = ComposerNotice(kind: .error, text: "Jet couldn't send that.")
        let warning = ComposerNotice(kind: .warning, text: "Can't reach This Mac right now.", action: .tryAgainConnection)
        let info = ComposerNotice(kind: .info, text: "That task is no longer available.")
        let confirmation = ComposerNotice(kind: .confirmation, text: "Removed.")
        let status = ComposerNotice(
            kind: .warning,
            text: "Usage limit reached. Send a message after it resets.",
            action: .showUsage
        )

        #expect(ComposerNoticeSlot.resolve(notice: error, statusNotice: status, offerEligible: true) == .notice(error))
        #expect(ComposerNoticeSlot.resolve(notice: warning, statusNotice: status, offerEligible: true) == .notice(warning))
        #expect(ComposerNoticeSlot.resolve(notice: info, statusNotice: status, offerEligible: true) == .notice(status))
        #expect(ComposerNoticeSlot.resolve(notice: confirmation, statusNotice: nil, offerEligible: true)
            == .notice(confirmation))
        #expect(ComposerNoticeSlot.resolve(notice: nil, statusNotice: nil, offerEligible: true) == .notificationOffer)
        #expect(ComposerNoticeSlot.resolve(notice: nil, statusNotice: nil, offerEligible: false) == nil)
    }

    @Test
    func statusNoticeCopy() {
        let signIn = Self.connected()
        Self.openTask(signIn, activity: .waitingForAuth)
        #expect(signIn.statusNotice == ComposerNotice(
            kind: .warning,
            text: "Sign in to Claude Code on this Mac, then send a message to continue.",
            action: .openSettings(.agents)
        ))
        #expect(signIn.statusNotice?.action?.title == "Assistant Settings…")

        let usage = Self.connected()
        Self.openTask(usage, activity: .waitingForQuota)
        #expect(usage.statusNotice == ComposerNotice(
            kind: .warning,
            text: "Usage limit reached. Send a message after it resets.",
            action: .showUsage
        ))

        let working = Self.connected()
        Self.openTask(working, activity: .working)
        #expect(working.statusNotice == nil)
    }

    // MARK: - Notification offer

    @Test
    func notificationOfferEligibility() {
        #expect(NotificationOffer.isEligible(
            offerShown: false, authorization: .notDetermined, anyPreferenceOn: false, startedHere: true
        ))
        #expect(NotificationOffer.isEligible(
            offerShown: false, authorization: .authorized, anyPreferenceOn: false, startedHere: true
        ))
        #expect(!NotificationOffer.isEligible(
            offerShown: true, authorization: .notDetermined, anyPreferenceOn: false, startedHere: true
        ))
        #expect(!NotificationOffer.isEligible(
            offerShown: false, authorization: .denied, anyPreferenceOn: false, startedHere: true
        ))
        #expect(!NotificationOffer.isEligible(
            offerShown: false, authorization: .notDetermined, anyPreferenceOn: true, startedHere: true
        ))
        #expect(!NotificationOffer.isEligible(
            offerShown: false, authorization: .notDetermined, anyPreferenceOn: false, startedHere: false
        ))
    }

    @Test(arguments: [true, false])
    func turningNotificationsOnWritesPreferencesOnlyWhenGranted(_ granted: Bool) async {
        let notifications = FakeNotifications(result: granted ? .authorized : .denied)
        let session = Self.connected(isPreviewSession: false, notifications: notifications)
        Self.openTask(session, activity: .waitingForUser)
        let defaults = Self.defaults()

        await NotificationOffer.turnOn(session: session, defaults: defaults)

        #expect(session.memory.notificationOfferShown)
        #expect(notifications.requests == 1)
        for key in NotificationOffer.preferenceKeys {
            #expect(defaults.bool(forKey: key) == granted)
        }
        #expect(session.composerNotice == (granted
            ? ComposerNotice(kind: .confirmation, text: "Notifications are on. You can change them in Settings.")
            : ComposerNotice(kind: .info, text: "Notifications for Jet are off in System Settings.")))
    }

    @Test
    func previewSessionsNeverRecordTheOffer() async {
        let preview = Self.connected()
        NotificationOffer.decline(session: preview)
        await NotificationOffer.turnOn(session: preview, defaults: Self.defaults())
        #expect(!preview.memory.notificationOfferShown)

        let live = Self.connected(isPreviewSession: false)
        NotificationOffer.decline(session: live)
        #expect(live.memory.notificationOfferShown)
    }

    // MARK: - Pickers

    @Test
    func assistantLabelsAddTheVersionOnlyWhenNamesCollide() {
        let labels = ComposerAssistantChoices.labels(for: [
            JetInstalledCraft(id: "claude-a", version: "1.2", harnesses: ["claude-code"]),
            JetInstalledCraft(id: "claude-b", version: "2.0", harnesses: ["claude-code"]),
            JetInstalledCraft(id: "codex", version: "0.9", harnesses: ["codex"]),
        ])
        #expect(labels == [
            "claude-a": "Claude Code (1.2)",
            "claude-b": "Claude Code (2.0)",
            "codex": "Codex",
        ])
    }

    @Test
    func choosingAProjectNeverNavigates() {
        let session = Self.connected(projects: [Self.webApp, Self.billing])
        session.open(.newTask)

        session.chooseNewTaskProject(Self.billing.id, on: session.localPlaneRegistryID)

        #expect(session.sidebarSelection == .newTask)
        #expect(session.selectedProjectID == Self.billing.id)
        #expect(session.newTaskPlaneRegistryID == session.localPlaneRegistryID)

        // A project the computer doesn't list is ignored.
        session.chooseNewTaskProject(UUID(), on: session.localPlaneRegistryID)
        #expect(session.selectedProjectID == Self.billing.id)
    }

    @Test
    func choosingAnotherComputerSaysWhichProjectItUses() {
        let session = Self.connected()
        let studio = Self.addStudioMac(session)
        session.open(.newTask)

        session.chooseNewTaskComputer(studio)

        #expect(session.newTaskPlaneRegistryID == studio)
        #expect(session.selectedProjectID == Self.apiServer.id)
        #expect(session.composerNotice == ComposerNotice(
            kind: .confirmation,
            text: "Project changed to api-server on Studio Mac."
        ))
        #expect(session.sidebarSelection == .newTask)
    }

    @Test
    func startingANewTaskInAProjectOpensNewTaskAndFocusesTheComposer() {
        let session = Self.connected(projects: [Self.webApp, Self.billing])
        Self.openTask(session, activity: .waitingForUser)
        let focus = session.composerFocusRequest

        session.startNewTask(in: Self.billing.id, on: session.localPlaneRegistryID, notice: "Added “billing”.")

        #expect(session.sidebarSelection == .newTask)
        #expect(session.selectedConversationID == nil)
        #expect(session.selectedProjectID == Self.billing.id)
        #expect(session.composerNotice == ComposerNotice(kind: .confirmation, text: "Added “billing”."))
        #expect(session.composerFocusRequest == focus + 1)
    }

    @Test
    func startingANewTaskWaitsForAnUnsavedEdit() {
        let session = Self.connected(projects: [Self.webApp, Self.billing])
        Self.openTask(session, activity: .waitingForUser)
        session.selectedProjectID = Self.webApp.id
        session.editableFile = JetEditableFile(
            cursor: 1,
            target: .workspace(UUID()),
            path: "src/login.ts",
            content: "old",
            revision: JetFileRevision(object: "abc", mode: "100644")
        )
        session.fileDraft = "new"

        session.startNewTask(in: Self.billing.id, on: session.localPlaneRegistryID)

        // Cancel keeps both the task and the project.
        #expect(session.pendingNavigation != nil)
        #expect(session.selectedProjectID == Self.webApp.id)
        #expect(session.sidebarSelection == .conversation)
    }

    // MARK: - New Task checklist

    static func inputs(
        isLocal: Bool = true,
        helper: NewTaskChecklist.Helper = .running,
        hasConversations: Bool = false,
        projects: NewTaskChecklist.Choice = .chosen("web-app"),
        assistants: NewTaskChecklist.Choice = .chosen("Claude Code")
    ) -> NewTaskChecklist.Inputs {
        NewTaskChecklist.Inputs(
            computerName: isLocal ? "This Mac" : "Studio Mac",
            isLocal: isLocal,
            helper: helper,
            hasConversations: hasConversations,
            projects: projects,
            assistants: assistants
        )
    }

    @Test
    func checklistWhileJetStarts() {
        let rows = NewTaskChecklist.make(Self.inputs(helper: .starting))
        #expect(rows[0] == NewTaskChecklist.Row(
            id: .helper,
            title: "Jet on this Mac",
            detail: "Starting Jet…",
            note: "Jet runs a small helper in the background so tasks keep going when this window is closed.",
            state: .inProgress
        ))
        #expect(rows[1] == NewTaskChecklist.Row(
            id: .project, title: "Project", detail: "Available once Jet is running.", state: .pending
        ))
        #expect(rows[2] == NewTaskChecklist.Row(
            id: .assistant, title: "Assistant", detail: "Available once Jet is running.", state: .pending
        ))

        // The helper note is for first runs only.
        #expect(NewTaskChecklist.make(Self.inputs(helper: .starting, hasConversations: true))[0].note == nil)
    }

    @Test
    func checklistWhileARetryIsScheduled() {
        let rows = NewTaskChecklist.make(Self.inputs(helper: .retrying(attempt: 2, limit: 3)))
        #expect(rows[0].detail == "Reconnecting… (attempt 2 of 3)")
        #expect(rows[0].state == .inProgress)
        #expect(NewTaskChecklist.make(Self.inputs(helper: .reconnecting))[0].detail == "Reconnecting…")
    }

    @Test
    func checklistForAnIncompleteInstallOffersReinstall() {
        let error = Self.helperError("core.install_incomplete")
        let row = NewTaskChecklist.make(Self.inputs(helper: .failed(error)))[0]
        #expect(row == NewTaskChecklist.Row(
            id: .helper,
            title: "Jet on this Mac",
            detail: "Jet couldn't start its helper.",
            note: "Jet's helper is incomplete. Reinstall Jet from its latest release.",
            state: .failed,
            actions: [.reinstall, .tryAgain],
            errorCode: "core.install_incomplete"
        ))
        #expect(NewTaskChecklist.reinstallURL?.absoluteString == "https://github.com/apexgang/jet/releases/latest")
    }

    @Test
    func checklistForAnotherCopyOfJet() {
        let row = NewTaskChecklist.make(Self.inputs(helper: .failed(Self.helperError("core.owned_by_other_channel"))))[0]
        #expect(row.note == "Another copy of Jet already runs this Mac's helper.")
        #expect(row.actions == [.tryAgain])
        #expect(row.errorCode == "core.owned_by_other_channel")
    }

    @Test
    func checklistHelperReasons() {
        #expect(NewTaskChecklist.helperFailureReason(.offline) == "It isn't responding.")
        #expect(NewTaskChecklist.helperFailureReason(Self.helperError("core.start_failed")) == nil)
    }

    @Test
    func checklistWithoutProjects() {
        let local = NewTaskChecklist.make(Self.inputs(projects: .none))[1]
        #expect(local == NewTaskChecklist.Row(
            id: .project,
            title: "Project",
            detail: "Choose the Git repository you want to work on.",
            state: .needsAction,
            actions: [.addProject],
            actionHint: "or drop a folder here"
        ))
        // Folders can't be dropped onto another computer.
        #expect(NewTaskChecklist.make(Self.inputs(isLocal: false, projects: .none))[1].actionHint == nil)

        #expect(NewTaskChecklist.make(Self.inputs(projects: .notChosen))[1].detail == "Choose a project in the menu below.")
        let failed = NewTaskChecklist.make(Self.inputs(projects: .failed))[1]
        #expect(failed.detail == "Couldn't load your projects.")
        #expect(failed.actions == [.reloadProjects])
        #expect(failed.state == .failed)
    }

    @Test
    func checklistWithoutAssistants() {
        let row = NewTaskChecklist.make(Self.inputs(assistants: .none))[2]
        #expect(row == NewTaskChecklist.Row(
            id: .assistant,
            title: "Assistant",
            detail: "No coding assistant found. Jet works with Claude Code and Codex.",
            state: .needsAction,
            actions: [.installationHelp, .checkAssistantsAgain]
        ))
        let failed = NewTaskChecklist.make(Self.inputs(assistants: .failed))[2]
        #expect(failed.detail == "Couldn't check for coding assistants.")
        #expect(failed.actions == [.checkAssistantsAgain])
    }

    @Test
    func checklistForAnotherComputer() {
        let offline = NewTaskChecklist.make(Self.inputs(isLocal: false, helper: .failed(.offline)))[0]
        #expect(offline == NewTaskChecklist.Row(
            id: .helper,
            title: "Jet on Studio Mac",
            detail: "Can't reach Studio Mac.",
            state: .failed,
            actions: [.tryAgain]
        ))
        #expect(NewTaskChecklist.make(Self.inputs(isLocal: false, helper: .starting))[0].detail
            == "Connecting to Studio Mac…")
        #expect(NewTaskChecklist.make(Self.inputs(isLocal: false))[0].detail == "Connected")
    }

    @Test
    func checklistWhenEverythingIsReady() {
        let rows = NewTaskChecklist.make(Self.inputs())
        #expect(rows.map(\.state) == [.done, .done, .done])
        #expect(rows.map(\.detail) == ["Jet is running", "web-app", "Claude Code"])
    }

    @Test
    func localHelperShowsARetryCountOnlyForAScheduledRetry() {
        func helper(
            _ connection: JetConnectionState,
            _ setup: DesktopSession.SetupState,
            attempt: Int = 0,
            failure: JetPresentationError? = nil
        ) -> NewTaskChecklist.Helper {
            NewTaskChecklist.Inputs.localHelper(
                connection: connection,
                setupState: setup,
                retryAttempt: attempt,
                retryLimit: 3,
                planeFailure: failure
            )
        }
        let failed = Self.helperError("core.start_failed")
        #expect(helper(.reconnecting(attempt: 2), .failed(failed), attempt: 2) == .retrying(attempt: 2, limit: 3))
        #expect(helper(.reconnecting(attempt: 1), .ready(Self.snapshot())) == .reconnecting)
        #expect(helper(.failed(failed), .failed(failed)) == .failed(failed))
        #expect(helper(.connected(Self.negotiation), .ready(Self.snapshot())) == .running)
        #expect(helper(.connected(Self.negotiation), .loading) == .starting)
        #expect(helper(.connecting, .loading) == .starting)
        #expect(helper(.disconnected, .idle) == .starting)
        #expect(helper(.disconnected, .ready(Self.snapshot()), failure: .offline) == .failed(.offline))
    }

    @Test
    func checklistInputsFromTheSession() {
        let session = Self.connected()
        session.open(.newTask)
        #expect(NewTaskChecklist.Inputs.from(session) == Self.inputs(hasConversations: false))
        #expect(session.canStartTask)

        let empty = Self.connected(projects: [], crafts: [])
        empty.open(.newTask)
        let inputs = NewTaskChecklist.Inputs.from(empty)
        #expect(inputs.projects == .none)
        #expect(inputs.assistants == .none)
    }

    // MARK: - Examples

    @Test
    func examplesAreEditableText() {
        #expect(NewTaskView.examples.map(\.title) == [
            "Explain how this project is organized",
            "Fix a failing test",
            "Find the cause of a bug",
        ])
        for example in NewTaskView.examples {
            #expect(!example.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }

        let session = Self.connected()
        session.open(.newTask)
        session.insertExample(NewTaskView.examples[1].prompt)
        #expect(session.draft == "Find the failing test and fix it. The test is: ")
        #expect(session.userOperation == nil)
    }

    // MARK: - Add Project model

    @Test
    func aRegistrableFolderIsReviewed() async {
        let fake = FakeProjects(previews: ["/code/mobile-app": Self.registrable("/code/mobile-app")])
        let model = fake.model()

        await model.check(path: "/code/mobile-app")

        #expect(model.phase == .reviewed(Self.registrable("/code/mobile-app")))
        #expect(fake.previewedPaths == ["/code/mobile-app"])
    }

    @Test
    func aKnownRootIsAlreadyAdded() async {
        let fake = FakeProjects(
            previews: ["/code/web-app/": Self.registrable("/code/web-app/")],
            existing: [Self.project("/code/web-app")]
        )
        let model = fake.model()

        await model.check(path: "/code/web-app/")
        #expect(model.phase == .alreadyAdded(Self.project("/code/web-app")))

        await model.add()
        #expect(fake.registrations.isEmpty)
    }

    @Test
    func useSuggestedRootPreviewsTheRepository() async {
        let inside = JetProjectPreview(
            root: "/code/mobile-app/src",
            registrability: .unavailable(verdict: "inside_working_tree", detail: ""),
            suggestedRoot: "/code/mobile-app"
        )
        let fake = FakeProjects(previews: [
            "/code/mobile-app/src": inside,
            "/code/mobile-app": Self.registrable("/code/mobile-app"),
        ])
        let model = fake.model()

        await model.check(path: "/code/mobile-app/src")
        #expect(model.phase == .reviewed(inside))
        await model.useSuggestedRoot()

        #expect(fake.previewedPaths == ["/code/mobile-app/src", "/code/mobile-app"])
        #expect(model.phase == .reviewed(Self.registrable("/code/mobile-app")))
    }

    @Test
    func anUncertainRegistrationKeepsItsCommandID() async {
        let fake = FakeProjects(previews: ["/code/mobile-app": Self.registrable("/code/mobile-app")])
        fake.registerResults = [
            .failure(JetClientFailure.commandOutcomeUnknown(commandID: UUID())),
            .success(Self.project("/code/mobile-app")),
        ]
        let model = fake.model()
        await model.check(path: "/code/mobile-app")
        let commandID = model.registrationCommandID

        await model.add()
        #expect(model.phase == .unconfirmed(Self.registrable("/code/mobile-app")))

        // Not listed yet: back to the review with the same Command.
        await model.checkAgain()
        #expect(fake.refreshes == 1)
        #expect(model.phase == .reviewed(Self.registrable("/code/mobile-app")))
        #expect(model.registrationCommandID == commandID)

        await model.add()
        #expect(fake.registrations.map(\.commandID) == [commandID, commandID])
        #expect(model.phase == .added(Self.project("/code/mobile-app")))
    }

    @Test
    func checkAgainFindsAnAddedProject() async {
        let fake = FakeProjects(previews: ["/code/mobile-app": Self.registrable("/code/mobile-app")])
        fake.registerResults = [.failure(JetClientFailure.commandOutcomeUnknown(commandID: UUID()))]
        let model = fake.model()
        await model.check(path: "/code/mobile-app")
        await model.add()

        fake.existing = [Self.project("/code/mobile-app")]
        await model.checkAgain()

        #expect(model.phase == .added(Self.project("/code/mobile-app")))
        #expect(fake.registrations.count == 1)
    }

    @Test
    func aNewFolderGetsAFreshCommandID() async {
        let fake = FakeProjects(previews: [
            "/code/a": Self.registrable("/code/a"),
            "/code/b": Self.registrable("/code/b"),
        ])
        let model = fake.model()

        await model.check(path: "/code/a")
        let first = model.registrationCommandID
        await model.check(path: "/code/a")
        #expect(model.registrationCommandID == first)
        await model.check(path: "/code/b")
        #expect(model.registrationCommandID != first)
    }

    @Test
    func aStalePreviewIsIgnored() async {
        let fake = FakeProjects(previews: [
            "/code/slow": Self.registrable("/code/slow"),
            "/code/fast": Self.registrable("/code/fast"),
        ])
        fake.heldPaths = ["/code/slow"]
        let model = fake.model()

        let slow = Task { await model.check(path: "/code/slow") }
        while fake.held.isEmpty { await Task.yield() }
        await model.check(path: "/code/fast")
        #expect(model.phase == .reviewed(Self.registrable("/code/fast")))

        fake.releaseHeld()
        await slow.value
        #expect(model.phase == .reviewed(Self.registrable("/code/fast")))
    }

    @Test
    func aRelativeRemotePathNeverReachesTheComputer() async {
        let fake = FakeProjects(previews: [:])
        let model = fake.model()
        model.remotePath = "code/api-server"

        await model.checkRemotePath()

        #expect(model.phase == .choosing(message: "Enter a full path that starts with /."))
        #expect(fake.previewedPaths.isEmpty)

        model.remotePath = "  /srv/api-server "
        fake.previews["/srv/api-server"] = Self.registrable("/srv/api-server")
        await model.checkRemotePath()
        #expect(fake.previewedPaths == ["/srv/api-server"])
    }

    @Test
    func anAlreadyRegisteredAnswerRefreshesTheList() async {
        let fake = FakeProjects(previews: ["/code/mobile-app": Self.registrable("/code/mobile-app")])
        fake.registerResults = [.failure(JetClientFailure.presentation(Self.projectError("project.already_registered")))]
        fake.existingAfterRefresh = [Self.project("/code/mobile-app")]
        let model = fake.model()
        await model.check(path: "/code/mobile-app")

        await model.add()

        #expect(fake.refreshes == 1)
        #expect(model.phase == .alreadyAdded(Self.project("/code/mobile-app")))
    }

    @Test
    func aDefiniteRefusalChecksTheFolderAgainWithANewCommand() async {
        let preview = Self.registrable("/code/mobile-app")
        let fake = FakeProjects(previews: ["/code/mobile-app": preview])
        let refusal = Self.projectError("project.not_a_repository")
        fake.registerResults = [.failure(JetClientFailure.presentation(refusal))]
        let model = fake.model()
        await model.check(path: "/code/mobile-app")
        let first = model.registrationCommandID

        await model.add()
        #expect(model.phase == .failed(preview, refusal))

        await model.retry()
        #expect(fake.previewedPaths == ["/code/mobile-app", "/code/mobile-app"])
        #expect(model.phase == .reviewed(preview))
        #expect(model.registrationCommandID != first)
    }

    @Test
    func aTransientFailureResendsTheSameCommand() async {
        let preview = Self.registrable("/code/mobile-app")
        let fake = FakeProjects(previews: ["/code/mobile-app": preview])
        fake.registerResults = [
            .failure(JetClientFailure.presentation(.offline)),
            .success(Self.project("/code/mobile-app")),
        ]
        let model = fake.model()
        await model.check(path: "/code/mobile-app")

        await model.add()
        #expect(model.phase == .failed(preview, .offline))
        await model.retry()

        #expect(fake.registrations.count == 2)
        #expect(fake.registrations[0].commandID == fake.registrations[1].commandID)
        #expect(model.phase == .added(Self.project("/code/mobile-app")))
    }

    @Test
    func choosingAnotherComputerStartsOver() async {
        let fake = FakeProjects(previews: ["/code/a": Self.registrable("/code/a")])
        let model = fake.model()
        await model.check(path: "/code/a")

        model.choosePlane(UUID())

        #expect(model.phase == .choosing(message: nil))
    }

    // MARK: - Add Project copy

    @Test
    func addProjectCopyForEveryVerdict() {
        func review(_ verdict: String?, root: String = "/Users/alex/Downloads", suggested: String? = nil, isLocal: Bool = true) -> AddProjectCopy {
            let registrability: JetProjectRegistrability = verdict.map { .unavailable(verdict: $0, detail: "") }
                ?? .registrable(detail: "")
            return AddProjectCopy.review(
                JetProjectPreview(root: root, registrability: registrability, suggestedRoot: suggested),
                computerName: "Studio Mac",
                isLocal: isLocal
            )
        }

        let add = review(nil, root: "/Users/alex/code/web-app")
        #expect(add == AddProjectCopy(
            title: "Add “web-app”?",
            message: "Tasks in this project work in a separate copy. Your folder doesn't change until you keep the changes.",
            showsPath: true,
            primary: .add
        ))
        #expect(add.primaryTitle == "Add Project")
        #expect(review(nil, root: "/srv/api", isLocal: false).title == "Add “api” on Studio Mac?")

        let inside = review("inside_working_tree", root: "/code/web-app/src", suggested: "/code/web-app")
        #expect(inside.title == "This folder is inside “web-app”.")
        #expect(inside.message == "Jet adds the whole repository.")
        #expect(inside.primary == .useRepository(root: "/code/web-app"))
        #expect(inside.primaryTitle == "Use web-app")

        let notGit = review("not_a_repository")
        #expect(notGit.title == "“Downloads” isn't a Git repository.")
        #expect(notGit.message == "Jet needs Git so your changes can be kept safely.")
        #expect(notGit.primaryTitle == "Choose Another Folder…")

        let bare = review("bare_repository")
        #expect(bare.title == "“Downloads” is a bare repository.")
        #expect(bare.message == "Choose a folder that has the project's files checked out.")
        #expect(bare.primary == .chooseAnotherFolder)

        let gitDir = review("inside_git_dir")
        #expect(gitDir.title == "This folder is inside Git's own data.")
        #expect(gitDir.message == "Choose the repository folder instead.")
        #expect(gitDir.primary == .chooseAnotherFolder)

        let broken = review("broken_repository")
        #expect(broken.title == "Git can't open “Downloads”.")
        #expect(broken.message == "Check the repository, then try again.")
        #expect(broken.primaryTitle == "Try Again")
    }

    @Test
    func addProjectFailureCopy() {
        #expect(AddProjectCopy.failure(.offline, computerName: "Studio Mac") == "Can't reach Studio Mac.")
        #expect(AddProjectCopy.failure(Self.projectError("project.path_invalid"), computerName: "Studio Mac")
            == "Enter a full path that starts with /.")
        for code in AddProjectCopy.changedFolderCodes {
            #expect(AddProjectCopy.failure(Self.projectError(code), computerName: "This Mac")
                == "The folder changed since Jet checked it. Choose it again.")
            #expect(!AddProjectCopy.showsDetails(Self.projectError(code)))
        }
        let other = Self.projectError("project.protected_root")
        #expect(AddProjectCopy.failure(other, computerName: "This Mac") == "Jet couldn't add this project.")
        #expect(AddProjectCopy.showsDetails(other))
        #expect(!AddProjectCopy.showsDetails(.offline))
    }

    // MARK: - Copy lint

    @Test
    func casualCopyAvoidsJetWords() {
        var copy: [String] = []
        let helpers: [NewTaskChecklist.Helper] = [
            .starting, .retrying(attempt: 2, limit: 3), .reconnecting, .running,
            .failed(Self.helperError("core.install_incomplete")),
            .failed(Self.helperError("core.owned_by_other_channel")),
            .failed(.offline),
        ]
        let choices: [NewTaskChecklist.Choice] = [.unknown, .failed, .none, .notChosen, .chosen("web-app")]
        for isLocal in [true, false] {
            for helper in helpers {
                for choice in choices {
                    let rows = NewTaskChecklist.make(Self.inputs(
                        isLocal: isLocal, helper: helper, projects: choice, assistants: choice
                    ))
                    for row in rows {
                        copy += [row.title, row.detail] + [row.note, row.actionHint].compactMap { $0 }
                        copy += row.actions.map(\.title)
                    }
                }
            }
        }
        copy += NewTaskView.examples.flatMap { [$0.title, $0.prompt] }
        for verdict in [nil, "inside_working_tree", "not_a_repository", "bare_repository", "inside_git_dir", "broken_repository"] {
            let registrability: JetProjectRegistrability = verdict.map { .unavailable(verdict: $0, detail: "") }
                ?? .registrable(detail: "")
            for isLocal in [true, false] {
                let review = AddProjectCopy.review(
                    JetProjectPreview(root: "/code/web-app", registrability: registrability, suggestedRoot: "/code"),
                    computerName: "Studio Mac",
                    isLocal: isLocal
                )
                copy += [review.title, review.message, review.primaryTitle]
            }
        }
        for code in ["transport.offline", "project.path_invalid", "project.not_a_repository", "project.other"] {
            copy.append(AddProjectCopy.failure(Self.projectError(code), computerName: "This Mac"))
        }
        for returnSends in [false, true] {
            for startsTask in [false, true] {
                copy.append(ComposerHint.text(returnSends: returnSends, startsTask: startsTask))
                copy.append(ComposerHint.sendHelp(blocker: nil, returnSends: returnSends, startsTask: startsTask))
            }
        }
        copy += [
            "Get a notification when a reply is ready or a task needs you?",
            "Notifications are on. You can change them in Settings.",
            "Notifications for Jet are off in System Settings.",
            "Not Now",
        ]

        for text in copy {
            #expect(JetCopy.foundAvoidWords(in: text).isEmpty, "\(text)")
        }
        // "Turn On" is a button phrase, not the Turn of a conversation.
        #expect(JetCopy.foundAvoidWords(in: "Turn On") == ["Turn"])
    }

    // MARK: - Helpers

    nonisolated static let webApp = JetProjectSummary(id: UUID(), root: "/Users/alex/code/web-app")
    nonisolated static let billing = JetProjectSummary(id: UUID(), root: "/Users/alex/code/billing")
    nonisolated static let apiServer = JetProjectSummary(id: UUID(), root: "/srv/api-server")
    nonisolated static let claude = JetInstalledCraft(id: "jet-craft-claude", version: "1", harnesses: ["claude-code"])
    static let negotiation = JetNegotiation(
        protocolVersion: 1, minorVersion: 43, codec: "json-v1", frameLimits: .protocolMaximum
    )

    static func defaults() -> UserDefaults {
        let suite = "jet.tests.composer.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        return defaults
    }

    static func snapshot(
        projects: [JetProjectSummary] = [webApp],
        crafts: [JetInstalledCraft] = [claude]
    ) -> JetSetupSnapshot {
        JetSetupSnapshot(
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
    }

    /// A live-path session connected to This Mac.
    static func connected(
        projects: [JetProjectSummary] = [webApp],
        crafts: [JetInstalledCraft] = [claude],
        isPreviewSession: Bool = true,
        notifications: (any JetNotificationDelivering)? = nil
    ) -> DesktopSession {
        let session = DesktopSession(
            makeJetClient: { throw CancellationError() },
            notifications: notifications,
            notificationPreference: { _ in false },
            memory: ClientMemory(defaults: defaults()),
            isPreviewSession: isPreviewSession
        )
        let snapshot = snapshot(projects: projects, crafts: crafts)
        session.setupState = .ready(snapshot)
        session.connectionState = .connected(negotiation)
        session.updatePlane(session.localPlaneRegistryID) { plane in
            plane.connection = .connected(negotiation)
            plane.snapshot = snapshot
        }
        session.selectedProjectID = projects.first?.id
        return session
    }

    @discardableResult
    static func addStudioMac(_ session: DesktopSession) -> UUID {
        let id = UUID()
        session.planes.append(JetPlanePresentation(
            id: id,
            name: "Studio Mac",
            endpoint: "alex@studio.local",
            isLocal: false,
            planeID: UUID(),
            connection: .connected(negotiation),
            snapshot: snapshot(projects: [apiServer]),
            failure: nil,
            conversationCursor: 1
        ))
        return id
    }

    /// Opens a task started here with Claude Code whose live Run has `activity`.
    static func openTask(_ session: DesktopSession, activity: JetRunActivity) {
        let conversation = JetConversationSummary(
            id: UUID(), revision: 1, title: "Fix login redirect loop",
            createdAtUnixMilliseconds: 1, projectID: webApp.id
        )
        let local = session.localPlaneRegistryID
        session.planeConversations[local, default: []].append(conversation)
        session.conversationPlaneRegistryIDs[conversation.id] = local
        session.rebuildConversationAggregation()
        let run = JetRunSummary(
            id: UUID(), conversationID: conversation.id, revision: 1, lifecycle: .active,
            title: conversation.title, createdAtUnixMilliseconds: 1, endedAtUnixMilliseconds: nil
        )
        session.sidebarSelection = .conversation
        session.selectedConversationID = conversation.id
        session.conversationSnapshot = JetConversationSnapshot(
            cursor: 1, conversation: conversation, workspaceID: nil,
            workspaceRoot: "/tmp/working-copy", runs: [run]
        )
        session.runExecution = JetRunExecution(
            cursor: 1, run: run, activity: activity, needsAttention: false, termination: nil
        )
        session.conversationFreshness = .live
        session.memory.recordAssistant(claude.id, for: conversation.id)
    }

    static func helperError(_ code: String) -> JetPresentationError {
        JetPresentationError(category: .unavailable, code: code, message: "", retryable: false)
    }

    static func projectError(_ code: String) -> JetPresentationError {
        JetPresentationError(category: .invalidInput, code: code, message: "", retryable: false)
    }

    static func registrable(_ root: String) -> JetProjectPreview {
        JetProjectPreview(root: root, registrability: .registrable(detail: "main working tree"))
    }

    /// Projects keep a stable ID per root so complete values compare.
    static func project(_ root: String) -> JetProjectSummary {
        JetProjectSummary(id: stableID(root), root: root)
    }

    private static func stableID(_ text: String) -> UUID {
        var bytes = [UInt8](repeating: 0, count: 16)
        for (index, byte) in text.utf8.enumerated() { bytes[index % 16] &+= byte }
        return UUID(uuid: (
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        ))
    }
}

// MARK: - Fakes

@MainActor
private final class FakeProjects {
    struct Registration: Equatable {
        let root: String
        let commandID: UUID
    }

    var previews: [String: JetProjectPreview]
    var existing: [JetProjectSummary]
    var existingAfterRefresh: [JetProjectSummary]?
    var registerResults: [Result<JetProjectSummary, Error>] = []
    var heldPaths: Set<String> = []
    private(set) var held: [CheckedContinuation<Void, Never>] = []
    private(set) var previewedPaths: [String] = []
    private(set) var registrations: [Registration] = []
    private(set) var refreshes = 0

    init(previews: [String: JetProjectPreview], existing: [JetProjectSummary] = []) {
        self.previews = previews
        self.existing = existing
    }

    func model() -> AddProjectModel {
        AddProjectModel(
            planeRegistryID: UUID(),
            existingProjects: { [unowned self] _ in existing },
            previewFolder: { [unowned self] path, _ in
                previewedPaths.append(path)
                if heldPaths.contains(path) {
                    await withCheckedContinuation { held.append($0) }
                }
                guard let preview = previews[path] else {
                    throw JetClientFailure.presentation(ComposerTests.projectError("project.path_invalid"))
                }
                return preview
            },
            registerProject: { [unowned self] preview, commandID, _ in
                registrations.append(Registration(root: preview.root, commandID: commandID))
                guard !registerResults.isEmpty else { return ComposerTests.project(preview.root) }
                return try registerResults.removeFirst().get()
            },
            refreshProjects: { [unowned self] _ in
                refreshes += 1
                if let existingAfterRefresh { existing = existingAfterRefresh }
            }
        )
    }

    func releaseHeld() {
        let continuations = held
        held = []
        for continuation in continuations { continuation.resume() }
    }
}

@MainActor
private final class FakeNotifications: JetNotificationDelivering {
    let result: JetNotificationAuthorization
    private(set) var requests = 0

    init(result: JetNotificationAuthorization) {
        self.result = result
    }

    func authorizationStatus() async -> JetNotificationAuthorization { result }

    func requestAuthorization() async throws -> JetNotificationAuthorization {
        requests += 1
        return result
    }

    func deliver(_ notification: JetUserNotification) async throws {}
}
