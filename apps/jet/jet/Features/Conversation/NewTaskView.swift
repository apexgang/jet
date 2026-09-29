import SwiftUI

// MARK: - Checklist (design §6.7)

/// What still stands between New Task and a first task. Pure, so each state is
/// testable; `Inputs.from(_:)` reads the session.
enum NewTaskChecklist {
    enum RowID: Hashable {
        case helper
        case project
        case assistant
    }

    enum State: Equatable {
        case pending
        case inProgress
        case needsAction
        case failed
        case done
    }

    enum Action: Hashable {
        /// Try Again: restarts the helper's connection.
        case tryAgain
        case reinstall
        case addProject
        /// Try Again on the project list.
        case reloadProjects
        case checkAssistantsAgain
        /// A menu of setup guides for the supported assistants.
        case installationHelp

        var title: String {
            switch self {
            case .tryAgain, .reloadProjects: String(localized: "Try Again")
            case .reinstall: String(localized: "Reinstall Jet…")
            case .addProject: String(localized: "Add Project…")
            case .checkAssistantsAgain: String(localized: "Check Again")
            case .installationHelp: String(localized: "Installation Help")
            }
        }
    }

    struct Row: Equatable, Identifiable {
        let id: RowID
        var title: String
        var detail: String
        var note: String?
        var state: State
        var actions: [Action] = []
        /// Words after the actions, such as "or drop a folder here".
        var actionHint: String?
        /// The stable error code, shown under Details.
        var errorCode: String?
    }

    /// The helper's state, whether Jet on this Mac or on another computer.
    enum Helper: Equatable {
        case starting
        /// A setup retry is scheduled (at 2, 5 and 15 seconds).
        case retrying(attempt: Int, limit: Int)
        case reconnecting
        case failed(JetPresentationError)
        case running
    }

    enum Choice: Equatable {
        case unknown
        case failed
        case none
        case notChosen
        case chosen(String)
    }

    struct Inputs: Equatable {
        var computerName: String
        var isLocal: Bool
        var helper: Helper
        var hasConversations: Bool
        var projects: Choice
        var assistants: Choice
    }

    static let reinstallURL = URL(string: "https://github.com/apexgang/jet/releases/latest")
    static let claudeCodeSetupURL = URL(string: "https://code.claude.com/docs/en/setup")
    static let codexSetupURL = URL(string: "https://github.com/openai/codex")

    static func make(_ inputs: Inputs) -> [Row] {
        let helperDone = inputs.helper == .running
        return [helperRow(inputs), projectRow(inputs, helperDone: helperDone), assistantRow(inputs, helperDone: helperDone)]
    }

    private static func helperRow(_ inputs: Inputs) -> Row {
        let computer = inputs.computerName
        guard inputs.isLocal else {
            let title = String(localized: "Jet on \(computer)")
            switch inputs.helper {
            case .running:
                return Row(id: .helper, title: title, detail: String(localized: "Connected"), state: .done)
            case .failed:
                return Row(
                    id: .helper,
                    title: title,
                    detail: String(localized: "Can't reach \(computer)."),
                    state: .failed,
                    actions: [.tryAgain]
                )
            case .starting, .retrying, .reconnecting:
                return Row(
                    id: .helper,
                    title: title,
                    detail: String(localized: "Connecting to \(computer)…"),
                    state: .inProgress
                )
            }
        }
        let title = String(localized: "Jet on this Mac")
        switch inputs.helper {
        case let .retrying(attempt, limit):
            return Row(
                id: .helper,
                title: title,
                detail: String(localized: "Reconnecting… (attempt \(attempt) of \(limit))"),
                state: .inProgress
            )
        case .starting:
            return Row(
                id: .helper,
                title: title,
                detail: String(localized: "Starting Jet…"),
                note: inputs.hasConversations
                    ? nil
                    : String(localized: "Jet runs a small helper in the background so tasks keep going when this window is closed."),
                state: .inProgress
            )
        case .reconnecting:
            return Row(id: .helper, title: title, detail: String(localized: "Reconnecting…"), state: .inProgress)
        case let .failed(error):
            return Row(
                id: .helper,
                title: title,
                detail: String(localized: "Jet couldn't start its helper."),
                note: helperFailureReason(error),
                state: .failed,
                actions: error.code == "core.install_incomplete" ? [.reinstall, .tryAgain] : [.tryAgain],
                errorCode: error.code
            )
        case .running:
            return Row(id: .helper, title: title, detail: String(localized: "Jet is running"), state: .done)
        }
    }

    /// The reason line under "Jet couldn't start its helper.", when Jet knows one.
    static func helperFailureReason(_ error: JetPresentationError) -> String? {
        switch error.code {
        case "core.install_incomplete":
            return String(localized: "Jet's helper is incomplete. Reinstall Jet from its latest release.")
        case "core.owned_by_other_channel":
            return String(localized: "Another copy of Jet already runs this Mac's helper.")
        default:
            return error.category == .offline ? String(localized: "It isn't responding.") : nil
        }
    }

    private static func projectRow(_ inputs: Inputs, helperDone: Bool) -> Row {
        let title = String(localized: "Project")
        guard helperDone else {
            return Row(id: .project, title: title, detail: String(localized: "Available once Jet is running."), state: .pending)
        }
        switch inputs.projects {
        case .unknown:
            return Row(id: .project, title: title, detail: String(localized: "Available once Jet is running."), state: .pending)
        case .failed:
            return Row(
                id: .project,
                title: title,
                detail: String(localized: "Couldn't load your projects."),
                state: .failed,
                actions: [.reloadProjects]
            )
        case .none:
            return Row(
                id: .project,
                title: title,
                detail: String(localized: "Choose the Git repository you want to work on."),
                state: .needsAction,
                actions: [.addProject],
                actionHint: inputs.isLocal ? String(localized: "or drop a folder here") : nil
            )
        case .notChosen:
            return Row(
                id: .project,
                title: title,
                detail: String(localized: "Choose a project in the menu below."),
                state: .needsAction
            )
        case let .chosen(name):
            return Row(id: .project, title: title, detail: name, state: .done)
        }
    }

    private static func assistantRow(_ inputs: Inputs, helperDone: Bool) -> Row {
        let title = String(localized: "Assistant")
        guard helperDone else {
            return Row(id: .assistant, title: title, detail: String(localized: "Available once Jet is running."), state: .pending)
        }
        switch inputs.assistants {
        case .unknown:
            return Row(id: .assistant, title: title, detail: String(localized: "Available once Jet is running."), state: .pending)
        case .failed:
            return Row(
                id: .assistant,
                title: title,
                detail: String(localized: "Couldn't check for coding assistants."),
                state: .failed,
                actions: [.checkAssistantsAgain]
            )
        case .none:
            return Row(
                id: .assistant,
                title: title,
                detail: String(localized: "No coding assistant found. Jet works with Claude Code and Codex."),
                state: .needsAction,
                actions: [.installationHelp, .checkAssistantsAgain]
            )
        case .notChosen:
            return Row(
                id: .assistant,
                title: title,
                detail: String(localized: "Choose an assistant in the menu below."),
                state: .needsAction
            )
        case let .chosen(name):
            return Row(id: .assistant, title: title, detail: name, state: .done)
        }
    }
}

extension NewTaskChecklist.Inputs {
    static func from(_ session: DesktopSession) -> Self {
        let planeRegistryID = session.newTaskPlaneRegistryID
        let isLocal = session.isLocalPlane(planeRegistryID)
        let snapshot = session.planeSetupSnapshot(for: planeRegistryID)

        let projects: NewTaskChecklist.Choice
        let assistants: NewTaskChecklist.Choice
        if let snapshot {
            if snapshot.issue(for: .projects) != nil {
                projects = .failed
            } else if snapshot.projects.projects.isEmpty {
                projects = .none
            } else if let project = session.selectedProject {
                projects = .chosen(project.name)
            } else {
                projects = .notChosen
            }
            let crafts = snapshot.capabilities.crafts
            if snapshot.issue(for: .capabilities) != nil {
                assistants = .failed
            } else if crafts.isEmpty {
                assistants = .none
            } else if let craftID = session.selectedCraftID, crafts.contains(where: { $0.id == craftID }) {
                assistants = .chosen(DesktopSession.assistantLabel(craftID: craftID, crafts: crafts))
            } else {
                assistants = .notChosen
            }
        } else {
            projects = .unknown
            assistants = .unknown
        }

        return Self(
            computerName: session.planeName(planeRegistryID),
            isLocal: isLocal,
            helper: isLocal ? localHelper(session) : remoteHelper(session, planeRegistryID),
            hasConversations: !session.conversations.isEmpty,
            projects: projects,
            assistants: assistants
        )
    }

    /// This Mac's helper. A retry count shows only while a setup retry is really
    /// scheduled: the connection reads `.reconnecting(attempt: n)` for that retry.
    static func localHelper(
        connection: JetConnectionState,
        setupState: DesktopSession.SetupState,
        retryAttempt: Int,
        retryLimit: Int,
        planeFailure: JetPresentationError?
    ) -> NewTaskChecklist.Helper {
        switch connection {
        case let .reconnecting(attempt):
            return attempt > 0 && attempt == retryAttempt
                ? .retrying(attempt: attempt, limit: retryLimit)
                : .reconnecting
        case let .failed(error):
            return .failed(error)
        case .connected:
            switch setupState {
            case .ready: return .running
            case let .failed(error): return .failed(error)
            case .idle, .loading: return .starting
            }
        case .connecting, .disconnected:
            if case let .failed(error) = setupState { return .failed(error) }
            if case .disconnected = connection, let planeFailure { return .failed(planeFailure) }
            return .starting
        }
    }

    private static func localHelper(_ session: DesktopSession) -> NewTaskChecklist.Helper {
        localHelper(
            connection: session.connectionState,
            setupState: session.setupState,
            retryAttempt: session.setupRetryAttempt,
            retryLimit: session.setupRetryLimit,
            planeFailure: session.planes.first { $0.id == session.localPlaneRegistryID }?.failure
        )
    }

    private static func remoteHelper(_ session: DesktopSession, _ planeRegistryID: UUID) -> NewTaskChecklist.Helper {
        if session.isComputerOffline(planeRegistryID) {
            return .failed(session.planes.first { $0.id == planeRegistryID }?.failure ?? .offline)
        }
        return session.isPlaneConnected(planeRegistryID) ? .running : .starting
    }
}

// MARK: - New Task

struct NewTaskView: View {
    @Bindable var session: DesktopSession
    let composerFocused: FocusState<Bool>.Binding

    @Environment(\.openURL) private var openURL
    @State private var isDropTargeted = false

    struct Example: Identifiable {
        let title: String
        let prompt: String
        var id: String { title }
    }

    static let examples: [Example] = [
        Example(
            title: String(localized: "Explain how this project is organized"),
            prompt: String(localized: "Explain how this project is organized. Start with a short overview of the main parts and how they fit together.")
        ),
        Example(
            title: String(localized: "Fix a failing test"),
            prompt: String(localized: "Find the failing test and fix it. The test is: ")
        ),
        Example(
            title: String(localized: "Find the cause of a bug"),
            prompt: String(localized: "Find the cause of this bug. What happens: ")
        ),
    ]

    var body: some View {
        GeometryReader { geometry in
            ScrollView {
                column
                    .frame(maxWidth: JetDesign.writingWidth, alignment: .leading)
                    .overlay {
                        if isDropTargeted {
                            RoundedRectangle(cornerRadius: JetDesign.fieldRadius)
                                .strokeBorder(JetDesign.accent, style: StrokeStyle(lineWidth: 2, dash: [6, 4]))
                                .padding(-16)
                                .allowsHitTesting(false)
                        }
                    }
                    .padding(32)
                    .frame(maxWidth: .infinity, minHeight: geometry.size.height)
                    // Dropping a folder on New Task opens Add Project with it (design §6.16).
                    .modifier(AddProjectFolderDrop(
                        isEnabled: session.userOperation == nil,
                        isTargeted: $isDropTargeted
                    ) { url in
                        session.presentAddProject(droppedURL: url)
                    })
            }
        }
        .accessibilityIdentifier("new-task-workspace")
    }

    private var showsChecklist: Bool { !session.canStartTask }

    private var showsExamples: Bool {
        !showsChecklist && session.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var column: some View {
        VStack(alignment: .leading, spacing: 0) {
            JetMark(size: 32)
            Text("What are you working on?")
                .font(.system(size: JetDesign.TextSize.invitation, weight: .medium))
                .accessibilityAddTraits(.isHeader)
                .padding(.top, 16)
            Text("Describe what you need. You review the changes before anything reaches your project.")
                .font(.system(size: JetDesign.TextSize.navigation))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 8)
            if showsChecklist {
                NewTaskChecklistView(rows: NewTaskChecklist.make(.from(session)), perform: perform)
                    .padding(.top, 24)
            }
            WorkspaceComposer(session: session, composerFocused: composerFocused, placement: .newTask)
                .padding(.top, 24)
            if showsExamples {
                examples
                    .padding(.top, 24)
            }
        }
    }

    private var examples: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Start with an idea")
                .font(.system(size: JetDesign.TextSize.metadata))
                .foregroundStyle(.secondary)
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) { exampleButtons }
                VStack(alignment: .leading, spacing: 8) { exampleButtons }
            }
        }
    }

    @ViewBuilder private var exampleButtons: some View {
        ForEach(Self.examples) { example in
            Button(example.title) { session.insertExample(example.prompt) }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .fixedSize()
        }
    }

    private func perform(_ action: NewTaskChecklist.Action) {
        switch action {
        case .tryAgain:
            Task { await session.retryConnection() }
        case .reinstall:
            if let url = NewTaskChecklist.reinstallURL { openURL(url) }
        case .addProject:
            session.presentAddProject(droppedURL: nil)
        case .reloadProjects:
            Task { await session.refreshProjects(on: session.newTaskPlaneRegistryID) }
        case .checkAssistantsAgain:
            session.perform(.checkAssistantsAgain)
        case .installationHelp:
            break // A menu; see NewTaskChecklistRow.
        }
    }
}

// MARK: - Checklist views

struct NewTaskChecklistView: View {
    let rows: [NewTaskChecklist.Row]
    let perform: (NewTaskChecklist.Action) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                if index > 0 { Divider() }
                NewTaskChecklistRow(row: row, perform: perform)
                    .padding(.vertical, 10)
            }
        }
    }
}

private struct NewTaskChecklistRow: View {
    let row: NewTaskChecklist.Row
    let perform: (NewTaskChecklist.Action) -> Void

    @Environment(\.openURL) private var openURL

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 10) {
                glyph
                    .frame(width: 16, height: 16)
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.title)
                        .font(.system(size: JetDesign.TextSize.navigation, weight: .semibold))
                    Text(row.detail)
                        .font(.system(size: JetDesign.TextSize.control))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    if let note = row.note {
                        Text(note)
                            .font(.system(size: JetDesign.TextSize.control))
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .accessibilityElement(children: .combine)

            if !row.actions.isEmpty || row.errorCode != nil {
                VStack(alignment: .leading, spacing: 6) {
                    if !row.actions.isEmpty {
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            ForEach(row.actions, id: \.self) { action in
                                button(for: action)
                            }
                            if let hint = row.actionHint {
                                Text(hint)
                                    .font(.system(size: JetDesign.TextSize.control))
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    if let code = row.errorCode {
                        ErrorCodeDetails(code: code)
                    }
                }
                .padding(.leading, 26)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder private var glyph: some View {
        switch row.state {
        case .pending:
            Image(systemName: "circle.dotted")
                .foregroundStyle(.tertiary)
                .accessibilityLabel("Waiting")
        case .inProgress:
            ProgressView()
                .controlSize(.small)
                .scaleEffect(0.8)
                .accessibilityLabel("In progress")
        case .needsAction:
            Image(systemName: "circle")
                .foregroundStyle(.secondary)
                .accessibilityLabel("Needs your action")
        case .failed:
            Image(systemName: "xmark.octagon.fill")
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(.red)
                .accessibilityLabel("Failed")
        case .done:
            Image(systemName: "checkmark.circle.fill")
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(.green)
                .accessibilityLabel("Done")
        }
    }

    @ViewBuilder private func button(for action: NewTaskChecklist.Action) -> some View {
        switch action {
        case .installationHelp:
            Menu(action.title) {
                Button("Claude Code") {
                    if let url = NewTaskChecklist.claudeCodeSetupURL { openURL(url) }
                }
                Button("Codex") {
                    if let url = NewTaskChecklist.codexSetupURL { openURL(url) }
                }
            }
            .menuStyle(.button)
            .buttonStyle(.bordered)
            .controlSize(.small)
            .fixedSize()
        default:
            Button(action.title) { perform(action) }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .fixedSize()
        }
    }
}

/// A Details disclosure with a stable error code and a Copy button.
struct ErrorCodeDetails: View {
    let code: String

    var body: some View {
        DisclosureGroup("Details") {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(verbatim: code)
                    .font(.system(size: JetDesign.TextSize.metadata).monospaced())
                    .textSelection(.enabled)
                Button {
                    DesktopSession.copyToPasteboard(code)
                } label: {
                    Label("Copy", systemImage: "document.on.document")
                }
                .buttonStyle(.borderless)
                .controlSize(.small)
                .help("Copy the error code")
            }
            .padding(.top, 4)
        }
        .font(.system(size: JetDesign.TextSize.control))
    }
}

#if DEBUG
#Preview("New Task") {
    DesktopPreviewScenes.newTask[0].makeView()
        .frame(width: 1000, height: 760)
}
#endif
