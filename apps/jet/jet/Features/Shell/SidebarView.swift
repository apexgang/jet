import SwiftUI

/// The sidebar (design §6.3): New Task, Needs You, Tasks, Projects and Jet Trash
/// in one native selectable list, with Search Tasks. The session derives every
/// row; this view only lays them out and forwards the person's actions.
struct SidebarView: View {
    @Bindable var session: DesktopSession

    @AppStorage("jet.sidebar.projects-expanded") private var projectsExpanded = true
    @FocusState private var isSearchFieldFocused: Bool
    @State private var isSearchPresented = false
    /// The query the session's search result answers, or nil while it answers none.
    @State private var searchedQuery: String?
    /// The ⌘K request already handled, so a request from before this view appeared
    /// doesn't steal focus.
    @State private var handledSearchFocusRequest: Int?
    @State private var retryingComputers: Set<UUID> = []
    @State private var isRetryingList = false
    @State private var dropTarget: SidebarDropTarget?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ScrollViewReader { proxy in
            list
                .onChange(of: session.selectedConversationID) { _, conversationID in
                    guard let conversationID else { return }
                    withAnimation(reduceMotion ? nil : .default) {
                        proxy.scrollTo(conversationID)
                    }
                }
        }
        .searchable(
            text: $session.searchText,
            isPresented: $isSearchPresented,
            placement: .sidebar,
            prompt: Text("Search Tasks")
        )
        .searchFocused($isSearchFieldFocused)
        .onSubmit(of: .search) {
            searchedQuery = nil
            Task { await search(immediately: true) }
        }
        .task(id: session.searchText) {
            await search(immediately: false)
        }
        .task(id: session.searchFocusRequest) {
            await focusSearch()
        }
        // Menu commands such as ⌘⌫ stay off while the person types a search. The
        // field lives in the sidebar's bar, so the scene carries the value.
        .focusedSceneValue(\.isEditingText, isSearchFieldFocused ? true : nil)
        .toolbar {
            ToolbarItem {
                Button {
                    session.beginNewTaskFromSidebar()
                } label: {
                    Label("New Task", systemImage: "square.and.pencil")
                }
                .help("New Task (⌘N)")
            }
        }
#if !os(macOS)
        .navigationTitle("Jet")
#endif
    }

    // MARK: - List

    private var selection: Binding<SidebarItem?> {
        Binding(
            get: { session.sidebarItem },
            set: { item in
                // Clicking empty space never clears the destination.
                if let item { session.selectFromSidebar(item) }
            }
        )
    }

    private var list: some View {
        let presentation = session.isSidebarSearching
            ? session.sidebarSearchPresentation(resultQuery: searchedQuery)
            : nil
        return List(selection: selection) {
            if let presentation {
                searchContent(presentation)
            } else {
                libraryContent
            }
        }
        .listStyle(.sidebar)
        .overlay {
            if let presentation, presentation.showsNoResults {
                ContentUnavailableView.search(text: session.sidebarSearchQuery)
            }
        }
        .animation(reduceMotion ? nil : .default, value: session.needsYouConversationIDs)
#if os(macOS)
        .contextMenu(forSelectionType: SidebarItem.self) { items in
            menu(for: items)
        } primaryAction: { items in
            activate(items)
        }
        .onKeyPress(.return) {
            guard !isSearchFieldFocused else { return .ignored }
            return session.activateSidebarItem(session.sidebarItem) ? .handled : .ignored
        }
        .onDeleteCommand {
            guard !isSearchFieldFocused,
                  case let .task(conversationID)? = session.sidebarItem,
                  session.sidebarCanChangeTask(conversationID)
            else { return }
            session.moveToTrashFromSidebar(conversationID)
        }
#else
        .contextMenu(forSelectionType: SidebarItem.self) { items in
            menu(for: items)
        }
#endif
        .accessibilityIdentifier("sidebar")
    }

    // MARK: - Library

    @ViewBuilder
    private var libraryContent: some View {
        ForEach(session.sidebarOfflineComputers) { plane in
            SidebarNoticeRow(
                systemImage: "wifi.slash",
                tint: .secondary,
                text: String(localized: "\(plane.name) is offline"),
                isWorking: retryingComputers.contains(plane.id)
            ) {
                retryComputer(plane.id)
            }
            .accessibilityIdentifier("sidebar-computer-offline-\(plane.id.uuidString.lowercased())")
        }

        SidebarNewTaskRow(hasDraft: session.hasNewTaskDraft)
            .tag(SidebarItem.newTask)
            .accessibilityIdentifier("sidebar-new-task")
            .sidebarFolderDrop(.newTask, current: $dropTarget, perform: session.addProjectFromSidebarDrop)

        let sections = session.sidebarSections
        if !sections.needsYou.isEmpty {
            Section("Needs You") {
                ForEach(sections.needsYou) { taskRow($0) }
            }
        }
        tasksSection(sections.tasks)

        if session.usesLivePlane {
            // Until a computer reports its projects, the list is unknown, not empty.
            if session.sidebarKnowsProjects {
                projectsSection
            }
            Section {
                Label("Jet Trash", systemImage: "trash")
                    .tag(SidebarItem.trash)
                    .accessibilityIdentifier("sidebar-trash")
            }
        }
    }

    @ViewBuilder
    private func tasksSection(_ tasks: [JetConversationSummary]) -> some View {
        switch session.sidebarListState {
        case .loading:
            Section("Tasks") {
                ForEach(0 ..< 4, id: \.self) { SidebarPlaceholderRow(index: $0) }
            }
        case .failed:
            Section("Tasks") {
                SidebarNoticeRow(
                    systemImage: "exclamationmark.triangle.fill",
                    tint: .orange,
                    text: String(localized: "Couldn't load tasks"),
                    isWorking: isRetryingList,
                    action: retryList
                )
            }
        case .empty:
            Section("Tasks") {
                Text("Tasks you start appear here.")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
        case .ready:
            // Every loaded task may be in Needs You; an empty Tasks header adds nothing.
            if !tasks.isEmpty || session.hasMoreConversations {
                Section("Tasks") {
                    ForEach(tasks) { taskRow($0) }
                    if session.hasMoreConversations {
                        showEarlierTasksButton
                    }
                }
            }
        }
    }

    private var showEarlierTasksButton: some View {
        Button {
            Task { await session.loadMoreConversations() }
        } label: {
            HStack(spacing: 6) {
                Text("Show Earlier Tasks")
                    .foregroundStyle(JetDesign.accentText)
                if session.isLoadingMoreConversations {
                    ProgressView()
                        .controlSize(.small)
                }
            }
        }
        .buttonStyle(.borderless)
        .disabled(session.isLoadingMoreConversations)
        .accessibilityIdentifier("sidebar-show-earlier-tasks")
    }

    private var projectsSection: some View {
        Section(isExpanded: $projectsExpanded) {
            let projects = session.sidebarProjects
            if projects.isEmpty {
                Button {
                    session.presentAddProject(droppedURL: nil)
                } label: {
                    Label("Add Project…", systemImage: "plus")
                }
                .buttonStyle(.borderless)
                .tint(JetDesign.accentText)
            } else {
                ForEach(projects) { projectRow($0) }
            }
        } header: {
            HStack(spacing: 4) {
                Text("Projects")
                Spacer(minLength: 0)
                Button {
                    session.presentAddProject(droppedURL: nil)
                } label: {
                    Image(systemName: "plus")
                        .frame(width: 28, height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("Add Project…")
                .accessibilityLabel("Add Project…")
                .accessibilityIdentifier("sidebar-add-project")
            }
            .sidebarFolderDrop(.projects, current: $dropTarget, perform: session.addProjectFromSidebarDrop)
        }
    }

    private func projectRow(_ project: JetPlaneProject) -> some View {
        let id = project.project.id
        return SidebarProjectRow(
            name: project.project.name,
            detail: session.sidebarProjectDetail(project),
            root: project.project.root,
            isDimmed: session.isComputerOffline(project.planeRegistryID)
        )
        .tag(SidebarItem.project(id))
        .accessibilityIdentifier("sidebar-project-\(id.uuidString.lowercased())")
        .sidebarFolderDrop(.project(id), current: $dropTarget, perform: session.addProjectFromSidebarDrop)
    }

    // MARK: - Task rows

    private func taskRow(_ conversation: JetConversationSummary) -> some View {
        let id = conversation.id
        return TimelineView(.everyMinute) { context in
            SidebarTaskRow(presentation: session.sidebarRowPresentation(for: conversation, now: context.date))
                .accessibilityIdentifier("sidebar-task-\(id.uuidString.lowercased())")
        }
        .tag(SidebarItem.task(id))
        .onAppear {
            session.statusStore.noteVisible(
                id,
                planeRegistryID: session.planeRegistryID(for: id) ?? session.localPlaneRegistryID
            )
        }
        // WP4: .onDisappear { session.statusStore.noteHidden(id) }
    }

    // MARK: - Search

    @ViewBuilder
    private func searchContent(_ presentation: SidebarSearchPresentation) -> some View {
        if !presentation.titleMatches.isEmpty {
            Section("Tasks") {
                ForEach(presentation.titleMatches) { taskRow($0) }
            }
        }
        if presentation.isPending || !presentation.otherMatches.isEmpty || !presentation.failures.isEmpty {
            Section {
                ForEach(presentation.otherMatches) { match in
                    SidebarOtherMatchRow(
                        match: match,
                        status: session.taskStatus(for: match.conversationID),
                        isUnread: session.isUnread(match.conversationID)
                    )
                    .tag(SidebarItem.task(match.conversationID))
                    .accessibilityIdentifier("sidebar-match-\(match.conversationID.uuidString.lowercased())")
                }
                ForEach(presentation.failures) { failure in
                    SidebarNoticeRow(
                        systemImage: "exclamationmark.triangle.fill",
                        tint: .orange,
                        text: String(localized: "Couldn't search \(failure.computerName)"),
                        action: retrySearch
                    )
                }
            } header: {
                HStack(spacing: 6) {
                    Text("Other Matches")
                    if presentation.isPending {
                        ProgressView()
                            .controlSize(.small)
                            .accessibilityLabel(Text("Searching"))
                    }
                }
            }
        }
    }

    /// Records the query the session's result answers. The debounced search passes
    /// `immediately: false`; Return and Try Again search at once.
    private func search(immediately: Bool) async {
        let query = session.sidebarSearchQuery
        let answered: String?
        if immediately {
            answered = await session.retrySidebarSearch()
        } else {
            answered = await session.runSidebarSearch()
        }
        guard !Task.isCancelled, session.sidebarSearchQuery == query else { return }
        searchedQuery = answered
    }

    private func retrySearch() {
        searchedQuery = nil
        Task { await search(immediately: true) }
    }

    /// ⌘K. The first run only records the current request. A new request focuses
    /// the field at once and again after the sidebar has been revealed.
    private func focusSearch() async {
        let request = session.searchFocusRequest
        guard let handled = handledSearchFocusRequest else {
            handledSearchFocusRequest = request
            return
        }
        guard request != handled else { return }
        handledSearchFocusRequest = request
        isSearchPresented = true
        isSearchFieldFocused = true
        try? await Task.sleep(for: .milliseconds(300))
        guard !Task.isCancelled, !isSearchFieldFocused else { return }
        isSearchPresented = true
        isSearchFieldFocused = true
    }

    // MARK: - Try Again

    private func retryComputer(_ planeRegistryID: UUID) {
        guard !retryingComputers.contains(planeRegistryID) else { return }
        retryingComputers.insert(planeRegistryID)
        Task {
            await session.retrySidebarComputer(planeRegistryID)
            retryingComputers.remove(planeRegistryID)
        }
    }

    private func retryList() {
        guard !isRetryingList else { return }
        isRetryingList = true
        Task {
            await session.retrySidebarTaskList()
            isRetryingList = false
        }
    }

    // MARK: - Menus and activation

    private func activate(_ items: Set<SidebarItem>) {
        guard items.count == 1 else { return }
        session.activateSidebarItem(items.first)
    }

    /// Only the items that apply are shown; New Task and Jet Trash have no menu.
    @ViewBuilder
    private func menu(for items: Set<SidebarItem>) -> some View {
        if items.count == 1, let item = items.first {
            switch item {
            case let .task(conversationID):
                taskMenu(conversationID)
            case let .project(projectID):
                if let project = session.sidebarProjects.first(where: { $0.project.id == projectID }) {
                    projectMenu(project)
                }
            case .newTask, .trash:
                EmptyView()
            }
        }
    }

    @ViewBuilder
    private func taskMenu(_ conversationID: UUID) -> some View {
        let canChange = session.sidebarCanChangeTask(conversationID)
        let isOpen = session.selectedConversationID == conversationID
        let canReveal = isOpen && session.canRevealWorkingCopy
        let branch = isOpen ? session.sidebarKeptBranchName : nil
        if canChange {
            Button("Rename…", systemImage: "pencil") {
                session.renameFromSidebar(conversationID)
            }
            Button("Repeat Daily…", systemImage: "clock") {
                session.repeatDailyFromSidebar(conversationID)
            }
        }
        if canReveal || branch != nil {
            if canChange { Divider() }
            if canReveal {
                Button("Show Working Copy in Finder", systemImage: "folder") {
                    session.revealWorkingCopy()
                }
            }
            if let branch {
                Button("Copy Branch Name", systemImage: "document.on.document") {
                    DesktopSession.copyToPasteboard(branch)
                }
            }
        }
        if canChange {
            Divider()
            Button("Move to Jet Trash…", systemImage: "trash") {
                session.moveToTrashFromSidebar(conversationID)
            }
        }
    }

    @ViewBuilder
    private func projectMenu(_ project: JetPlaneProject) -> some View {
        Button("New Task in “\(project.project.name)”") {
            session.newTaskFromSidebar(in: project)
        }
        if session.sidebarCanRevealProject(project) {
            Button("Show in Finder") {
                session.sidebarRevealProject(project)
            }
        }
        Button("Copy Path") {
            DesktopSession.copyToPasteboard(project.project.root)
        }
        if !session.isComputerOffline(project.planeRegistryID) {
            Divider()
            Button("Move Project Folder to Trash…") {
                session.moveProjectFolderToTrashFromSidebar(project)
            }
        }
    }
}

#if DEBUG
#Preview("Sidebar") {
    NavigationSplitView {
        SidebarView(session: .preview { DesktopPreviewData.connect($0) })
    } detail: {
        Text("Detail")
    }
}
#endif
