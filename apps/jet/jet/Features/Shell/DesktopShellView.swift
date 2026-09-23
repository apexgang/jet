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
        .confirmationDialog(
            session.runControlConfirmation == .interruptTurn
                ? "Interrupt this Turn?"
                : "Stop this Run?",
            isPresented: Binding(
                get: { session.runControlConfirmation != nil },
                set: { if !$0 { session.cancelRunControl() } }
            ),
            titleVisibility: .visible
        ) {
            if session.runControlConfirmation == .interruptTurn {
                Button("Interrupt Turn") {
                    Task { await session.confirmRunControl() }
                }
            } else if session.runControlConfirmation == .stopRun {
                Button("Stop Run", role: .destructive) {
                    Task { await session.confirmRunControl() }
                }
            }
            Button("Cancel", role: .cancel, action: session.cancelRunControl)
        } message: {
            Text(
                session.runControlConfirmation == .interruptTurn
                    ? "Jet will end the active Turn. The Run can accept the next queued Turn when native cancellation succeeds."
                    : "Jet will end the whole Run and its native processes. Recorded output and Workspace changes remain available."
            )
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

                HStack {
                    Label("Needs attention", systemImage: "bell")
                    Spacer()
                    if session.attentionCount > 0 {
                        Text(session.attentionCount, format: .number)
                            .font(.caption2.monospacedDigit())
                            .foregroundStyle(.orange)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(.orange.opacity(0.12), in: Capsule())
                            .accessibilityLabel("\(session.attentionCount) items")
                    }
                }
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
        if !session.planeIsConnected { return "Reconnecting" }
        if session.conversationOperation != nil { return "Loading" }
        switch session.runExecution?.activity {
        case .waitingForApproval: return "Approval needed"
        case .waitingForUser: return "Waiting for you"
        case .waitingForAuth: return "Sign-in needed"
        case .waitingForQuota: return "Usage limited"
        case .reconnecting: return "Reconnecting"
        case .working, nil: break
        }
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
        if !session.planeIsConnected { return "arrow.clockwise" }
        if session.runExecution?.needsAttention == true {
            return "exclamationmark.circle"
        }
        return switch session.selectedRun?.lifecycle {
        case .starting, .active, .stopping: "bolt.horizontal.circle"
        case .completed: "checkmark.circle"
        case .failed, .canceled, .lost: "exclamationmark.circle"
        case .created, nil: "circle"
        }
    }

    private var color: Color {
        if session.conversationFreshness == .cached { return .orange }
        if !session.planeIsConnected { return .orange }
        if session.runExecution?.needsAttention == true { return .orange }
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
                        LiveTimelineEntryView(entry: entry, session: session)
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
    let session: DesktopSession

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
            if let approval = entry.approval {
                ApprovalCardView(approval: approval, session: session)
            }
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

private struct ApprovalCardView: View {
    let approval: JetApprovalPresentation
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .firstTextBaseline) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(approval.tool)
                        .font(.headline)
                    Text(stateLabel)
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.orange)
                }
                Spacer(minLength: 12)
                Text(approval.scope)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }

            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 8) {
                GridRow {
                    Text("Target").foregroundStyle(.secondary)
                    Text(approval.target).textSelection(.enabled)
                }
                GridRow {
                    Text("Consequence").foregroundStyle(.secondary)
                    Text(approval.consequence)
                }
            }
            .font(.caption)

            VStack(alignment: .leading, spacing: 5) {
                Text("Requested action")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                Text(approval.action)
                    .font(.caption.monospaced())
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(9)
                    .background(.background, in: RoundedRectangle(cornerRadius: 7))
            }

            if let rationale = approval.rationale {
                Text(rationale)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            HStack {
                if approval.canAuthorizeRetry {
                    Button("Authorize one retry") {
                        Task { await session.authorizeApprovalRetry(approval) }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(session.supervisionOperation != nil)
                } else if approval.state == .requested || approval.state == .unavailable {
                    Text("Approve and Reject need the planned approval-decision protocol command.")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 8)
                Button("Interrupt Turn…") {
                    session.requestRunControl(.interruptTurn)
                }
                .disabled(!session.canInterruptTurn || session.supervisionOperation != nil)
                Button("Stop Run…", role: .destructive) {
                    session.requestRunControl(.stopRun)
                }
                .disabled(!session.canStopRun || session.supervisionOperation != nil)
            }
        }
        .padding(16)
        .background(.orange.opacity(0.07), in: RoundedRectangle(cornerRadius: 12))
        .overlay {
            RoundedRectangle(cornerRadius: 12)
                .stroke(.orange.opacity(0.35), lineWidth: 1)
        }
        .accessibilityElement(children: .contain)
    }

    private var stateLabel: String {
        switch approval.state {
        case .requested: "Approval needed"
        case .allowed: "Action allowed"
        case .denied: "Action denied"
        case .unavailable: "Decision needed"
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

            if session.queueIsFull {
                Label(
                    "The Turn queue is full. Withdraw a queued Turn or wait for one to finish.",
                    systemImage: "exclamationmark.triangle"
                )
                .font(.caption)
                .foregroundStyle(.orange)
                .frame(maxWidth: 760, alignment: .leading)
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
                Text("\(session.draftBytes.formatted()) / \(JetTurnQueue.maximumPromptBytes.formatted()) bytes")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(
                        session.draftBytes > JetTurnQueue.maximumPromptBytes ? .red : .secondary
                    )
                    .accessibilityLabel("Message size")
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
                if let error = session.workError, session.selectedWorkPanel != .run {
                    ContentUnavailableView {
                        Label("Work details unavailable", systemImage: "exclamationmark.triangle")
                    } description: {
                        Text(error.message)
                    } actions: {
                        ForEach(error.recoveryActions) { action in
                            Button(action.label) {
                                Task { await session.applyWorkRecovery(action) }
                            }
                        }
                        if error.recoveryActions.isEmpty, error.retryable {
                            Button("Try Again") { Task { await session.loadWorkPanel() } }
                        }
                    }
                    .padding()
                } else if session.workOperation == "refresh",
                          session.workDiff == nil,
                          session.selectedWorkPanel != .run
                {
                    ProgressView("Loading work details")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    switch session.selectedWorkPanel {
                    case .changes:
                        ChangesWorkView(session: session)
                    case .files:
                        FilesWorkView(session: session)
                    case .terminal:
                        TerminalWorkView(session: session)
                    case .run:
                        RunSummaryView(session: session)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            if let notice = session.workNotice {
                Divider()
                VStack(alignment: .leading, spacing: 8) {
                    Text(notice)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityLabel(notice)
                    if let error = session.workNoticeError {
                        HStack(spacing: 8) {
                            ForEach(error.recoveryActions) { action in
                                Button(action.label) {
                                    Task { await session.applyWorkRecovery(action) }
                                }
                                .controlSize(.small)
                            }
                            if let conflict = error.revisionConflict {
                                Text("Current revision \(conflict.currentRevision)")
                                    .font(.caption2)
                                    .foregroundStyle(.tertiary)
                            }
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
                .background(.bar)
            }
        }
    }
}

private struct ChangesWorkView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                WorkSectionHeader(
                    title: "Changed files",
                    detail: "\(session.workDiff?.scope.label ?? "Current") checkpoint · \(session.workDiff?.totalFiles ?? 0) files"
                ) {
                    Button("Refresh") { Task { await session.loadWorkPanel() } }
                        .disabled(session.workOperation != nil)
                }
                .id("changes-heading")

                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        Picker("Checkpoint", selection: $session.checkpointKind) {
                            ForEach(WorkCheckpointKind.allCases, id: \.self) { kind in
                                Text(kind.title)
                                    .tag(kind)
                                    .disabled(kind == .final && session.selectedRun?.lifecycle.isLive == true)
                            }
                        }
                        .pickerStyle(.menu)
                        Spacer(minLength: 4)
                        Button("Apply") {
                            Task { await session.applyWorkCheckpoint() }
                        }
                        .disabled(!session.canApplyWorkCheckpoint)
                    }
                    switch session.checkpointKind {
                    case .turn:
                        TextField("Turn", value: $session.checkpointTurn, format: .number)
                            .textFieldStyle(.roundedBorder)
                    case .historical:
                        HStack(spacing: 8) {
                            TextField("From", value: $session.checkpointFromTurn, format: .number)
                            TextField("To", value: $session.checkpointToTurn, format: .number)
                        }
                        .textFieldStyle(.roundedBorder)
                    case .current, .final:
                        EmptyView()
                    }
                }
                .controlSize(.small)
                .padding(.horizontal, 14)
                .padding(.bottom, 10)

                if session.workFiles.isEmpty {
                    WorkPanelEmptyState(
                        title: "No changes recorded",
                        message: "The selected Run has not produced a file change at this checkpoint.",
                        symbol: "checkmark.circle"
                    )
                    .frame(minHeight: 180)
                } else {
                    ForEach(session.workFiles) { file in
                        Button {
                            Task { await session.selectWorkFile(file.path) }
                        } label: {
                            HStack(spacing: 8) {
                                Text(file.status.prefix(1).uppercased())
                                    .font(.caption2.bold().monospaced())
                                    .foregroundStyle(statusColor(file.status))
                                    .frame(width: 16)
                                Text(file.path)
                                    .font(.caption)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                Spacer(minLength: 4)
                                Text(file.origin)
                                    .font(.caption2)
                                    .foregroundStyle(.tertiary)
                            }
                            .contentShape(Rectangle())
                            .padding(.horizontal, 14)
                            .padding(.vertical, 7)
                            .background(
                                session.selectedWorkFilePath == file.path
                                    ? Color.accentColor.opacity(0.12)
                                    : Color.clear
                            )
                        }
                        .buttonStyle(.plain)
                        .id("file-\(file.path)")
                    }
                    if session.workNextPage != nil {
                        Button("Load more files") {
                            Task { await session.loadMoreWorkFiles() }
                        }
                        .disabled(session.workOperation != nil)
                        .frame(maxWidth: .infinity)
                        .padding(12)
                    }
                }

                Divider().padding(.top, 6)
                WorkSectionHeader(
                    title: "Patch",
                    detail: ByteCountFormatter.string(
                        fromByteCount: Int64(session.workDiff?.artifact.size ?? 0),
                        countStyle: .file
                    ) + " retained artifact"
                )
                .id("patch-heading")

                if patchIsBinary {
                    WorkPanelEmptyState(
                        title: "Binary change",
                        message: "Jet records the file change, but this patch is not readable as text.",
                        symbol: "doc.badge.ellipsis"
                    )
                    .frame(minHeight: 160)
                } else if session.workPatch.isEmpty {
                    Text("No text patch is available.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(16)
                } else {
                    ScrollView(.horizontal) {
                        Text(session.workPatch)
                            .font(.system(.caption2, design: .monospaced))
                            .textSelection(.enabled)
                            .fixedSize(horizontal: true, vertical: false)
                            .padding(14)
                    }
                    .frame(maxHeight: 420)
                    .background(Color.primary.opacity(0.035))
                }

                if let diff = session.workDiff,
                   diff.patchTruncated,
                   session.workPatchBytesLoaded < diff.artifact.size
                {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(artifactMessage(diff.artifact.availability))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        if diff.artifact.availability == .stored {
                            Button("Load next chunk") {
                                Task { await session.loadMorePatch() }
                            }
                            .disabled(session.workOperation != nil)
                        }
                    }
                    .padding(14)
                    .id("artifact-state")
                }

                Divider().padding(.top, 6)
                DeliveryWorkView(session: session)
                    .id("delivery-heading")
            }
            .scrollTargetLayout()
        }
        .scrollPosition(id: scrollAnchor)
    }

    private var scrollAnchor: Binding<String?> {
        Binding(
            get: { session.workScrollAnchors[.changes] },
            set: { session.workScrollAnchors[.changes] = $0 }
        )
    }

    private var patchIsBinary: Bool {
        session.workPatch.contains("GIT binary patch")
            || session.workPatch.contains("Binary files")
    }

    private func statusColor(_ status: String) -> Color {
        switch status {
        case "added": .green
        case "deleted": .red
        default: .orange
        }
    }

    private func artifactMessage(_ availability: JetArtifactAvailability) -> String {
        switch availability {
        case .stored: "The complete patch can be loaded in verified chunks."
        case .diskPressure: "The full patch was not retained because storage is constrained."
        case .runBudgetExceeded: "The Run reached its retained-artifact budget."
        case .artifactSizeExceeded: "The complete patch exceeds the artifact size limit."
        }
    }
}

private struct DeliveryWorkView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            WorkSectionHeader(
                title: "Delivery",
                detail: "Authoritative Git outcomes from this Conversation"
            ) {
                Button("Refresh") { Task { await session.loadGitDeliveries() } }
                    .disabled(session.gitDeliveryOperation != nil)
            }
            .padding(.horizontal, -14)
            .padding(.vertical, -12)

            if let reason = session.gitDeliveryUnavailableReason {
                Label(reason, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Picker("Operation", selection: $session.gitDeliveryChoice) {
                ForEach(JetGitDeliveryChoice.allCases, id: \.self) { choice in
                    Text(choice.title).tag(choice)
                }
            }
            .pickerStyle(.menu)

            deliveryFields

            if let request = previewRequest {
                LabeledContent("Destination", value: request.operation.destinationLabel)
                    .font(.caption)
                if let checkpoint = request.checkpoint {
                    LabeledContent("Checkpoint", value: checkpoint.label)
                        .font(.caption)
                }
            }

            HStack {
                Spacer()
                Button("Review \(session.gitDeliveryChoice.title)…") {
                    session.prepareGitDelivery()
                }
                .buttonStyle(.borderedProminent)
                .disabled(!session.canPrepareGitDelivery)
                .accessibilityIdentifier("git-delivery-review")
            }

            if let uncertain = session.gitDeliveryAdmissionUncertain {
                let reviewSummary: String = uncertain.reviewSummary
                VStack(alignment: .leading, spacing: 8) {
                    Label("Request admission is unknown", systemImage: "questionmark.diamond")
                        .font(.subheadline.weight(.semibold))
                    Text("Check the durable history first. Retrying uses the same command identity and unchanged request.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Text(reviewSummary)
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                    HStack {
                        Button("Check Status") { Task { await session.loadGitDeliveries() } }
                        Button("Retry Same Request") {
                            Task { await session.retryGitDeliveryAdmission() }
                        }
                    }
                    .controlSize(.small)
                }
                .padding(10)
                .background(.orange.opacity(0.1), in: RoundedRectangle(cornerRadius: 8))
            }

            if let notice = session.gitDeliveryNotice {
                Text(notice)
                    .font(.caption)
                    .foregroundStyle(deliveryNoticeColor)
                    .accessibilityLabel(notice)
            }

            Divider()

            Text("Delivery history")
                .font(.subheadline.weight(.semibold))

            if session.gitDeliveryOperation == "refresh", session.gitDeliveries.isEmpty {
                ProgressView("Loading delivery history")
                    .controlSize(.small)
            } else if session.gitDeliveries.isEmpty {
                Text("No Git delivery has been requested for this Conversation.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(session.gitDeliveries) { delivery in
                    GitDeliveryRow(session: session, delivery: delivery)
                }
            }
        }
        .controlSize(.small)
        .padding(14)
        .confirmationDialog(
            session.gitDeliveryConfirmation.map { "Review \($0.operation.title)?" }
                ?? "Review delivery?",
            isPresented: Binding(
                get: { session.gitDeliveryConfirmation != nil },
                set: { if !$0 { session.cancelGitDeliveryConfirmation() } }
            ),
            titleVisibility: .visible
        ) {
            Button(session.gitDeliveryConfirmation?.operation.title ?? "Deliver") {
                Task { await session.confirmGitDelivery() }
            }
            Button("Cancel", role: .cancel, action: session.cancelGitDeliveryConfirmation)
        } message: {
            Text(
                session.gitDeliveryConfirmation.map {
                    "Destination: \($0.operation.destinationLabel). \($0.checkpoint?.label ?? "No checkpoint content"). Jet queues this as a durable Effect and reports each outcome separately."
                } ?? "Review the destination before delivery."
            )
        }
        .confirmationDialog(
            "Mark this outcome as reviewed?",
            isPresented: Binding(
                get: { session.gitDeliveryAcknowledgementConfirmation != nil },
                set: { if !$0 { session.cancelGitDeliveryAcknowledgement() } }
            ),
            titleVisibility: .visible
        ) {
            Button("Mark Reviewed") {
                Task { await session.confirmGitDeliveryAcknowledgement() }
            }
            Button("Cancel", role: .cancel, action: session.cancelGitDeliveryAcknowledgement)
        } message: {
            Text("Jet will release the uncertainty barrier after your review. It will not repeat the Git operation or claim that it succeeded or failed.")
        }
    }

    @ViewBuilder
    private var deliveryFields: some View {
        switch session.gitDeliveryChoice {
        case .branch:
            TextField("New branch name", text: $session.gitBranchName)
                .textFieldStyle(.roundedBorder)
        case .commit:
            Text("Jet commits the latest retained Turn checkpoint. The Plane generates and records the exact commit message.")
                .font(.caption)
                .foregroundStyle(.secondary)
        case .push:
            TextField("Remote", text: $session.gitRemoteName)
                .textFieldStyle(.roundedBorder)
        case .draftPullRequest:
            TextField("GitHub remote", text: $session.gitRemoteName)
                .textFieldStyle(.roundedBorder)
            TextField("Base branch, or leave empty for repository default", text: $session.gitBaseBranch)
                .textFieldStyle(.roundedBorder)
        }
    }

    private var previewRequest: JetGitDeliveryRequest? {
        guard let conversationID = session.selectedConversationID,
              let diff = session.workDiff
        else { return nil }
        let checkpoint = JetGitCheckpoint(runID: diff.runID, turn: diff.latestTurn)
        switch session.gitDeliveryChoice {
        case .branch:
            return JetGitDeliveryRequest(
                conversationID: conversationID,
                checkpoint: nil,
                operation: .branch(name: session.gitBranchName.isEmpty ? "Enter a branch" : session.gitBranchName)
            )
        case .commit:
            return JetGitDeliveryRequest(
                conversationID: conversationID,
                checkpoint: checkpoint,
                operation: .commit
            )
        case .push:
            return JetGitDeliveryRequest(
                conversationID: conversationID,
                checkpoint: nil,
                operation: .push(remote: session.gitRemoteName)
            )
        case .draftPullRequest:
            return JetGitDeliveryRequest(
                conversationID: conversationID,
                checkpoint: checkpoint,
                operation: .draftPullRequest(
                    remote: session.gitRemoteName,
                    base: session.gitBaseBranch.isEmpty ? nil : session.gitBaseBranch
                )
            )
        }
    }

    private var deliveryNoticeColor: Color {
        session.gitDeliveryError == nil ? .secondary : .red
    }
}

private struct GitDeliveryRow: View {
    let session: DesktopSession
    let delivery: JetGitDelivery

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Label(delivery.operation.title, systemImage: statusSymbol)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(statusColor)
                Spacer()
                Text(delivery.outcome.title)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            Text(delivery.operation.destinationLabel)
                .font(.caption2.monospaced())
                .textSelection(.enabled)
            if let checkpoint = delivery.checkpoint {
                Text("\(checkpoint.label) · Run \(checkpoint.runID.uuidString.prefix(8))")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            if let message = delivery.message {
                Text(message.title)
                    .font(.caption)
                    .lineLimit(2)
            }
            outcomeDetails
            if delivery.canRetry || delivery.needsAcknowledgement {
                HStack {
                    Spacer()
                    if delivery.canRetry {
                        Button("Review Retry…") { session.reviewRetry(delivery) }
                    }
                    if delivery.needsAcknowledgement {
                        Button("Mark Reviewed…") {
                            session.reviewGitDeliveryAcknowledgement(delivery)
                        }
                    }
                }
            }
        }
        .padding(10)
        .background(Color.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("git-delivery-\(delivery.id.uuidString.lowercased())")
    }

    @ViewBuilder
    private var outcomeDetails: some View {
        switch delivery.outcome {
        case .pending:
            Text("The outbox accepted this Effect and is still reconciling its result.")
                .font(.caption2)
                .foregroundStyle(.secondary)
        case let .completed(head, branch, pullRequest):
            Text("HEAD \(head.prefix(12))")
                .font(.caption2.monospaced())
            if let branch { Text("Branch \(branch)").font(.caption2.monospaced()) }
            if let pullRequest {
                Text(pullRequest)
                    .font(.caption2.monospaced())
                    .textSelection(.enabled)
            }
        case let .failed(code):
            Text("Jet confirmed failure: \(code)")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        case .outcomeUnknown:
            Text(
                delivery.acknowledgedBy == nil
                    ? "Jet could not establish whether the external operation happened. It will not retry automatically."
                    : "A user reviewed this unknown outcome. Jet did not repeat the operation."
            )
            .font(.caption2)
            .foregroundStyle(.secondary)
        }
    }

    private var statusSymbol: String {
        switch delivery.outcome {
        case .pending: "clock"
        case .completed: "checkmark.circle.fill"
        case .failed: "xmark.circle.fill"
        case .outcomeUnknown: "questionmark.diamond.fill"
        }
    }

    private var statusColor: Color {
        switch delivery.outcome {
        case .pending: .secondary
        case .completed: .green
        case .failed: .red
        case .outcomeUnknown: .orange
        }
    }
}

private struct FilesWorkView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(spacing: 0) {
            WorkSectionHeader(
                title: "Workspace file",
                detail: session.editableFile?.path ?? "Choose a changed file"
            ) {
                if !session.workFiles.isEmpty {
                    Menu("Choose") {
                        ForEach(session.workFiles) { file in
                            Button(file.path) { Task { await session.selectWorkFile(file.path) } }
                        }
                    }
                }
            }
            Divider()

            if session.selectedWorkFilePath == nil {
                WorkPanelEmptyState(
                    title: "No file selected",
                    message: "Choose a file in Changes. Only files from this Run can be opened here.",
                    symbol: "doc"
                )
            } else if session.workOperation == "file", session.editableFile == nil {
                ProgressView("Loading file")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let file = session.editableFile, file.content != nil {
                VStack(spacing: 0) {
                    HStack {
                        Text("\(file.content?.utf8.count ?? 0) bytes")
                        Spacer()
                        Text("Revision bound")
                            .help(file.revision.label)
                    }
                    .font(.caption2.monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)

                    TextEditor(text: $session.fileDraft)
                        .font(.system(.caption, design: .monospaced))
                        .scrollContentBackground(.hidden)
                        .padding(8)
                        .background(Color.primary.opacity(0.035))
                        .accessibilityLabel("Edit \(file.path)")

                    HStack {
                        Button("Reload") { Task { await session.selectWorkFile(file.path) } }
                        Spacer()
                        Button("Save Edit") { Task { await session.saveSelectedWorkFile() } }
                            .buttonStyle(.borderedProminent)
                            .disabled(session.workOperation != nil || session.fileDraft == file.content)
                    }
                    .padding(12)

                    Divider()
                    VStack(alignment: .leading, spacing: 9) {
                        Text("Review comment").font(.subheadline.weight(.semibold))
                        TextField("Line", value: $session.reviewLine, format: .number)
                            .textFieldStyle(.roundedBorder)
                        TextField(
                            "Describe the issue or requested change",
                            text: $session.reviewComment,
                            axis: .vertical
                        )
                        .lineLimit(3 ... 6)
                        .textFieldStyle(.roundedBorder)
                        HStack {
                            Spacer()
                            Button("Add Review Comment") {
                                Task { await session.submitSelectedReview() }
                            }
                            .disabled(
                                session.workOperation != nil
                                    || session.reviewLine == 0
                                    || session.reviewComment.trimmingCharacters(
                                        in: .whitespacesAndNewlines
                                    ).isEmpty
                            )
                        }
                    }
                    .padding(14)
                }
            } else {
                WorkPanelEmptyState(
                    title: "Content unavailable",
                    message: "The file may be binary, oversized, deleted, or no longer available as bounded UTF-8 text.",
                    symbol: "doc.badge.ellipsis"
                )
            }
        }
    }
}

private struct TerminalWorkView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(spacing: 0) {
            WorkSectionHeader(
                title: "Workspace terminal",
                detail: session.workDiff?.workspaceID == nil
                    ? "Requires a managed Workspace"
                    : "Scoped to this managed Workspace"
            ) {
                Button("New") { Task { await session.createWorkspaceTerminal() } }
                    .disabled(session.workDiff?.workspaceID == nil || session.workOperation != nil)
            }
            Divider()

            if session.workDiff?.workspaceID == nil {
                WorkPanelEmptyState(
                    title: "No managed Workspace",
                    message: "This Run does not expose a terminal-capable Workspace.",
                    symbol: "terminal"
                )
            } else if session.workTerminals.isEmpty {
                WorkPanelEmptyState(
                    title: "No terminal open",
                    message: "Create a terminal owned by this Workspace. Jet does not launch an unrestricted host shell.",
                    symbol: "terminal"
                )
            } else {
                Picker("Session", selection: $session.selectedTerminalID) {
                    ForEach(session.workTerminals) { terminal in
                        Text("\(terminal.id.uuidString.prefix(8)) · \(terminal.state.rawValue)")
                            .tag(Optional(terminal.id))
                    }
                }
                .padding(12)

                GeometryReader { geometry in
                    ScrollView([.horizontal, .vertical]) {
                        Text(
                            session.selectedTerminalID.flatMap { session.terminalOutput[$0] }
                                ?? "Terminal output will appear here."
                        )
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .topLeading)
                        .padding(12)
                    }
                    .onAppear {
                        session.setTerminalGeometry(
                            width: geometry.size.width,
                            height: geometry.size.height
                        )
                    }
                    .onChange(of: geometry.size) { _, size in
                        session.setTerminalGeometry(width: size.width, height: size.height)
                    }
                }
                .defaultScrollAnchor(.bottom)
                .background(Color.primary.opacity(0.035))

                HStack(spacing: 8) {
                    TextField("Send input", text: $session.terminalInput)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(.caption, design: .monospaced))
                        .disabled(session.attachedTerminalID == nil)
                        .onSubmit { Task { await session.sendTerminalLine() } }
                    Button("Send") { Task { await session.sendTerminalLine() } }
                        .disabled(session.attachedTerminalID == nil || session.terminalInput.isEmpty)
                }
                .padding(12)

                HStack {
                    if session.attachedTerminalID == session.selectedTerminalID {
                        Button("Detach", action: session.detachSelectedTerminal)
                    } else {
                        Button("Attach") { Task { await session.attachSelectedTerminal() } }
                            .disabled(selectedTerminal?.state != .open)
                    }
                    Spacer()
                    Button("Close", role: .destructive) {
                        Task { await session.closeSelectedTerminal() }
                    }
                    .disabled(session.selectedTerminalID == nil || session.workOperation != nil)
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 12)
            }
        }
    }

    private var selectedTerminal: JetWorkspaceTerminal? {
        guard let selectedTerminalID = session.selectedTerminalID else { return nil }
        return session.workTerminals.first { $0.id == selectedTerminalID }
    }
}

private struct WorkSectionHeader<Actions: View>: View {
    let title: String
    let detail: String
    let actions: Actions

    init(
        title: String,
        detail: String,
        @ViewBuilder actions: () -> Actions
    ) {
        self.title = title
        self.detail = detail
        self.actions = actions()
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.subheadline.weight(.semibold))
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 6)
            actions
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }
}

private extension WorkSectionHeader where Actions == EmptyView {
    init(title: String, detail: String) {
        self.init(title: title, detail: detail) { EmptyView() }
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
    @Bindable var session: DesktopSession

    var body: some View {
        List {
            Section("Current Run") {
                LabeledContent("Lifecycle", value: lifecycleLabel)
                LabeledContent("Activity", value: activityLabel)
                LabeledContent("Runs on", value: "This Mac")
                LabeledContent(
                    "Checkpoint",
                    value: session.workDiff?.scope.label ?? "Unavailable"
                )
                LabeledContent(
                    "Latest Turn",
                    value: session.workDiff?.latestTurn.formatted() ?? "—"
                )
                LabeledContent(
                    "Changed files",
                    value: session.workDiff?.totalFiles.formatted() ?? "0"
                )
                if let run = session.selectedRun {
                    LabeledContent("Revision", value: run.revision.formatted())
                }
                if let termination = session.runExecution?.termination {
                    Text(termination.summary)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                HStack {
                    Button("Interrupt Turn…") {
                        session.requestRunControl(.interruptTurn)
                    }
                    .disabled(!session.canInterruptTurn || session.supervisionOperation != nil)

                    Button("Stop Run…", role: .destructive) {
                        session.requestRunControl(.stopRun)
                    }
                    .disabled(!session.canStopRun || session.supervisionOperation != nil)
                }
            }

            Section("Turn queue") {
                if session.supervisionOperation == "refresh", session.turnQueue == nil {
                    ProgressView("Loading the authoritative queue")
                } else if let turns = session.turnQueue?.turns, !turns.isEmpty {
                    ForEach(turns) { turn in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(turn.state == .active ? "Current Turn" : "Position \(turn.position)")
                                    .font(.subheadline.weight(.semibold))
                                Text("\(turn.source.rawValue.replacingOccurrences(of: "_", with: " ")) · \(turn.targetLabel)")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                Text("Turn \(turn.sequence) · \(turn.state.rawValue.replacingOccurrences(of: "_", with: " "))")
                                    .font(.caption2)
                                    .foregroundStyle(.tertiary)
                            }
                            Spacer(minLength: 8)
                            if turn.withdrawable {
                                Button("Withdraw") {
                                    Task { await session.withdrawTurn(turn) }
                                }
                                .disabled(session.supervisionOperation != nil)
                            }
                        }
                    }
                } else {
                    Text("No active or queued Turns.")
                        .foregroundStyle(.secondary)
                }
                Text("Up to \(JetTurnQueue.maximumEntries) unsettled Turns; each prompt can contain \(JetTurnQueue.maximumPromptBytes.formatted()) UTF-8 bytes.")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
        }
        .listStyle(.inset)
        .refreshable { await session.loadWorkPanel() }
    }

    private var lifecycleLabel: String {
        if session.usesLivePlane {
            return session.selectedRun?.lifecycle.rawValue.capitalized ?? "Not started"
        }
        return session.scenario?.run?.lifecycle.rawValue.capitalized ?? "Not started"
    }

    private var activityLabel: String {
        session.runExecution?.activity?.rawValue
            .replacingOccurrences(of: "_", with: " ")
            .capitalized
            ?? (session.hasLiveRun ? "Starting" : "Idle")
    }
}

#Preview {
    DesktopShellView(session: DesktopSession())
        .frame(width: 1280, height: 800)
}

@MainActor
private func makeDeliveryPreviewSession() -> DesktopSession {
    let conversationID = UUID(uuidString: "00000000-0000-0000-0000-000000000020")!
    let checkpoint = JetGitCheckpoint(
        runID: UUID(uuidString: "00000000-0000-0000-0000-000000000010")!,
        turn: 3
    )
    let policy = JetGitDeliveryPolicy(
        automatic: false,
        branch: true,
        commit: true,
        push: true,
        draftPullRequest: true,
        branchPrefix: "jet/"
    )
    let session = DesktopSession()
    session.gitDeliveries = [
        JetGitDelivery(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000040")!,
            conversationID: conversationID,
            checkpoint: checkpoint,
            operation: .commit,
            policy: policy,
            utilityJobID: nil,
            message: JetGitMessage(title: "Updated desktop delivery", body: "", fallbackReason: nil),
            acknowledgedBy: nil,
            outcome: .pending
        ),
        JetGitDelivery(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000041")!,
            conversationID: conversationID,
            checkpoint: checkpoint,
            operation: .draftPullRequest(remote: "origin", base: "main"),
            policy: policy,
            utilityJobID: nil,
            message: JetGitMessage(title: "Updated desktop delivery", body: "", fallbackReason: nil),
            acknowledgedBy: nil,
            outcome: .completed(
                head: String(repeating: "a", count: 40),
                branch: "jet/delivery",
                pullRequest: "https://github.com/example/jet/pull/7"
            )
        ),
        JetGitDelivery(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000042")!,
            conversationID: conversationID,
            checkpoint: nil,
            operation: .push(remote: "origin"),
            policy: policy,
            utilityJobID: nil,
            message: nil,
            acknowledgedBy: nil,
            outcome: .failed(code: "git.policy_changed")
        ),
        JetGitDelivery(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000043")!,
            conversationID: conversationID,
            checkpoint: nil,
            operation: .branch(name: "jet/reviewed"),
            policy: policy,
            utilityJobID: nil,
            message: nil,
            acknowledgedBy: nil,
            outcome: .outcomeUnknown
        ),
    ]

    return session
}

#Preview("Delivery outcomes") {
    ScrollView {
        DeliveryWorkView(session: makeDeliveryPreviewSession())
    }
    .frame(width: 420, height: 760)
}
