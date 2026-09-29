import SwiftUI

/// A project's page (design §6.11): its tasks, what happens when a reply
/// finishes, branch names and where the folder is.
struct ProjectPageView: View {
    @Bindable var session: DesktopSession
    /// A seeded settings model, for previews only.
    var model: ReplyFinishModel? = nil

    var body: some View {
        content
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .onAppear { session.clearStaleLibraryNotice() }
            // remove in wave 3: Add Project… still sets the old importer flag; the
            // Add Project sheet replaces the importer this page used to host.
            .onChange(of: session.isProjectImporterPresented, initial: true) { _, isPresented in
                guard isPresented else { return }
                session.isProjectImporterPresented = false
                session.presentAddProject(droppedURL: nil)
            }
#if !os(macOS)
            .navigationTitle(session.selectedProject?.name ?? String(localized: "Project"))
#endif
    }

    @ViewBuilder
    private var content: some View {
        if let project = session.selectedProject {
            ProjectPageForm(
                session: session,
                project: project,
                planeRegistryID: session.selectedPlaneRegistryID,
                model: model
            )
            .id(ProjectKey(planeRegistryID: session.selectedPlaneRegistryID, projectID: project.id))
        } else if isLoadingSetup {
            ProjectPagePlaceholder()
        } else if session.canLoadLibrary,
                  session.selectedSetupSnapshot?.projects.projects.isEmpty == true,
                  session.allProjects.isEmpty
        {
            ContentUnavailableView {
                Label("No Projects", systemImage: "folder")
            } description: {
                Text("Add a Git repository to start working on it.")
            } actions: {
                Button("Add Project…") { session.presentAddProject(droppedURL: nil) }
            }
        } else {
            ContentUnavailableView(
                "No Project Selected",
                systemImage: "folder",
                description: Text("Choose a project in the sidebar.")
            )
        }
    }

    /// The computer's projects haven't arrived yet and it isn't offline.
    private var isLoadingSetup: Bool {
        session.canLoadLibrary
            && session.selectedSetupSnapshot == nil
            && !session.isComputerOffline(session.selectedPlaneRegistryID)
    }

    private struct ProjectKey: Hashable {
        let planeRegistryID: UUID
        let projectID: UUID
    }
}

private struct ProjectPageForm: View {
    let session: DesktopSession
    let project: JetProjectSummary
    let planeRegistryID: UUID
    @State private var model: ReplyFinishModel
    @State private var showsAllTasks = false
    @FocusState private var prefixFocused: Bool

    static let visibleTaskLimit = 8

    init(session: DesktopSession, project: JetProjectSummary, planeRegistryID: UUID, model: ReplyFinishModel?) {
        self.session = session
        self.project = project
        self.planeRegistryID = planeRegistryID
        _model = State(initialValue: model ?? ReplyFinishModel(
            planeRegistryID: planeRegistryID,
            scope: .project(project.id),
            makeAccess: session.libraryAccessProvider,
            computer: session.planeDisplayName(planeRegistryID),
            isLocal: session.isLocalPlane(planeRegistryID)
        ))
    }

    /// Settings can't change while the computer is offline; WP5's banner offers Try Again.
    private var isOffline: Bool {
        session.isComputerOffline(planeRegistryID) || model.isOffline
    }

    var body: some View {
        Form {
            if let notice = session.libraryPageNotice {
                Section {
                    InlineNotice(notice: notice, perform: session.perform)
                }
            }
            tasksSection
            replyFinishSection
            branchesSection
            locationSection
        }
        .formStyle(.grouped)
        .readingWidthForm()
        .accessibilityIdentifier("project-page")
        .replyFinishPushConfirmation(model)
        .task { await model.load() }
        .onChange(of: prefixFocused) { _, isFocused in
            if !isFocused { commitBranchPrefix() }
        }
    }

    // MARK: Tasks

    private var tasks: [JetConversationSummary] {
        session.tasks(inProject: project.id, on: planeRegistryID)
    }

    @ViewBuilder
    private var tasksSection: some View {
        Section("Tasks in This Project") {
            Button("New Task in “\(project.name)”", systemImage: "square.and.pencil") {
                session.startNewTask(inProject: project.id, on: planeRegistryID)
            }
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("project-new-task")

            let tasks = tasks
            if tasks.isEmpty {
                Text("No tasks in \(project.name) yet.")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(showsAllTasks ? tasks : Array(tasks.prefix(Self.visibleTaskLimit))) { task in
                    ProjectTaskRow(session: session, task: task)
                }
                if !showsAllTasks, tasks.count > Self.visibleTaskLimit {
                    Button("Show All \(tasks.count) Tasks") { showsAllTasks = true }
                        .buttonStyle(.borderless)
                        .foregroundStyle(JetDesign.accentText)
                }
            }
        }
    }

    // MARK: When a Reply Finishes

    @ViewBuilder
    private var replyFinishSection: some View {
        Section {
            if isOffline {
                Label {
                    Text("Can't reach \(session.planeDisplayName(planeRegistryID)). Project settings can't be changed right now.")
                } icon: {
                    Image(systemName: "wifi.slash")
                }
                .foregroundStyle(.secondary)
            } else if model.issueRow == nil, let issue = model.issue {
                LibraryNoticeRow(issue: issue, perform: perform)
            }
            ReplyFinishSection(
                model: model,
                projectName: project.name,
                isDisabled: isOffline,
                perform: perform
            )
        } header: {
            Text("When a Reply Finishes")
        } footer: {
            Text("Replies that stop early or with an error aren't saved automatically.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    // MARK: Branches and Names

    @ViewBuilder
    private var branchesSection: some View {
        Section("Branches and Names") {
            if model.phase == .loading, model.scopeSnapshot == nil {
                LabeledContent("Branch names start with") { Text("Loading…").foregroundStyle(.secondary) }
                LabeledContent("Name tasks automatically") { Text("Loading…").foregroundStyle(.secondary) }
            } else if model.scopeSnapshot != nil {
                branchPrefixRow
                automaticNamingRow
            }
        }
    }

    @ViewBuilder
    private var branchPrefixRow: some View {
        @Bindable var model = model
        TextField("Branch names start with", text: $model.branchPrefixDraft)
            .focused($prefixFocused)
            .onSubmit(commitBranchPrefix)
            .disabled(isOffline || model.isBusy)
            .accessibilityIdentifier("project-branch-prefix")
        if let problem = model.branchPrefixError {
            Label(problem, systemImage: "xmark.octagon.fill")
                .foregroundStyle(.red)
                .font(.callout)
        }
        if model.savingRow == .branchPrefix {
            SavingLabel().font(.callout).foregroundStyle(.secondary)
        }
        if model.issueRow == .branchPrefix, let issue = model.issue {
            LibraryNoticeRow(issue: issue, perform: perform)
        }
        if model.branchPrefixIsExplicit {
            Button("Reset to Default (\(model.defaultBranchPrefix))") {
                Task { await model.resetBranchPrefix() }
            }
            .buttonStyle(.borderless)
            .foregroundStyle(JetDesign.accentText)
            .disabled(isOffline || model.isBusy)
        }
    }

    @ViewBuilder
    private var automaticNamingRow: some View {
        Toggle(isOn: Binding(
            get: { model.automaticNaming },
            set: { isOn in Task { await model.setAutomaticNaming(isOn) } }
        )) {
            Text("Name tasks automatically")
            Text("Jet suggests a short title for each new task.")
        }
        .disabled(isOffline || model.isBusy)
        .accessibilityIdentifier("project-auto-naming")
        if model.savingRow == .automaticNaming {
            SavingLabel().font(.callout).foregroundStyle(.secondary)
        }
        if model.issueRow == .automaticNaming, let issue = model.issue {
            LibraryNoticeRow(issue: issue, perform: perform)
        }
        if model.automaticNamingIsExplicit {
            Button(model.defaultAutomaticNaming ? "Reset to Default (On)" : "Reset to Default (Off)") {
                Task { await model.resetAutomaticNaming() }
            }
            .buttonStyle(.borderless)
            .foregroundStyle(JetDesign.accentText)
            .disabled(isOffline || model.isBusy)
        }
    }

    // MARK: Location

    private var locationSection: some View {
        Section("Location") {
            Text(project.root)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
                .help(project.root)
            HStack(spacing: JetDesign.smallGap) {
                if session.canRevealInFinder(on: planeRegistryID) {
                    Button("Show in Finder", systemImage: "folder") {
                        session.revealInFinder(path: project.root)
                    }
                }
                Button("Copy Path", systemImage: "document.on.document") {
                    DesktopSession.copyToPasteboard(project.root)
                }
            }
            .buttonStyle(.bordered)
        }
    }

    // MARK: Actions

    private func commitBranchPrefix() {
        let draft = model.branchPrefixDraft
        Task { await model.setBranchPrefix(draft) }
    }

    private func perform(_ action: LibraryIssue.Action) {
        switch action {
        case .tryAgain: Task { await model.tryAgain() }
        case .sendSameRequestAgain: Task { await model.sendSameRequestAgain() }
        case let .review(pane): session.perform(.openSettings(pane))
        default: break
        }
    }
}

/// One task of the project: its status glyph, title and when it started, or what
/// it needs from you. VoiceOver reads it as one element.
private struct ProjectTaskRow: View {
    let session: DesktopSession
    let task: JetConversationSummary

    var body: some View {
        let status = session.taskStatus(for: task.id)
        let isUnread = session.isUnread(task.id)
        Button {
            session.sidebarItem = .task(task.id)
        } label: {
            HStack(spacing: JetDesign.smallGap) {
                TaskStatusGlyph(status: status, isUnread: isUnread)
                Text(title)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    // Offline rows are dimmed by the row, not marked with a glyph.
                    .foregroundStyle(status == .offline ? .secondary : .primary)
                Spacer(minLength: JetDesign.smallGap)
                Text(status.needsYou
                     ? status.title
                     : JetCopy.relative(ms: task.createdAtUnixMilliseconds, now: session.taskListNow))
                    .font(.callout)
                    .foregroundStyle(status.needsYou ? .primary : .secondary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(accessibilityText(status: status, isUnread: isUnread)))
        .accessibilityAddTraits(.isButton)
        .accessibilityIdentifier("project-task-\(task.id.uuidString.lowercased())")
    }

    private var title: String {
        let trimmed = task.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? LibraryCopy.untitledTask : trimmed
    }

    /// "Fix login redirect, Needs permission, new reply".
    private func accessibilityText(status: TaskStatus, isUnread: Bool) -> String {
        var parts = [title]
        if !status.accessibilityDescription.isEmpty { parts.append(status.accessibilityDescription) }
        if isUnread { parts.append(String(localized: "new reply")) }
        return parts.joined(separator: ", ")
    }
}

/// The page's shape while the computer's projects load.
private struct ProjectPagePlaceholder: View {
    var body: some View {
        Form {
            Section("Tasks in This Project") {
                ForEach(0 ..< 3, id: \.self) { _ in
                    Text("Placeholder task title for loading")
                }
            }
            Section("When a Reply Finishes") {
                Text("Save to a Branch")
                Text("Placeholder caption text for loading the setting")
            }
        }
        .formStyle(.grouped)
        .redacted(reason: .placeholder)
        .readingWidthForm()
        .accessibilityLabel(Text("Loading…"))
    }
}

/// Keeps a full-width form's content at the reading width, centred, so the
/// scroll edge and toolbar background still span the whole column.
private struct ReadingWidthForm: ViewModifier {
    @State private var width: CGFloat = 0

    func body(content: Content) -> some View {
        content
            .contentMargins(.horizontal, max(0, (width - JetDesign.readingWidth) / 2), for: .scrollContent)
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
    }
}

private extension View {
    func readingWidthForm() -> some View { modifier(ReadingWidthForm()) }
}

#if DEBUG
#Preview("Project page") {
    DesktopPreviewScenes.view("library-project-page")
        .frame(width: 1280, height: 800)
}
#endif
