import SwiftUI
#if os(macOS)
import AppKit
#endif

/// What the detail column shows, as the toolbar, title and banners see it.
enum ShellDestination: Equatable, Sendable {
    case newTask
    case task
    case project
    case trash

    init(session: DesktopSession) {
        switch session.sidebarSelection {
        case .newTask: self = .newTask
        case .project: self = .project
        case .trash: self = .trash
        case .conversation, .search, .needsAttention, .schedules, .planes:
            self = session.selectedConversationID == nil ? .newTask : .task
        }
    }
}

// MARK: - Title and subtitle

/// The window title and subtitle for each destination (design §6.4).
enum ShellTitle {
    static func title(destination: ShellDestination, taskTitle: String?, projectName: String?) -> String {
        switch destination {
        case .newTask: String(localized: "New Task")
        case .task: nonEmpty(taskTitle) ?? String(localized: "Untitled task")
        case .project: nonEmpty(projectName) ?? String(localized: "Project")
        case .trash: String(localized: "Jet Trash")
        }
    }

    /// "web-app · Claude Code · Studio Mac", leaving out the parts that aren't known.
    static func subtitle(projectName: String?, assistantName: String?, remoteComputer: String?) -> String {
        [projectName, assistantName, remoteComputer]
            .compactMap(nonEmpty)
            .joined(separator: " · ")
    }

    /// A project's folder. Only a local folder under `home` is shortened to "~"; a
    /// remote project also names its computer when there are several.
    static func projectSubtitle(root: String, isLocal: Bool, home: String, computer: String? = nil) -> String {
        var path = root
        let home = home.hasSuffix("/") ? String(home.dropLast()) : home
        if isLocal, !home.isEmpty {
            if root == home {
                path = "~"
            } else if root.hasPrefix(home + "/") {
                path = "~" + root.dropFirst(home.count)
            }
        }
        return subtitle(projectName: path, assistantName: nil, remoteComputer: computer)
    }

    static func title(for session: DesktopSession, destination: ShellDestination) -> String {
        title(
            destination: destination,
            taskTitle: taskTitle(session),
            projectName: session.selectedProject?.name
        )
    }

    static func subtitle(for session: DesktopSession, destination: ShellDestination) -> String {
        let planeRegistryID = session.selectedPlaneRegistryID
        let isLocal = session.isLocalPlane(planeRegistryID)
        switch destination {
        case .newTask, .trash:
            return ""
        case .task:
            return subtitle(
                projectName: session.selectedConversationID.flatMap(session.projectName(for:)),
                assistantName: session.selectedAssistantName,
                remoteComputer: isLocal ? nil : session.selectedPlaneName
            )
        case .project:
            guard let project = session.selectedProject else { return "" }
            return projectSubtitle(
                root: project.root,
                isLocal: isLocal,
                home: NSHomeDirectory(),
                computer: session.showsComputerNames && !isLocal ? session.selectedPlaneName : nil
            )
        }
    }

    /// The summary's title, else the loaded snapshot's when it belongs to the open task.
    private static func taskTitle(_ session: DesktopSession) -> String? {
        if let title = nonEmpty(session.selectedConversation?.title) { return title }
        guard let snapshot = session.conversationSnapshot,
              snapshot.conversation.id == session.selectedConversationID
        else { return nil }
        return nonEmpty(snapshot.conversation.title)
    }

    private static func nonEmpty(_ text: String?) -> String? {
        guard let text, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return text
    }
}

// MARK: - Toolbar status

/// Which status the toolbar has room for. SwiftUI has no visibility priority for
/// toolbar items, so the status gives way first by the detail column's width.
enum ShellToolbarStatus {
    /// Below this detail width the status is hidden.
    static let hiddenBelow: CGFloat = 520
    /// Below this detail width Working drops its phase.
    static let compactBelow: CGFloat = 680

    static func visible(_ status: TaskStatus, isTask: Bool, detailWidth: CGFloat) -> TaskStatus? {
        guard isTask, status != .unknown, status != .notStarted, detailWidth >= hiddenBelow else {
            return nil
        }
        if case .working = status, detailWidth < compactBelow { return .working(nil) }
        return status
    }
}

// MARK: - Command state

/// The one derivation of what the toolbar and the menu bar can do right now.
struct ShellCommandState: Equatable {
    var taskRef: ConversationRef?
    var isConnected: Bool
    var modal: Bool
    var sendTitle: String
    var canSend: Bool
    var canInterrupt: Bool
    var canStopAssistant: Bool
    var canRename: Bool
    var canEditTask: Bool
    var detailsAvailable: Bool
    var showsKeepChanges: Bool
    var canKeepChanges: Bool
    var keepChangesProminent: Bool
    var isLocalTask: Bool
    var canRevealWorkingCopy: Bool
    var canCopyWorkingCopyPath: Bool
    var canMoveToTrash: Bool
    var canReviewChanges: Bool
    var canCheckGitStatus: Bool
    var gitStepToCheck: JetGitDelivery?

    /// - Parameters:
    ///   - isEditingText: a text input has focus, so ⌘⌫ must not move the task.
    ///   - isComposerFocused: that input is the composer, where ⌘↩ sends.
    ///   - hasOpenDialog: a dialog the session doesn't track is open.
    init(
        session: DesktopSession,
        isEditingText: Bool = false,
        isComposerFocused: Bool = false,
        hasOpenDialog: Bool = false
    ) {
        let isTask = ShellDestination(session: session) == .task
        let snapshotRevision = session.conversationSnapshot.flatMap {
            $0.conversation.id == session.selectedConversationID ? $0.conversation.revision : nil
        }
        taskRef = isTask ? session.selectedConversationRef : nil
        isConnected = session.planeIsConnected
        // The Git confirmations live in the delivery coordinator, which
        // `isModalPresented` doesn't know about.
        modal = session.isModalPresented || hasOpenDialog
            || session.deliveries.pendingConfirmation != nil
        sendTitle = session.nextSendStartsNewRun ? String(localized: "Start Task") : String(localized: "Send")
        canSend = session.canSend && !modal && (!isEditingText || isComposerFocused)
        let controlsRun = isConnected && !modal && session.supervisionOperation == nil
        canInterrupt = session.canInterruptTurn && controlsRun
        canStopAssistant = session.canStopRun && controlsRun
        canRename = isTask && (session.selectedConversation?.revision ?? snapshotRevision) != nil
            && isConnected && !modal
        canEditTask = isTask && isConnected && !modal
        detailsAvailable = isTask && session.canShowDetails
        showsKeepChanges = isTask && session.selectedRun != nil
            && session.selectedSetupSnapshot?.capabilities.gitIsAvailable == true
        canKeepChanges = isTask && session.canKeepChanges && !modal
        keepChangesProminent = session.hasKnownChanges && canKeepChanges
        isLocalTask = isTask && session.isLocalPlane(session.selectedPlaneRegistryID)
        canRevealWorkingCopy = isTask && session.canRevealWorkingCopy
        canCopyWorkingCopyPath = isTask && session.workingCopyPath != nil
        canMoveToTrash = isTask && isConnected && !isEditingText && !modal
        canReviewChanges = isTask && session.selectedRun != nil
        canCheckGitStatus = isTask && session.canCheckGitStatus
        gitStepToCheck = isTask ? session.uncheckedGitDelivery : nil
    }
}

// MARK: - VoiceOver announcements

/// Decides when a status change of the open task is announced: at most once every
/// two seconds, never for a selection change, and never twice for the same text.
struct StatusAnnouncementGate: Equatable {
    enum Decision: Equatable {
        case ignore
        case postNow(String)
        case postLater(String, at: Date)
        case cancelPending
    }

    static let interval: TimeInterval = 2

    private(set) var conversationID: UUID?
    private var lastSubmitted: String?
    private var lastPosted: String?
    private var lastPostDate: Date?

    mutating func submit(_ text: String, conversationID: UUID?, now: Date) -> Decision {
        guard conversationID == self.conversationID else {
            // Another task was opened: its status is the new baseline, not a change.
            self = StatusAnnouncementGate()
            self.conversationID = conversationID
            lastSubmitted = text
            lastPosted = text
            return .ignore
        }
        guard text != lastSubmitted else { return .ignore }
        lastSubmitted = text
        if text == lastPosted { return .cancelPending }
        if let lastPostDate, now < lastPostDate.addingTimeInterval(Self.interval) {
            return .postLater(text, at: lastPostDate.addingTimeInterval(Self.interval))
        }
        return .postNow(text)
    }

    mutating func didPost(_ text: String, at date: Date) {
        lastPosted = text
        lastPostDate = date
    }
}

/// Posts the open task's status changes to VoiceOver through the gate.
@MainActor
final class TaskStatusAnnouncer {
    private var gate = StatusAnnouncementGate()
    private var conversationID: UUID?
    private var pending: Task<Void, Never>?

    func statusChanged(_ status: TaskStatus, conversationID: UUID?) {
        if conversationID != self.conversationID {
            self.conversationID = conversationID
            cancel()
        }
        guard status != .unknown else { return }
        let now = Date.now
        switch gate.submit(status.accessibilityDescription, conversationID: conversationID, now: now) {
        case .ignore:
            break
        case .cancelPending:
            cancel()
        case let .postNow(text):
            cancel()
            post(text, at: now)
        case let .postLater(text, date):
            cancel()
            pending = Task { [weak self] in
                try? await Task.sleep(for: .seconds(max(0, date.timeIntervalSinceNow)))
                guard !Task.isCancelled, let self else { return }
                self.pending = nil
                self.post(text, at: .now)
            }
        }
    }

    func cancel() {
        pending?.cancel()
        pending = nil
    }

    private func post(_ text: String, at date: Date) {
        gate.didPost(text, at: date)
        AccessibilityNotification.Announcement(text).post()
    }
}

// MARK: - Toolbar

/// The detail column's title, title menu and toolbar items (design §6.4). The
/// fixture (iOS) views own their title, so it adds nothing there.
struct ShellToolbar: ViewModifier {
    @Bindable var session: DesktopSession
    let detailWidth: CGFloat

    /// A dialog or popover the session doesn't track (Revert…, a comment form).
    @FocusedValue(\.hasOpenDialog) private var hasOpenDialog

    func body(content: Content) -> some View {
#if os(macOS)
        if session.usesLivePlane {
            let destination = ShellDestination(session: session)
            let state = ShellCommandState(session: session, hasOpenDialog: hasOpenDialog == true)
            content
                .navigationTitle(ShellTitle.title(for: session, destination: destination))
                .navigationSubtitle(ShellTitle.subtitle(for: session, destination: destination))
                .toolbarTitleMenu {
                    if destination == .task { titleMenu(state) }
                }
                .renameAction { session.presentRename() }
                .toolbar { items(state, destination: destination) }
        } else {
            content
                .toolbar { DetailsToolbarItem(session: session) }
        }
#else
        content
#endif
    }

#if os(macOS)
    @ViewBuilder
    private func titleMenu(_ s: ShellCommandState) -> some View {
        RenameButton()
            .disabled(!s.canRename)
        if s.isLocalTask {
            Button("Show Working Copy in Finder", action: session.revealWorkingCopy)
                .disabled(!s.canRevealWorkingCopy)
        }
        Divider()
        Button("Move to Jet Trash…") { moveToTrash(s) }
            .disabled(!s.canMoveToTrash)
    }

    @ToolbarContentBuilder
    private func items(_ s: ShellCommandState, destination: ShellDestination) -> some ToolbarContent {
        let status = ShellToolbarStatus.visible(
            session.selectedTaskStatus,
            isTask: destination == .task,
            detailWidth: detailWidth
        )
        ToolbarItem(placement: .principal) {
            TaskStatusLabel(status: status ?? .unknown)
        }
        .sharedBackgroundVisibility(.hidden)
        .hidden(status == nil)

        ToolbarItem(placement: .primaryAction) {
            keepChangesButton(s)
        }
        .hidden(!s.showsKeepChanges)

        ToolbarSpacer(.fixed, placement: .primaryAction)

        ToolbarItem(placement: .primaryAction) {
            Button {
                session.requestInterrupt(thenReply: false)
            } label: {
                Label("Interrupt", systemImage: "stop.fill")
            }
            .help("Interrupt the current reply (⌘.)")
            .disabled(!s.canInterrupt)
            .accessibilityIdentifier("toolbar-interrupt")
        }
        .hidden(!session.canInterruptTurn)

        ToolbarItem(placement: .primaryAction) {
            Menu {
                switch destination {
                case .task: taskMoreItems(s)
                case .project: projectMoreItems(s)
                case .newTask, .trash: EmptyView()
                }
            } label: {
                Label("More", systemImage: "ellipsis.circle")
            }
            .menuIndicator(.hidden)
            .help("More")
            .accessibilityIdentifier("toolbar-more")
        }
        .hidden(destination == .newTask || destination == .trash)

        DetailsToolbarItem(session: session)
    }

    @ViewBuilder
    private func keepChangesButton(_ s: ShellCommandState) -> some View {
        let button = Button("Keep Changes…") { session.presentKeepChanges(mode: .plan) }
            .disabled(!s.canKeepChanges)
            .help(session.keepChangesUnavailableReason ?? String(localized: "Keep the changes from this task (⇧⌘K)"))
            .accessibilityIdentifier("toolbar-keep-changes")
        if s.keepChangesProminent {
            button.buttonStyle(.glassProminent)
        } else {
            button
        }
    }

    @ViewBuilder
    private func taskMoreItems(_ s: ShellCommandState) -> some View {
        Button("Rename…") { session.presentRename() }
            .disabled(!s.canRename)
        Button("Repeat Daily…") { session.presentRepeatDaily() }
            .disabled(!s.canEditTask)
        Button("Task Settings…") { session.presentTaskSettings() }
            .disabled(!s.canEditTask)
        Divider()
        if s.isLocalTask {
            Button("Show Working Copy in Finder", action: session.revealWorkingCopy)
                .disabled(!s.canRevealWorkingCopy)
        }
        Button("Copy Working Copy Path", action: session.copyWorkingCopyPath)
            .disabled(!s.canCopyWorkingCopyPath)
        Divider()
        Button("Stop Assistant…", action: session.requestStopAssistant)
            .disabled(!s.canStopAssistant)
        Divider()
        Button("Move to Jet Trash…") { moveToTrash(s) }
            .disabled(!s.canMoveToTrash)
    }

    @ViewBuilder
    private func projectMoreItems(_ s: ShellCommandState) -> some View {
        let planeRegistryID = session.selectedPlaneRegistryID
        let project = session.selectedProject
        let isLocal = session.isLocalPlane(planeRegistryID)
        Button {
            guard let project else { return }
            session.startNewTask(in: project.id, on: planeRegistryID)
        } label: {
            if let project {
                Text("New Task in “\(project.name)”")
            } else {
                Text("New Task")
            }
        }
        .disabled(project == nil || s.modal)
        if isLocal {
            Button("Show in Finder") {
                guard let project else { return }
                session.revealInFinder(path: project.root)
            }
            .disabled(project == nil)
        }
        Divider()
        Button("Move Project Folder to Trash…") {
            guard let project else { return }
            Task { await session.prepareProjectRemoval(project.id) }
        }
        .disabled(project == nil || !s.isConnected || s.modal || session.setupOperation != nil)
    }

    private func moveToTrash(_ s: ShellCommandState) {
        guard let ref = s.taskRef else { return }
        session.presentMoveToTrash(ref)
    }
#endif
}

#if os(macOS)
/// Show Details / Hide Details (⌥⌘0). It is attached at the split-view level so it
/// stays the trailing-most toolbar item, above the inspector when Details is open.
struct DetailsToolbarItem: ToolbarContent {
    let session: DesktopSession

    var body: some ToolbarContent {
        ToolbarItem(placement: .primaryAction) {
            let isOpen = session.isWorkPanelPresented
            let isAvailable = isOpen || session.canShowDetails
            let title = isOpen ? String(localized: "Hide Details") : String(localized: "Show Details")
            Button(action: session.toggleDetails) {
                Label(title, systemImage: "sidebar.right")
            }
            .help(isAvailable ? title : String(localized: "Details appear once a task starts."))
            .disabled(!isAvailable)
            .accessibilityIdentifier("toolbar-details")
        }
    }
}
#endif
