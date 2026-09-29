import Foundation
import SwiftUI
import Testing
@testable import jet

@MainActor
struct ShellChromeTests {
    // MARK: - Column layout

    @Test func wideWindowKeepsTheSidebarWithDetails() {
        var layout = ShellColumnLayout()
        layout.update(width: 1_280, detailsPresented: true)
        #expect(layout.visibility == .all)
        #expect(!layout.isSidebarAutoHidden)
    }

    @Test func narrowWindowWithDetailsHidesTheSidebarUntilDetailsCloseOrTheWindowWidens() {
        var layout = ShellColumnLayout()
        layout.update(width: 900, detailsPresented: true)
        #expect(layout.visibility == .detailOnly)
        #expect(layout.isSidebarAutoHidden)
        #expect(layout.prefersSidebar)

        layout.update(width: 900, detailsPresented: false)
        #expect(layout.visibility == .all)

        layout.update(width: 900, detailsPresented: true)
        layout.update(width: 1_200, detailsPresented: true)
        #expect(layout.visibility == .all)
    }

    @Test func narrowWindowWithoutDetailsKeepsTheSidebar() {
        var layout = ShellColumnLayout()
        layout.update(width: 900, detailsPresented: false)
        #expect(layout.visibility == .all)
    }

    @Test func shrinkingNeverAsksToCloseDetails() {
        var layout = ShellColumnLayout()
        layout.update(width: 1_280, detailsPresented: true)
        layout.update(width: 900, detailsPresented: true)
        // The echo of the auto-hide is not a person's choice.
        let r1 = layout.personSet(.detailOnly, width: 900, detailsPresented: true)
        #expect(!r1)
        #expect(layout.prefersSidebar)
    }

    @Test func aSidebarThePersonHidStaysHidden() {
        var layout = ShellColumnLayout()
        let r2 = layout.personSet(.detailOnly, width: 1_280, detailsPresented: false)
        #expect(!r2)
        #expect(!layout.prefersSidebar)
        layout.update(width: 1_400, detailsPresented: false)
        #expect(layout.visibility == .detailOnly)
        layout.update(width: 900, detailsPresented: true)
        layout.update(width: 900, detailsPresented: false)
        #expect(layout.visibility == .detailOnly)
    }

    @Test func showingTheSidebarWhileNarrowClosesDetails() {
        var layout = ShellColumnLayout()
        layout.update(width: 900, detailsPresented: true)
        let r3 = layout.personSet(.all, width: 900, detailsPresented: true)
        #expect(r3)
        #expect(layout.visibility == .all)
        #expect(!layout.isSidebarAutoHidden)
    }

    @Test func showingTheSidebarWhenWideDoesNotCloseDetails() {
        var layout = ShellColumnLayout()
        _ = layout.personSet(.detailOnly, width: 1_280, detailsPresented: true)
        let r4 = layout.personSet(.all, width: 1_280, detailsPresented: true)
        #expect(!r4)
        #expect(layout.visibility == .all)
    }

    @Test func echoesAreIgnored() {
        var layout = ShellColumnLayout()
        let r5 = layout.personSet(.all, width: 900, detailsPresented: true)
        #expect(!r5)
        let r6 = layout.personSet(.automatic, width: 900, detailsPresented: true)
        #expect(!r6)
        #expect(layout.prefersSidebar)
    }

    @Test func persistedRoundTrip() {
        var layout = ShellColumnLayout()
        #expect(layout.persisted == "all")
        _ = layout.personSet(.detailOnly, width: 1_280, detailsPresented: false)
        #expect(layout.persisted == "detailOnly")
        #expect(ShellColumnLayout(persisted: "detailOnly").visibility == .detailOnly)
        #expect(ShellColumnLayout(persisted: "all").visibility == .all)
        #expect(ShellColumnLayout(persisted: "doubleColumn").persisted == "all")
        #expect(ShellColumnLayout(persisted: "").persisted == "all")
    }

    // MARK: - Banners

    private func inputs(
        destination: ShellDestination = .task,
        computer: String = "This Mac",
        isLocal: Bool = true,
        connection: ShellBannerInputs.Connection = .connected,
        showsSavedView: Bool = false,
        recoveryReadOnly: Bool = false,
        errorCodes: [String] = []
    ) -> ShellBannerInputs {
        ShellBannerInputs(
            destination: destination,
            computerName: computer,
            isLocal: isLocal,
            connection: connection,
            showsSavedView: showsSavedView,
            recoveryReadOnly: recoveryReadOnly,
            errorCodes: errorCodes
        )
    }

    @Test func noBannerWhenConnectedOnNewTaskOrWhileStarting() {
        #expect(ShellBanner.resolve(inputs()) == nil)
        #expect(ShellBanner.resolve(inputs(destination: .newTask, connection: .notConnected)) == nil)
        #expect(ShellBanner.resolve(inputs(connection: .starting)) == nil)
        #expect(ShellBanner.resolve(inputs(connection: .starting, showsSavedView: true)) == nil)
    }

    @Test func jetTrashShowsItsOwnConnectionAndRecoveryRows() {
        #expect(ShellBanner.resolve(inputs(destination: .trash, connection: .notConnected)) == nil)
        #expect(ShellBanner.resolve(inputs(destination: .trash, recoveryReadOnly: true)) == nil)
    }

    @Test func notConnectedNamesTheComputer() {
        #expect(ShellBanner.resolve(inputs(connection: .notConnected)) == .notConnected(computer: "This Mac"))
        #expect(
            ShellBanner.resolve(inputs(computer: "Studio Mac", isLocal: false, connection: .notConnected))
                == .notConnected(computer: "Studio Mac")
        )
        #expect(ShellBanner.resolve(inputs(destination: .project, connection: .notConnected)) != nil)
        #expect(
            ShellBanner.notConnected(computer: "Studio Mac").text
                == "Not connected to Studio Mac. Showing the last saved view."
        )
    }

    @Test func aCachedTaskShowsNotConnected() {
        #expect(ShellBanner.resolve(inputs(showsSavedView: true)) == .notConnected(computer: "This Mac"))
        #expect(
            ShellBanner.resolve(inputs(computer: "Studio Mac", isLocal: false, showsSavedView: true))
                == .notConnected(computer: "Studio Mac")
        )
    }

    @Test func readOnlyRecoveryFromJSONAndFromErrorCode() {
        #expect(ShellBanner.isReadOnly(recoveryJSON: #"{"state":"read_only","reason":"integrity_check_failed","snapshots":[]}"#))
        #expect(!ShellBanner.isReadOnly(recoveryJSON: #"{"state":"ok"}"#))
        #expect(!ShellBanner.isReadOnly(recoveryJSON: #"{"state":"read_only""#))
        #expect(!ShellBanner.isReadOnly(recoveryJSON: "[]"))
        #expect(!ShellBanner.isReadOnly(recoveryJSON: nil))
        #expect(ShellBanner.resolve(inputs(recoveryReadOnly: true)) == .dataProtection)
        #expect(ShellBanner.resolve(inputs(errorCodes: ["recovery.read_only"])) == .dataProtection)
        #expect(ShellBanner.dataProtection.text == "Jet paused changes to protect your data.")
    }

    @Test func diskPressureCopyLocalAndRemote() {
        #expect(ShellBanner.resolve(inputs(destination: .newTask, errorCodes: ["storage.disk_pressure"])) == .lowDiskSpace(computer: nil))
        #expect(
            ShellBanner.resolve(inputs(computer: "Studio Mac", isLocal: false, errorCodes: ["storage.disk_pressure"]))
                == .lowDiskSpace(computer: "Studio Mac")
        )
        #expect(ShellBanner.lowDiskSpace(computer: nil).text == "Your Mac is almost out of disk space. Jet paused new work.")
        #expect(
            ShellBanner.lowDiskSpace(computer: "Studio Mac").text
                == "Studio Mac is almost out of disk space. Jet paused new work."
        )
    }

    @Test func bannerPriority() {
        let all = inputs(
            connection: .notConnected,
            recoveryReadOnly: true,
            errorCodes: ["storage.disk_pressure"]
        )
        #expect(ShellBanner.resolve(all) == .notConnected(computer: "This Mac"))
        var connected = all
        connected.connection = .connected
        #expect(ShellBanner.resolve(connected) == .dataProtection)
        connected.recoveryReadOnly = false
        #expect(ShellBanner.resolve(connected) == .lowDiskSpace(computer: nil))
    }

    @Test func localConnectionStates() {
        #expect(ShellBannerInputs.localConnection(setupState: .loading, connectionState: .connecting) == .starting)
        #expect(ShellBannerInputs.localConnection(setupState: .idle, connectionState: .disconnected) == .starting)
        #expect(ShellBannerInputs.localConnection(setupState: .failed(.offline), connectionState: .disconnected) == .notConnected)
    }

    @Test func bannerCopyAvoidsDomainWords() {
        let banners: [ShellBanner] = [
            .notConnected(computer: "This Mac"),
            .dataProtection,
            .lowDiskSpace(computer: nil),
            .lowDiskSpace(computer: "Studio Mac"),
        ]
        for banner in banners {
            #expect(JetCopy.foundAvoidWords(in: banner.text).isEmpty, "\(banner.text)")
            #expect(JetCopy.foundAvoidWords(in: banner.actionTitle).isEmpty, "\(banner.actionTitle)")
        }
    }

    // MARK: - Titles

    @Test func titlesForEveryDestination() {
        #expect(ShellTitle.title(destination: .newTask, taskTitle: "x", projectName: "y") == "New Task")
        #expect(ShellTitle.title(destination: .task, taskTitle: "Fix login", projectName: nil) == "Fix login")
        #expect(ShellTitle.title(destination: .task, taskTitle: nil, projectName: nil) == "Untitled task")
        #expect(ShellTitle.title(destination: .task, taskTitle: "  ", projectName: nil) == "Untitled task")
        #expect(ShellTitle.title(destination: .project, taskTitle: nil, projectName: "web-app") == "web-app")
        #expect(ShellTitle.title(destination: .trash, taskTitle: nil, projectName: nil) == "Jet Trash")
    }

    @Test func subtitlesOmitUnknownParts() {
        #expect(
            ShellTitle.subtitle(projectName: "web-app", assistantName: "Claude Code", remoteComputer: "Studio Mac")
                == "web-app · Claude Code · Studio Mac"
        )
        #expect(ShellTitle.subtitle(projectName: "web-app", assistantName: "Claude Code", remoteComputer: nil) == "web-app · Claude Code")
        #expect(ShellTitle.subtitle(projectName: nil, assistantName: "Codex", remoteComputer: nil) == "Codex")
        #expect(ShellTitle.subtitle(projectName: nil, assistantName: nil, remoteComputer: nil) == "")
    }

    @Test func projectSubtitleUsesTildeOnlyForLocalProjects() {
        #expect(ShellTitle.projectSubtitle(root: "/Users/alex/code/web-app", isLocal: true, home: "/Users/alex") == "~/code/web-app")
        #expect(ShellTitle.projectSubtitle(root: "/Users/alex/code/web-app", isLocal: false, home: "/Users/alex") == "/Users/alex/code/web-app")
        #expect(ShellTitle.projectSubtitle(root: "/Users/alexander/app", isLocal: true, home: "/Users/alex") == "/Users/alexander/app")
        #expect(ShellTitle.projectSubtitle(root: "/Users/alex", isLocal: true, home: "/Users/alex/") == "~")
        #expect(
            ShellTitle.projectSubtitle(root: "/srv/app", isLocal: false, home: "/Users/alex", computer: "Studio Mac")
                == "/srv/app · Studio Mac"
        )
    }

    // MARK: - Toolbar status

    @Test func toolbarStatusHidesUnknownAndNotStarted() {
        #expect(ShellToolbarStatus.visible(.unknown, isTask: true, detailWidth: 900) == nil)
        #expect(ShellToolbarStatus.visible(.notStarted, isTask: true, detailWidth: 900) == nil)
        #expect(ShellToolbarStatus.visible(.waitingForReply, isTask: false, detailWidth: 900) == nil)
        #expect(ShellToolbarStatus.visible(.waitingForReply, isTask: true, detailWidth: 900) == .waitingForReply)
    }

    @Test func toolbarStatusGivesWayByWidth() {
        let working = TaskStatus.working(.editingFiles)
        let hidden = ShellToolbarStatus.hiddenBelow
        let compact = ShellToolbarStatus.compactBelow
        #expect(ShellToolbarStatus.visible(working, isTask: true, detailWidth: hidden - 1) == nil)
        #expect(ShellToolbarStatus.visible(working, isTask: true, detailWidth: hidden) == .working(nil))
        #expect(ShellToolbarStatus.visible(working, isTask: true, detailWidth: compact - 1) == .working(nil))
        #expect(ShellToolbarStatus.visible(working, isTask: true, detailWidth: compact) == working)
        #expect(ShellToolbarStatus.visible(.needsPermission, isTask: true, detailWidth: compact - 1) == .needsPermission)
    }

    // MARK: - Announcement gate

    @Test func gatePostsImmediatelyThenCoalesces() {
        let id = UUID()
        let start = Date(timeIntervalSince1970: 1_000)
        var gate = StatusAnnouncementGate()
        let r7 = gate.submit("Working", conversationID: id, now: start)
        #expect(r7 == .ignore)
        let r8 = gate.submit("Waiting for your reply", conversationID: id, now: start)
        #expect(r8 == .postNow("Waiting for your reply"))
        gate.didPost("Waiting for your reply", at: start)
        let later = start.addingTimeInterval(0.5)
        let r9 = gate.submit("Finished", conversationID: id, now: later)
        #expect(r9 == .postLater("Finished", at: start.addingTimeInterval(2)))
        let afterInterval = start.addingTimeInterval(3)
        let r10 = gate.submit("Failed", conversationID: id, now: afterInterval)
        #expect(r10 == .postNow("Failed"))
    }

    @Test func gateIgnoresSelectionChangesAndRepeats() {
        let first = UUID()
        let second = UUID()
        let now = Date(timeIntervalSince1970: 1_000)
        var gate = StatusAnnouncementGate()
        _ = gate.submit("Working", conversationID: first, now: now)
        let r11 = gate.submit("Finished", conversationID: second, now: now)
        #expect(r11 == .ignore)
        let r12 = gate.submit("Finished", conversationID: second, now: now)
        #expect(r12 == .ignore)
        let r13 = gate.submit("Failed", conversationID: second, now: now)
        #expect(r13 == .postNow("Failed"))
        let r14 = gate.submit("Failed", conversationID: second, now: now)
        #expect(r14 == .ignore)
    }

    @Test func gateCancelsAPendingPostWhenTheStatusReturns() {
        let id = UUID()
        let start = Date(timeIntervalSince1970: 1_000)
        var gate = StatusAnnouncementGate()
        _ = gate.submit("Working", conversationID: id, now: start)
        let r15 = gate.submit("Finished", conversationID: id, now: start)
        #expect(r15 == .postNow("Finished"))
        gate.didPost("Finished", at: start)
        let r16 = gate.submit("Working", conversationID: id, now: start.addingTimeInterval(1))
        #expect(r16 == .postLater("Working", at: start.addingTimeInterval(2)))
        let r17 = gate.submit("Finished", conversationID: id, now: start.addingTimeInterval(1.5))
        #expect(r17 == .cancelPending)
    }

    // MARK: - Text scale

    @Test func textScaleSteps() {
        let steps = TranscriptTextScale.steps
        #expect(!steps.isEmpty)
        #expect(steps == steps.sorted())
        #expect(steps.contains(1.0))
        #expect(TranscriptTextScale.bigger(than: steps.last!) == nil)
        #expect(TranscriptTextScale.smaller(than: steps.first!) == nil)
        for (lower, upper) in zip(steps, steps.dropFirst()) {
            #expect(TranscriptTextScale.bigger(than: lower) == upper)
            #expect(TranscriptTextScale.smaller(than: upper) == lower)
        }
    }

    @Test func textScaleOffStepValues() {
        #expect(TranscriptTextScale.bigger(than: 1.0005) == TranscriptTextScale.bigger(than: 1.0))
        #expect(TranscriptTextScale.bigger(than: 1.07) == 1.15)
        #expect(TranscriptTextScale.smaller(than: 1.07) == 1.0)
        #expect(TranscriptTextScale.smaller(than: 0.5) == nil)
        #expect(TranscriptTextScale.bigger(than: 9) == nil)
    }

    // MARK: - Command state

#if DEBUG
    @Test func newTaskCommandState() {
        let session = DesktopSession.preview {
            DesktopPreviewData.connect($0)
            $0.open(.newTask)
        }
        let state = ShellCommandState(session: session)
        #expect(state.sendTitle == "Start Task")
        #expect(!state.detailsAvailable)
        #expect(!state.canMoveToTrash)
        #expect(state.taskRef == nil)
        #expect(ShellTitle.title(for: session, destination: ShellDestination(session: session)) == "New Task")
        #expect(ShellTitle.subtitle(for: session, destination: .newTask) == "")
    }

    @Test func workingCommandState() {
        let session = DesktopSession.preview { DesktopPreviewData.working($0) }
        let state = ShellCommandState(session: session)
        #expect(ShellDestination(session: session) == .task)
        #expect(state.sendTitle == "Send")
        #expect(state.detailsAvailable)
        #expect(state.canMoveToTrash)
        #expect(state.canInterrupt)
        #expect(state.taskRef != nil)
        #expect(!ShellCommandState(session: session, isEditingText: true).canMoveToTrash)
        #expect(!ShellCommandState(session: session, isEditingText: true).canSend)
        #expect(
            ShellCommandState(session: session, isEditingText: true, isComposerFocused: true).canSend
                == ShellCommandState(session: session).canSend
        )
        #expect(!ShellCommandState(session: session, hasOpenDialog: true).canInterrupt)
    }

    @Test func aSheetDisablesInterruptAndMoveToTrash() {
        let session = DesktopSession.preview { DesktopPreviewData.working($0) }
        session.presentRename()
        let state = ShellCommandState(session: session)
        #expect(state.modal)
        #expect(!state.canInterrupt)
        #expect(!state.canMoveToTrash)
    }

    @Test func connectedTasksShowNoBanner() {
        let session = DesktopSession.preview { DesktopPreviewData.working($0) }
        #expect(ShellBanner.resolve(ShellBannerInputs(session: session)) == nil)
    }

    @Test func anOfflineTaskShowsNotConnected() {
        let session = DesktopSession.preview { DesktopPreviewData.offline($0) }
        #expect(ShellBanner.resolve(ShellBannerInputs(session: session)) == .notConnected(computer: "This Mac"))
    }
#endif
}
