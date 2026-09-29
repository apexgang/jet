import SwiftUI
import UniformTypeIdentifiers

// MARK: - Model

/// Add Project's flow: choose a folder, review what jetd says about it, then
/// register it with one Command ID that survives retries of an uncertain outcome.
@MainActor
@Observable
final class AddProjectModel {
    enum Phase: Equatable {
        case choosing(message: String?)
        case checking(path: String)
        case reviewed(JetProjectPreview)
        case alreadyAdded(JetProjectSummary)
        case adding(JetProjectPreview)
        case failed(JetProjectPreview, JetPresentationError)
        case unconfirmed(JetProjectPreview)
        case previewFailed(path: String, JetPresentationError)
        case added(JetProjectSummary)
    }

    typealias ExistingProjects = @MainActor (UUID) -> [JetProjectSummary]
    typealias PreviewFolder = @MainActor (String, UUID) async throws -> JetProjectPreview
    typealias RegisterProject = @MainActor (JetProjectPreview, UUID, UUID) async throws -> JetProjectSummary
    typealias RefreshProjects = @MainActor (UUID) async -> Void

    private(set) var planeRegistryID: UUID
    var remotePath = ""
    private(set) var phase: Phase = .choosing(message: nil)
    /// Kept for one reviewed root, including every retry of an uncertain result.
    private(set) var registrationCommandID = UUID()

    /// Drops results of checks and registrations that were superseded.
    @ObservationIgnored private var generation = 0
    /// The root `registrationCommandID` belongs to. Cleared after a definite
    /// rejection, whose answer jetd stores under that Command ID.
    @ObservationIgnored private var commandRoot: String?
    @ObservationIgnored private let existingProjects: ExistingProjects
    @ObservationIgnored private let previewFolder: PreviewFolder
    @ObservationIgnored private let registerProject: RegisterProject
    @ObservationIgnored private let refreshProjects: RefreshProjects

    init(
        planeRegistryID: UUID,
        existingProjects: @escaping ExistingProjects,
        previewFolder: @escaping PreviewFolder,
        registerProject: @escaping RegisterProject,
        refreshProjects: @escaping RefreshProjects
    ) {
        self.planeRegistryID = planeRegistryID
        self.existingProjects = existingProjects
        self.previewFolder = previewFolder
        self.registerProject = registerProject
        self.refreshProjects = refreshProjects
    }

    var isAdding: Bool {
        if case .adding = phase { return true }
        return false
    }

    /// Choosing another computer starts over.
    func choosePlane(_ planeRegistryID: UUID) {
        guard planeRegistryID != self.planeRegistryID else { return }
        self.planeRegistryID = planeRegistryID
        generation += 1
        phase = .choosing(message: nil)
    }

    /// Asks the computer about a folder. A registrable folder that is already a
    /// project goes straight to "already in Jet".
    func check(path: String) async {
        generation += 1
        let request = generation
        let plane = planeRegistryID
        phase = .checking(path: path)
        do {
            let preview = try await previewFolder(path, plane)
            guard request == generation else { return }
            review(preview)
        } catch {
            guard request == generation else { return }
            phase = .previewFailed(path: path, DesktopSession.presentationError(error))
        }
    }

    /// Checks a typed remote path; a relative path never reaches the computer.
    func checkRemotePath() async {
        let path = remotePath.trimmingCharacters(in: .whitespacesAndNewlines)
        guard path.hasPrefix("/") else {
            generation += 1
            phase = .choosing(message: String(localized: "Enter a full path that starts with /."))
            return
        }
        await check(path: path)
    }

    /// Use the repository that contains the chosen folder.
    func useSuggestedRoot() async {
        guard case let .reviewed(preview) = phase, let root = preview.suggestedRoot else { return }
        await check(path: root)
    }

    /// Back to choosing, keeping a typed remote path.
    func chooseAnother(message: String? = nil) {
        generation += 1
        phase = .choosing(message: message)
    }

    /// Registers the reviewed folder. The same Command ID is sent until the
    /// outcome is known, so an uncertain registration is never duplicated.
    func add() async {
        let preview: JetProjectPreview
        switch phase {
        case let .reviewed(reviewed) where reviewed.canRegister:
            preview = reviewed
        case let .failed(failed, error) where error.retryable:
            preview = failed
        default:
            return
        }
        generation += 1
        let request = generation
        let plane = planeRegistryID
        phase = .adding(preview)
        do {
            let project = try await registerProject(preview, registrationCommandID, plane)
            guard request == generation else { return }
            phase = .added(project)
        } catch {
            guard request == generation else { return }
            if case JetClientFailure.commandOutcomeUnknown = error {
                phase = .unconfirmed(preview)
                return
            }
            let failure = DesktopSession.presentationError(error)
            if failure.code == "project.already_registered" {
                await refreshProjects(plane)
                guard request == generation else { return }
                if let existing = existingProject(root: preview.root) {
                    phase = .alreadyAdded(existing)
                    return
                }
            }
            if !failure.retryable { commandRoot = nil }
            phase = .failed(preview, failure)
        }
    }

    /// Try Again: a transient failure resends the same Command; a definite one,
    /// a failed check or a broken repository checks the folder again.
    func retry() async {
        switch phase {
        case let .failed(_, error) where error.retryable:
            await add()
        case let .failed(preview, _):
            await check(path: preview.root)
        case let .previewFailed(path, _):
            await check(path: path)
        case let .reviewed(preview) where !preview.canRegister:
            await check(path: preview.root)
        default:
            return
        }
    }

    /// Check Again after an uncertain registration: the project counts as added
    /// only when the computer lists it. Otherwise the review returns with the same
    /// Command ID, so adding again can't register the folder twice.
    func checkAgain() async {
        guard case let .unconfirmed(preview) = phase else { return }
        generation += 1
        let request = generation
        let plane = planeRegistryID
        phase = .checking(path: preview.root)
        await refreshProjects(plane)
        guard request == generation else { return }
        if let existing = existingProject(root: preview.root) {
            phase = .added(existing)
        } else {
            phase = .reviewed(preview)
        }
    }

    private func review(_ preview: JetProjectPreview) {
        if preview.canRegister, let existing = existingProject(root: preview.root) {
            phase = .alreadyAdded(existing)
            return
        }
        let root = Self.standardized(preview.root)
        if preview.canRegister, commandRoot != root {
            commandRoot = root
            registrationCommandID = UUID()
        }
        phase = .reviewed(preview)
    }

    private func existingProject(root: String) -> JetProjectSummary? {
        let wanted = Self.standardized(root)
        return existingProjects(planeRegistryID).first { Self.standardized($0.root) == wanted }
    }

    static func standardized(_ path: String) -> String {
        let standardized = URL(fileURLWithPath: path).standardizedFileURL.path
        return standardized.isEmpty ? path : standardized
    }

#if DEBUG
    /// Shows a phase directly, for previews and screenshots.
    func seedForPreview(_ phase: Phase, remotePath: String = "") {
        self.phase = phase
        self.remotePath = remotePath
    }
#endif
}

extension AddProjectModel {
    /// A model that talks to the session's computers.
    convenience init(session: DesktopSession, planeRegistryID: UUID) {
        self.init(
            planeRegistryID: planeRegistryID,
            existingProjects: { [weak session] plane in session?.projects(on: plane) ?? [] },
            previewFolder: { [weak session] path, plane in
                guard let session else { throw CancellationError() }
                return try await session.previewProjectFolder(path: path, on: plane)
            },
            registerProject: { [weak session] preview, commandID, plane in
                guard let session else { throw CancellationError() }
                return try await session.registerProjectFolder(preview, on: plane, commandID: commandID)
            },
            refreshProjects: { [weak session] plane in
                await session?.refreshProjects(on: plane)
            }
        )
    }
}

// MARK: - Copy

/// Add Project's copy for each verdict and failure (design §6.7).
struct AddProjectCopy: Equatable {
    enum Primary: Equatable {
        case add
        case useRepository(root: String)
        case chooseAnotherFolder
        /// Try Again: checks the same folder once more.
        case checkAgain
    }

    var title: String
    var message: String
    var showsPath: Bool
    var primary: Primary

    var primaryTitle: String {
        switch primary {
        case .add: String(localized: "Add Project")
        case let .useRepository(root): String(localized: "Use \(Self.folderName(root))")
        case .chooseAnotherFolder: String(localized: "Choose Another Folder…")
        case .checkAgain: String(localized: "Try Again")
        }
    }

    /// The folder's own name, such as "web-app".
    static func folderName(_ path: String) -> String {
        let name = URL(fileURLWithPath: path).lastPathComponent
        return name.isEmpty ? path : name
    }

    static func review(_ preview: JetProjectPreview, computerName: String, isLocal: Bool) -> AddProjectCopy {
        let folder = folderName(preview.root)
        switch preview.registrability {
        case .registrable:
            return AddProjectCopy(
                title: isLocal
                    ? String(localized: "Add “\(folder)”?")
                    : String(localized: "Add “\(folder)” on \(computerName)?"),
                message: String(localized: "Tasks in this project work in a separate copy. Your folder doesn't change until you keep the changes."),
                showsPath: true,
                primary: .add
            )
        case let .unavailable(verdict, _):
            switch verdict {
            case "inside_working_tree":
                let root = preview.suggestedRoot ?? preview.root
                return AddProjectCopy(
                    title: String(localized: "This folder is inside “\(folderName(root))”."),
                    message: String(localized: "Jet adds the whole repository."),
                    showsPath: false,
                    primary: .useRepository(root: root)
                )
            case "not_a_repository":
                return AddProjectCopy(
                    title: String(localized: "“\(folder)” isn't a Git repository."),
                    message: String(localized: "Jet needs Git so your changes can be kept safely."),
                    showsPath: false,
                    primary: .chooseAnotherFolder
                )
            case "bare_repository":
                return AddProjectCopy(
                    title: String(localized: "“\(folder)” is a bare repository."),
                    message: String(localized: "Choose a folder that has the project's files checked out."),
                    showsPath: false,
                    primary: .chooseAnotherFolder
                )
            case "inside_git_dir":
                return AddProjectCopy(
                    title: String(localized: "This folder is inside Git's own data."),
                    message: String(localized: "Choose the repository folder instead."),
                    showsPath: false,
                    primary: .chooseAnotherFolder
                )
            case "broken_repository":
                return AddProjectCopy(
                    title: String(localized: "Git can't open “\(folder)”."),
                    message: String(localized: "Check the repository, then try again."),
                    showsPath: false,
                    primary: .checkAgain
                )
            default:
                return AddProjectCopy(
                    title: String(localized: "Jet can't add “\(folder)”."),
                    message: String(localized: "Choose the top folder of a Git repository."),
                    showsPath: false,
                    primary: .chooseAnotherFolder
                )
            }
        }
    }

    /// Registration refusals that mean the folder is no longer what Jet checked.
    static let changedFolderCodes: Set<String> = [
        "project.not_a_repository",
        "project.repository_broken",
        "project.bare_repository",
        "project.inside_git_dir",
        "project.root_not_toplevel",
    ]

    static func failure(_ error: JetPresentationError, computerName: String) -> String {
        if error.category == .offline { return String(localized: "Can't reach \(computerName).") }
        if error.code == "project.path_invalid" { return String(localized: "Enter a full path that starts with /.") }
        if changedFolderCodes.contains(error.code) {
            return String(localized: "The folder changed since Jet checked it. Choose it again.")
        }
        return String(localized: "Jet couldn't add this project.")
    }

    /// Only the general failure shows its code under Details.
    static func showsDetails(_ error: JetPresentationError) -> Bool {
        error.category != .offline
            && error.code != "project.path_invalid"
            && !changedFolderCodes.contains(error.code)
    }
}

// MARK: - Sheet

/// Add Project (design §6.7): opened by Add Project…, ⇧⌘O or a folder drop.
struct AddProjectSheet: View {
    @Bindable var session: DesktopSession
    @State private var model: AddProjectModel
    private let droppedURL: URL?

    @State private var importerPresented = false
    @State private var importerWasAutoPresented = false
    @State private var didStart = false
    @State private var isDropTargeted = false

    init(session: DesktopSession, planeRegistryID: UUID, droppedURL: URL?) {
        self.session = session
        self.droppedURL = droppedURL
        // A dropped folder is always on this Mac.
        let plane = droppedURL == nil ? planeRegistryID : session.localPlaneRegistryID
        _model = State(initialValue: AddProjectModel(session: session, planeRegistryID: plane))
    }

    /// A sheet around a prepared model, for previews.
    init(session: DesktopSession, model: AddProjectModel) {
        self.session = session
        droppedURL = nil
        _model = State(initialValue: model)
    }

    private var isLocal: Bool { session.isLocalPlane(model.planeRegistryID) }
    private var computerName: String { session.planeName(model.planeRegistryID) }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            content
            buttons
        }
        .padding(20)
        .frame(width: 460, alignment: .leading)
        .overlay {
            if isDropTargeted {
                RoundedRectangle(cornerRadius: JetDesign.fieldRadius)
                    .strokeBorder(JetDesign.accent, style: StrokeStyle(lineWidth: 2, dash: [6, 4]))
                    .padding(6)
                    .allowsHitTesting(false)
            }
        }
        .accessibilityIdentifier("add-project-sheet")
        .interactiveDismissDisabled(model.isAdding)
        .fileImporter(
            isPresented: $importerPresented,
            allowedContentTypes: [.folder],
            allowsMultipleSelection: false
        ) { result in
            importerWasAutoPresented = false
            switch result {
            case let .success(urls):
                guard let url = urls.first else { return }
                Task { await model.check(path: url.path) }
            case .failure:
                model.chooseAnother(message: String(localized: "Couldn't open that folder. Choose it again."))
            }
        } onCancellation: {
            // Cancelling the panel that opened with the sheet closes the sheet too.
            if importerWasAutoPresented, case .choosing = model.phase { session.dismissSheet() }
            importerWasAutoPresented = false
        }
#if os(macOS)
        .fileDialogConfirmationLabel("Choose")
#endif
        .modifier(AddProjectFolderDrop(
            isEnabled: acceptsDrop,
            isTargeted: $isDropTargeted
        ) { url in
            Task { await model.check(path: url.path) }
        })
        .onChange(of: model.phase) { _, phase in
            if case let .added(project) = phase {
                finish(with: project, notice: String(localized: "Added “\(project.name)”."))
            }
        }
        .task {
            guard !didStart else { return }
            didStart = true
            if let droppedURL {
                await model.check(path: droppedURL.path)
            } else if isLocal, !session.isPreviewSession, case .choosing = model.phase {
                importerWasAutoPresented = true
                importerPresented = true
            }
        }
    }

    // MARK: Content

    @ViewBuilder private var content: some View {
        switch model.phase {
        case let .choosing(message):
            VStack(alignment: .leading, spacing: 8) {
                SheetTitle(text: String(localized: "Add Project"))
                if session.planes.count >= 2 {
                    Picker("Computer", selection: planeSelection) {
                        ForEach(session.planes) { plane in
                            Text(plane.name).tag(plane.id)
                        }
                    }
                    .fixedSize()
                    .padding(.vertical, 4)
                }
                if isLocal {
                    Text("Choose the folder of a Git repository on this Mac.")
                        .font(.system(size: JetDesign.TextSize.navigation))
                    Text("You can also drop a folder here.")
                        .font(.system(size: JetDesign.TextSize.navigation))
                        .foregroundStyle(.secondary)
                } else {
                    Text("Enter the path of a Git repository on \(computerName).")
                        .font(.system(size: JetDesign.TextSize.navigation))
                        .fixedSize(horizontal: false, vertical: true)
                    TextField(
                        String(localized: "Folder path on \(computerName)"),
                        text: $model.remotePath,
                        prompt: Text(verbatim: "/path/to/project")
                    )
                    .textFieldStyle(.roundedBorder)
                    .autocorrectionDisabled()
                    .accessibilityIdentifier("add-project-remote-path")
                    .onSubmit { Task { await model.checkRemotePath() } }
                    .padding(.top, 4)
                }
            }
            if let message {
                InlineNotice(notice: ComposerNotice(kind: .warning, text: message)) { _ in }
            }
        case let .checking(path):
            VStack(alignment: .leading, spacing: 8) {
                SheetTitle(text: String(localized: "Add Project"))
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text("Checking “\(AddProjectCopy.folderName(path))”…")
                        .font(.system(size: JetDesign.TextSize.navigation))
                        .foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .combine)
            }
        case let .reviewed(preview), let .adding(preview):
            ProjectPreviewView(preview: preview, computerName: computerName, isLocal: isLocal)
        case let .failed(preview, error):
            ProjectPreviewView(preview: preview, computerName: computerName, isLocal: isLocal)
            failureNotice(error)
        case let .unconfirmed(preview):
            ProjectPreviewView(preview: preview, computerName: computerName, isLocal: isLocal)
            InlineNotice(
                notice: ComposerNotice(kind: .warning, text: String(localized: "Jet couldn't confirm the project was added."))
            ) { _ in }
        case let .previewFailed(_, error):
            SheetTitle(text: String(localized: "Add Project"))
            failureNotice(error)
        case let .alreadyAdded(project):
            VStack(alignment: .leading, spacing: 8) {
                SheetTitle(text: String(localized: "“\(project.name)” is already in Jet."))
                Text("Start a task in it from New Task.")
                    .font(.system(size: JetDesign.TextSize.navigation))
            }
        case .added:
            SheetTitle(text: String(localized: "Add Project"))
        }
    }

    @ViewBuilder private func failureNotice(_ error: JetPresentationError) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            InlineNotice(
                notice: ComposerNotice(kind: .error, text: AddProjectCopy.failure(error, computerName: computerName))
            ) { _ in }
            if AddProjectCopy.showsDetails(error) {
                ErrorCodeDetails(code: error.code)
            }
        }
    }

    private var planeSelection: Binding<UUID> {
        Binding { model.planeRegistryID } set: { model.choosePlane($0) }
    }

    // MARK: Buttons

    private var buttons: some View {
        HStack(spacing: 8) {
            secondaryButton
            Spacer(minLength: 8)
            Button("Cancel") { session.dismissSheet() }
                .keyboardShortcut(.cancelAction)
                .disabled(model.isAdding)
            primaryButton
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
        }
    }

    private var chooseAnotherFolderButton: some View {
        Button("Choose Another Folder…") { chooseAnotherFolder() }
    }

    @ViewBuilder private var secondaryButton: some View {
        if isLocal {
            switch model.phase {
            case let .reviewed(preview):
                if AddProjectCopy.review(preview, computerName: computerName, isLocal: true).primary != .chooseAnotherFolder {
                    chooseAnotherFolderButton
                }
            case .failed, .previewFailed:
                chooseAnotherFolderButton
            default:
                EmptyView()
            }
        }
    }

    @ViewBuilder private var primaryButton: some View {
        switch model.phase {
        case .choosing:
            if isLocal {
                Button("Choose Folder…") { importerPresented = true }
                    .accessibilityIdentifier("add-project-choose-folder")
            } else {
                Button("Check Folder") { Task { await model.checkRemotePath() } }
                    .disabled(model.remotePath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    .accessibilityIdentifier("add-project-primary")
            }
        case .checking, .added:
            Button("Add Project") {}
                .disabled(true)
                .accessibilityIdentifier("add-project-primary")
        case let .reviewed(preview):
            let copy = AddProjectCopy.review(preview, computerName: computerName, isLocal: isLocal)
            Button(copy.primaryTitle) { performPrimary(copy.primary) }
                .accessibilityIdentifier("add-project-primary")
        case .adding:
            Button("Adding…") {}
                .disabled(true)
                .accessibilityIdentifier("add-project-primary")
        case .failed, .previewFailed:
            Button("Try Again") { Task { await model.retry() } }
                .accessibilityIdentifier("add-project-primary")
        case .unconfirmed:
            Button("Check Again") { Task { await model.checkAgain() } }
                .accessibilityIdentifier("add-project-primary")
        case let .alreadyAdded(project):
            Button("Use \(project.name)") { finish(with: project, notice: nil) }
                .accessibilityIdentifier("add-project-primary")
        }
    }

    private func performPrimary(_ primary: AddProjectCopy.Primary) {
        switch primary {
        case .add:
            Task { await model.add() }
        case .useRepository:
            Task { await model.useSuggestedRoot() }
        case .chooseAnotherFolder:
            chooseAnotherFolder()
        case .checkAgain:
            Task { await model.retry() }
        }
    }

    private func chooseAnotherFolder() {
        if isLocal {
            importerPresented = true
        } else {
            model.chooseAnother()
        }
    }

    private var acceptsDrop: Bool {
        guard isLocal else { return false }
        switch model.phase {
        case .choosing, .reviewed: return true
        default: return false
        }
    }

    /// Closes the sheet and opens New Task in the project.
    private func finish(with project: JetProjectSummary, notice: String?) {
        let planeRegistryID = model.planeRegistryID
        session.dismissSheet()
        session.startNewTask(in: project.id, on: planeRegistryID, notice: notice)
    }
}

private struct SheetTitle: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityAddTraits(.isHeader)
    }
}

/// Accepts a dropped folder on this Mac. Files and other items are ignored.
struct AddProjectFolderDrop: ViewModifier {
    let isEnabled: Bool
    @Binding var isTargeted: Bool
    let onFolder: (URL) -> Void

    func body(content: Content) -> some View {
#if os(macOS)
        content
            .dropDestination(for: URL.self, isEnabled: isEnabled) { urls, _ in
                isTargeted = false
                guard let url = urls.first(where: Self.isDirectory) else { return }
                onFolder(url)
            }
            .onDropSessionUpdated { drop in
                let isOver = drop.phase == .entering || drop.phase == .active
                isTargeted = isOver && isEnabled
            }
#else
        content
#endif
    }

    static func isDirectory(_ url: URL) -> Bool {
        url.isFileURL && (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true
    }
}

#if DEBUG
#Preview("Add Project") {
    DesktopPreviewScenes.view("add-project-confirm")
        .frame(width: 480, height: 340)
}
#endif
