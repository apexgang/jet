import SwiftUI

struct DesktopShellView: View {
    @Bindable var session: DesktopSession

    @SceneStorage("jet.shell.selection") private var storedSelection = SidebarDestination.conversation.rawValue
    @SceneStorage("jet.shell.work-panel") private var storedWorkPanel = WorkPanelTab.run.rawValue
    @SceneStorage("jet.shell.work-panel-presented") private var storedPanelPresented = true
    @AppStorage("jet.settings.restore-last-task") private var restoresLastTask = true
    @AppStorage("jet.last-conversation") private var storedConversationID = ""
    @State private var columnVisibility: NavigationSplitViewVisibility = .all

    var body: some View {
        NavigationSplitView(columnVisibility: $columnVisibility) {
            SidebarView(session: session)
#if os(macOS)
                .navigationSplitViewColumnWidth(min: 210, ideal: 244, max: 300)
#endif
        } detail: {
            if session.sidebarSelection == .project {
                ProjectSetupView(session: session)
            } else {
                ConversationView(session: session)
            }
        }
#if os(macOS)
        .inspector(isPresented: $session.isWorkPanelPresented) {
            WorkPanelView(session: session)
                .inspectorColumnWidth(min: 280, ideal: 340, max: 440)
        }
        .frame(minWidth: 900, minHeight: 600)
#endif
        .tint(Color(red: 41 / 255, green: 182 / 255, blue: 246 / 255))
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    session.isWorkPanelPresented.toggle()
                } label: {
                    Label(
                        session.isWorkPanelPresented ? "Hide Work Panel" : "Show Work Panel",
                        systemImage: "sidebar.right"
                    )
                }
                .help(session.isWorkPanelPresented ? "Hide Work Panel" : "Show Work Panel")
            }
        }
        .task {
            await session.loadFoundationFixture()
            if restoresLastTask {
                session.restore(
                    selection: storedSelection,
                    workPanel: storedWorkPanel,
                    panelPresented: storedPanelPresented
                )
            } else {
                session.beginNewTask()
            }
            await session.loadSetup(openWhenIncomplete: true)
            await session.loadConversations(
                restoring: restoresLastTask ? UUID(uuidString: storedConversationID) : nil
            )
        }
        .onChange(of: session.sidebarSelection) { _, value in
            storedSelection = value.rawValue
        }
        .onChange(of: session.selectedWorkPanel) { _, value in
            storedWorkPanel = value.rawValue
        }
        .onChange(of: session.isWorkPanelPresented) { _, value in
            storedPanelPresented = value
        }
        .onChange(of: session.selectedConversationID) { _, value in
            storedConversationID = value?.uuidString.lowercased() ?? ""
        }
    }
}

private struct SidebarView: View {
    @Bindable var session: DesktopSession
#if os(macOS)
    @Environment(\.openSettings) private var openSettings
#endif

    var body: some View {
        Group {
#if os(macOS)
            List(selection: $session.sidebarSelection) {
                sidebarSections
            }
#else
            List {
                sidebarSections
            }
#endif
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom) {
            PlaneStatusFooter(session: session)
        }
        .navigationTitle("Jet")
        .searchable(text: $session.searchText, prompt: "Search tasks")
        .onSubmit(of: .search) {
            session.selectSearch()
            Task { await session.searchConversations() }
        }
        .onChange(of: session.sidebarSelection) { _, _ in
            session.applySidebarSelection()
        }
    }

    @ViewBuilder
    private var sidebarSections: some View {
            Section {
                Button(action: session.beginNewTask) {
                    Label("New task", systemImage: "square.and.pencil")
                }
                .buttonStyle(.plain)

                Button(action: session.selectSearch) {
                    Label("Search", systemImage: "magnifyingglass")
                }
                .buttonStyle(.plain)

                Label("Needs attention", systemImage: "bell")
                    .tag(SidebarDestination.needsAttention)
            }

            Section("Projects") {
                if let projects = session.setupSnapshot?.projects.projects {
                    ForEach(projects) { project in
                        Button {
                            session.selectProject(project.id)
                        } label: {
                            Label(project.name, systemImage: "folder")
                        }
                        .buttonStyle(.plain)
                    }
                }

                Button(action: session.requestAddProject) {
                    Label(
                        session.setupSnapshot?.projects.projects.isEmpty == false
                            ? "Add Project…"
                            : "Add a Project",
                        systemImage: "plus"
                    )
                }
                .buttonStyle(.plain)

                Label("Manage Projects", systemImage: "folder.badge.gearshape")
                    .tag(SidebarDestination.project)
            }

            Section("Recent") {
                ForEach(session.conversations) { conversation in
                    Button {
                        session.selectConversation(conversation.id)
                    } label: {
                        Label(conversation.title, systemImage: "bubble.left.and.bubble.right")
                            .lineLimit(2)
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(
                        session.selectedConversationID == conversation.id
                            ? Color.accentColor.opacity(0.16)
                            : Color.clear
                    )
                    .accessibilityValue(
                        session.selectedConversationID == conversation.id ? "Selected" : ""
                    )
                }

                if session.conversations.isEmpty, session.conversationFreshness != .loading {
                    Text(
                        session.conversationFreshness == .live
                            ? "No tasks yet"
                            : "Tasks unavailable"
                    )
                        .foregroundStyle(.secondary)
                }

                if session.nextConversationPage != nil {
                    Button {
                        Task { await session.loadMoreConversations() }
                    } label: {
                        Label("Show more", systemImage: "ellipsis")
                    }
                    .disabled(session.conversationOperation != nil)
                }
            }

            if session.sidebarSelection == .search, let result = session.searchResult {
                Section("Search results") {
                    ForEach(result.hits) { hit in
                        Button {
                            session.selectSearchHit(hit.conversationID)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(hit.excerpt)
                                    .lineLimit(2)
                                Text(hit.field.rawValue.capitalized)
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .buttonStyle(.plain)
                    }

                    if result.hits.isEmpty {
                        Text("No matching tasks")
                            .foregroundStyle(.secondary)
                    }
                }
            }

            Section {
                Label("Schedules", systemImage: "calendar")
                    .tag(SidebarDestination.schedules)
                Label("Planes", systemImage: "desktopcomputer")
                    .tag(SidebarDestination.planes)
#if os(macOS)
                Button {
                    openSettings()
                } label: {
                    Label("Settings", systemImage: "gearshape")
                }
                .buttonStyle(.plain)
#endif
            }
    }
}

private struct PlaneStatusFooter: View {
    let session: DesktopSession

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(connectionColor)
                .frame(width: 7, height: 7)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text("This Mac")
                    .font(.caption.weight(.medium))
                Text(session.planeConnectionLabel)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(.bar)
        .accessibilityElement(children: .combine)
    }

    private var connectionColor: Color {
        switch session.connectionState {
        case .connected: .green
        case .connecting, .reconnecting: .orange
        case .failed: .red
        case .disconnected: .secondary
        }
    }
}

private struct ConversationView: View {
    @Bindable var session: DesktopSession
    @FocusState private var composerFocused: Bool

    var body: some View {
        if session.usesLivePlane {
            LiveConversationView(session: session, composerFocused: $composerFocused)
                .onChange(of: session.composerFocusRequest) { _, _ in
                    composerFocused = true
                }
        } else {
            fixtureContent
        }
    }

    @ViewBuilder
    private var fixtureContent: some View {
        switch session.contentState {
        case .loading:
            ProgressView("Loading workspace")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case let .failed(message):
            ContentUnavailableView {
                Label("Workspace unavailable", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            } actions: {
                Button("Try again", action: session.retryFixtureLoad)
            }
        case let .ready(scenario):
            VStack(spacing: 0) {
                ConversationHeader(session: session, scenario: scenario)
                Divider()
                TimelineView(scenario: scenario)
                Divider()
                ComposerView(session: session, scenario: scenario, composerFocused: $composerFocused)
            }
            .navigationTitle(scenario.conversation?.title ?? "New task")
            .onChange(of: session.composerFocusRequest) { _, _ in
                composerFocused = true
            }
        }
    }
}

private struct LiveConversationView: View {
    @Bindable var session: DesktopSession
    let composerFocused: FocusState<Bool>.Binding

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .center, spacing: 14) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(session.selectedConversationTitle)
                        .font(.headline)
                    HStack(spacing: 6) {
                        Text(session.selectedProjectName)
                        Text("·")
                        Text("Runs on This Mac")
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
                Spacer(minLength: 12)
                LiveStatusLabel(session: session)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 13)

            Divider()
            LiveTimelineView(session: session)
            Divider()
            LiveComposerView(session: session, composerFocused: composerFocused)
        }
        .navigationTitle(session.selectedConversationTitle)
    }
}

private struct LiveStatusLabel: View {
    let session: DesktopSession

    var body: some View {
        Label(label, systemImage: symbol)
            .font(.caption.weight(.medium))
            .foregroundStyle(color)
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(color.opacity(0.10), in: Capsule())
            .accessibilityLabel("Task status: \(label)")
    }

    private var label: String {
        if session.conversationFreshness == .cached { return "Offline cache" }
        if session.conversationOperation != nil { return "Loading" }
        switch session.selectedRun?.lifecycle {
        case .starting: return "Starting"
        case .active: return "Working"
        case .stopping: return "Stopping"
        case .completed: return "Completed"
        case .failed: return "Failed"
        case .canceled: return "Canceled"
        case .lost: return "Recovery needed"
        case .created, nil: return "Ready"
        }
    }

    private var symbol: String {
        switch session.selectedRun?.lifecycle {
        case .starting, .active, .stopping: "bolt.horizontal.circle"
        case .completed: "checkmark.circle"
        case .failed, .canceled, .lost: "exclamationmark.circle"
        case .created, nil: "circle"
        }
    }

    private var color: Color {
        if session.conversationFreshness == .cached { return .orange }
        return switch session.selectedRun?.lifecycle {
        case .starting, .active, .stopping:
            Color(red: 41 / 255, green: 182 / 255, blue: 246 / 255)
        case .completed: .green
        case .failed, .canceled, .lost: .orange
        case .created, nil: .secondary
        }
    }
}

private struct LiveTimelineView: View {
    let session: DesktopSession

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                if session.conversationFreshness == .cached {
                    HStack(alignment: .top, spacing: 10) {
                        Image(systemName: "wifi.slash")
                            .foregroundStyle(.orange)
                        VStack(alignment: .leading, spacing: 3) {
                            Text("Showing cached state")
                                .font(.subheadline.weight(.semibold))
                            Text("Jet will refresh this Conversation after the local Plane reconnects.")
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .padding(12)
                    .background(.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
                    .accessibilityElement(children: .combine)
                }

                if session.timeline.isEmpty {
                    ContentUnavailableView {
                        Label(
                            session.selectedConversationID == nil
                                ? "What should Jet do?"
                                : "Live activity starts here",
                            systemImage: "text.bubble"
                        )
                    } description: {
                        Text(
                            session.selectedConversationID == nil
                                ? "Describe the outcome. Jet will create an isolated Workspace in the selected Project."
                                : "The current Run state is restored above. New ordered activity will appear here."
                        )
                    }
                    .frame(maxWidth: .infinity, minHeight: 260)
                } else {
                    ForEach(session.timeline) { entry in
                        LiveTimelineEntryView(entry: entry)
                    }
                }
            }
            .frame(maxWidth: 760)
            .padding(.horizontal, 28)
            .padding(.vertical, 24)
            .frame(maxWidth: .infinity)
        }
        .defaultScrollAnchor(.bottom)
    }
}

private struct LiveTimelineEntryView: View {
    let entry: JetTimelineEntry

    var body: some View {
        switch entry.kind {
        case .user:
            HStack {
                Spacer(minLength: 52)
                Text(entry.text)
                    .textSelection(.enabled)
                    .padding(.horizontal, 13)
                    .padding(.vertical, 9)
                    .background(.quaternary, in: RoundedRectangle(cornerRadius: 13))
            }
        case .activity:
            Label(entry.text, systemImage: "bolt.horizontal.circle")
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityLabel("Activity: \(entry.text)")
        case .result:
            Label(entry.text, systemImage: "checkmark.circle")
                .font(.subheadline)
                .foregroundStyle(.green)
        case .agent:
            Text(entry.text)
                .textSelection(.enabled)
                .font(.body)
                .lineSpacing(3)
        }
    }
}

private struct LiveComposerView: View {
    @Bindable var session: DesktopSession
    let composerFocused: FocusState<Bool>.Binding

    var body: some View {
        VStack(spacing: 8) {
            if let actionNotice = session.actionNotice {
                Text(actionNotice)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: 760, alignment: .leading)
                    .accessibilityLabel(actionNotice)
            }

            HStack(alignment: .bottom, spacing: 10) {
                TextField(
                    "Describe what you want Jet to do",
                    text: $session.draft,
                    axis: .vertical
                )
                .textFieldStyle(.plain)
                .lineLimit(2 ... 6)
                .focused(composerFocused)
                .accessibilityLabel("Task message")

                Button("Send") {
                    Task { await session.submitDraft() }
                }
                .buttonStyle(.borderedProminent)
                .disabled(
                    !session.canSubmitDraft
                        || session.conversationOperation != nil
                        || !session.planeIsConnected
                )
                .keyboardShortcut(.return, modifiers: [.command])
            }
            .padding(12)
            .background(.background, in: RoundedRectangle(cornerRadius: 14))
            .overlay {
                RoundedRectangle(cornerRadius: 14)
                    .stroke(.separator, lineWidth: 1)
            }

            HStack(spacing: 12) {
                ContextValue(label: "Project", value: session.selectedProjectName)
                ContextValue(label: "Agent", value: session.selectedHarnessName)
                ContextValue(label: "Runs on", value: "This Mac")
                Spacer(minLength: 0)
            }
            .frame(maxWidth: 760)
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 24)
        .padding(.top, 12)
        .padding(.bottom, 16)
        .background(.bar)
    }
}

private struct ConversationHeader: View {
    let session: DesktopSession
    let scenario: DesktopFixtureScenario

    var body: some View {
        HStack(alignment: .center, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text(scenario.conversation?.title ?? "New task")
                    .font(.headline)
                HStack(spacing: 6) {
                    Text(session.selectedProjectName)
                    Text("·")
                    Text("Runs on \(scenario.plane.name)")
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            StatusLabel(scenario: scenario)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 13)
    }
}

private struct StatusLabel: View {
    let scenario: DesktopFixtureScenario

    var body: some View {
        Label(label, systemImage: symbol)
            .font(.caption.weight(.medium))
            .foregroundStyle(color)
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(color.opacity(0.10), in: Capsule())
            .accessibilityLabel("Task status: \(label)")
    }

    private var label: String {
        switch scenario.state {
        case .firstLaunch: return "Connecting"
        case .ready: return "Ready"
        case .queued: return "Queued"
        case .completed: return "Completed"
        case .offline: return "Offline"
        case .staleCursor: return "Refreshing"
        case .denied: return "Action denied"
        case .unsupported: return "Unsupported"
        case .recovery: return "Recovery needed"
        case .active, .approval: break
        }

        if let activity = scenario.run?.activity {
            switch activity {
            case .working: return "Working"
            case .waitingForUser: return "Waiting for you"
            case .waitingForApproval: return "Approval needed"
            case .waitingForAuth: return "Sign-in needed"
            case .waitingForQuota: return "Usage limited"
            case .reconnecting: return "Reconnecting"
            }
        } else if scenario.run?.lifecycle == .completed {
            return "Completed"
        } else {
            return "Ready"
        }
    }

    private var symbol: String {
        switch scenario.run?.activity {
        case .working: "sparkles"
        case .waitingForApproval, .waitingForAuth, .waitingForUser: "exclamationmark.circle"
        case .waitingForQuota, .reconnecting: "arrow.clockwise"
        case nil: scenario.run?.lifecycle == .completed ? "checkmark.circle" : "circle"
        }
    }

    private var color: Color {
        switch scenario.run?.activity {
        case .waitingForApproval, .waitingForAuth, .waitingForUser, .waitingForQuota, .reconnecting:
            .orange
        case .working:
            Color(red: 41 / 255, green: 182 / 255, blue: 246 / 255)
        case nil:
            scenario.run?.lifecycle == .completed ? .green : .secondary
        }
    }
}

private struct TimelineView: View {
    let scenario: DesktopFixtureScenario

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                if let notice = scenario.notice {
                    NoticeView(notice: notice)
                }

                if scenario.timeline.isEmpty {
                    ContentUnavailableView {
                        Label("What should Jet do?", systemImage: "text.bubble")
                    } description: {
                        Text("Describe the outcome. You can choose where it runs before sending.")
                    }
                    .frame(maxWidth: .infinity, minHeight: 260)
                } else {
                    ForEach(scenario.timeline, id: \.id) { entry in
                        TimelineEntryView(entry: entry)
                    }
                }
            }
            .frame(maxWidth: 760)
            .padding(.horizontal, 28)
            .padding(.vertical, 24)
            .frame(maxWidth: .infinity)
        }
        .defaultScrollAnchor(.bottom)
    }
}

private struct NoticeView: View {
    let notice: DesktopNoticeFixture

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(color)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                Text(notice.title)
                    .font(.subheadline.weight(.semibold))
                Text(notice.message)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(color.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
        .accessibilityElement(children: .combine)
    }

    private var symbol: String {
        switch notice.tone {
        case .informational: "info.circle"
        case .warning: "exclamationmark.triangle"
        case .critical: "xmark.octagon"
        }
    }

    private var color: Color {
        switch notice.tone {
        case .informational: Color(red: 41 / 255, green: 182 / 255, blue: 246 / 255)
        case .warning: .orange
        case .critical: .red
        }
    }
}

private struct TimelineEntryView: View {
    let entry: DesktopTimelineEntryFixture

    var body: some View {
        switch entry.kind {
        case .user:
            HStack {
                Spacer(minLength: 52)
                Text(entry.text)
                    .textSelection(.enabled)
                    .padding(.horizontal, 13)
                    .padding(.vertical, 9)
                    .background(.quaternary, in: RoundedRectangle(cornerRadius: 13))
            }
        case .activity:
            Label(entry.text, systemImage: "bolt.horizontal.circle")
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityLabel("Activity: \(entry.text)")
        case .approval:
            Label(entry.text, systemImage: "checkmark.shield")
                .font(.subheadline)
                .foregroundStyle(.orange)
        case .result:
            Label(entry.text, systemImage: "checkmark.circle")
                .font(.subheadline)
                .foregroundStyle(.green)
        case .recovery:
            Label(entry.text, systemImage: "lifepreserver")
                .font(.subheadline)
                .foregroundStyle(.orange)
        case .agent:
            Text(entry.text)
                .textSelection(.enabled)
                .font(.body)
                .lineSpacing(3)
        }
    }
}

private struct ComposerView: View {
    @Bindable var session: DesktopSession
    let scenario: DesktopFixtureScenario
    let composerFocused: FocusState<Bool>.Binding

    var body: some View {
        VStack(spacing: 8) {
            if let actionNotice = session.actionNotice {
                Text(actionNotice)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: 760, alignment: .leading)
                    .accessibilityLabel(actionNotice)
            }

            HStack(alignment: .bottom, spacing: 10) {
                TextField(
                    "Describe what you want Jet to do",
                    text: $session.draft,
                    axis: .vertical
                )
                .textFieldStyle(.plain)
                .lineLimit(2 ... 6)
                .focused(composerFocused)
                .accessibilityLabel("Task message")

                Button("Send") {
                    Task { await session.submitDraft() }
                }
                    .buttonStyle(.borderedProminent)
                    .disabled(!session.canSubmitDraft)
                    .keyboardShortcut(.return, modifiers: [.command])
            }
            .padding(12)
            .background(.background, in: RoundedRectangle(cornerRadius: 14))
            .overlay {
                RoundedRectangle(cornerRadius: 14)
                    .stroke(.separator, lineWidth: 1)
            }

            HStack(spacing: 12) {
                ContextValue(label: "Project", value: session.selectedProjectName)
                ContextValue(
                    label: "Agent",
                    value: session.selectedHarnessName
                )
                ContextValue(label: "Runs on", value: scenario.plane.name)
                Spacer(minLength: 0)
            }
            .frame(maxWidth: 760)
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 24)
        .padding(.top, 12)
        .padding(.bottom, 16)
        .background(.bar)
    }
}

private struct ContextValue: View {
    let label: String
    let value: String

    var body: some View {
        HStack(spacing: 4) {
            Text(label)
                .foregroundStyle(.tertiary)
            Text(value)
                .foregroundStyle(.secondary)
        }
        .font(.caption)
        .accessibilityElement(children: .combine)
    }
}

private struct WorkPanelView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(spacing: 0) {
            Picker("Work panel", selection: $session.selectedWorkPanel) {
                ForEach(WorkPanelTab.allCases, id: \.self) { tab in
                    Text(tab.title).tag(tab)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(12)

            Divider()

            Group {
                switch session.selectedWorkPanel {
                case .changes:
                    WorkPanelEmptyState(
                        title: "No change details yet",
                        message: "Changes will load here when the Run exposes a diff.",
                        symbol: "doc.text.magnifyingglass"
                    )
                case .files:
                    WorkPanelEmptyState(
                        title: "No file selected",
                        message: "Choose a file from a Run to inspect it without leaving the task.",
                        symbol: "doc"
                    )
                case .terminal:
                    WorkPanelEmptyState(
                        title: "No terminal open",
                        message: "Workspace terminals will appear here when the Plane provides one.",
                        symbol: "terminal"
                    )
                case .run:
                    RunSummaryView(session: session)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

private struct WorkPanelEmptyState: View {
    let title: String
    let message: String
    let symbol: String

    var body: some View {
        ContentUnavailableView {
            Label(title, systemImage: symbol)
        } description: {
            Text(message)
        }
        .padding()
    }
}

private struct RunSummaryView: View {
    let session: DesktopSession

    var body: some View {
        List {
            Section("Current Run") {
                LabeledContent("Lifecycle", value: lifecycleLabel)
                LabeledContent("Activity", value: session.hasLiveRun ? "Streaming" : "Idle")
                LabeledContent("Runs on", value: "This Mac")
            }

            Section("Queue") {
                Text("Queue controls arrive in Wave 2.1. Submitted Turns still follow Plane order.")
                    .foregroundStyle(.secondary)
            }
        }
        .listStyle(.inset)
    }

    private var lifecycleLabel: String {
        if session.usesLivePlane {
            return session.selectedRun?.lifecycle.rawValue.capitalized ?? "Not started"
        }
        return session.scenario?.run?.lifecycle.rawValue.capitalized ?? "Not started"
    }
}

#Preview {
    DesktopShellView(session: DesktopSession())
        .frame(width: 1280, height: 800)
}
