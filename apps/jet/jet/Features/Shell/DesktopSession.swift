import Foundation
import Observation

typealias JetClientFactory = @Sendable () async throws -> JetClient

enum SidebarDestination: String, CaseIterable, Hashable, Sendable {
    case newTask
    case search
    case needsAttention
    case project
    case conversation
    case schedules
    case planes
}

enum WorkPanelTab: String, CaseIterable, Hashable, Sendable {
    case changes
    case files
    case terminal
    case run

    var title: String {
        switch self {
        case .changes: "Changes"
        case .files: "Files"
        case .terminal: "Terminal"
        case .run: "Run"
        }
    }
}

@MainActor
@Observable
final class DesktopSession {
    enum ContentState {
        case loading
        case ready(DesktopFixtureScenario)
        case failed(String)
    }

    private enum FixtureLoadResult: Sendable {
        case success(DesktopFixtureCorpus)
        case failure(String)
    }

    enum SetupState {
        case idle
        case loading
        case ready(JetSetupSnapshot)
        case failed(JetPresentationError)
    }

    var contentState: ContentState = .loading
    var sidebarSelection: SidebarDestination = .conversation
    var selectedWorkPanel: WorkPanelTab = .run
    var isWorkPanelPresented = true
    var draft = ""
    var composerFocusRequest = 0
    var actionNotice: String?
    var setupState: SetupState = .idle
    var connectionState: JetConnectionState = .disconnected
    var selectedProjectID: UUID?
    var isProjectImporterPresented = false
    var projectPreview: JetProjectPreview?
    var removalPreview: JetProjectRemovalPreview?
    var permanentRemovalAllowed = false
    var setupNotice: String?
    var setupOperation: String?
    var remotePairingSkipped = false

    private var scenarios: [DesktopFixtureState: DesktopFixtureScenario] = [:]
    private var didLoadFixtures = false
    private let makeJetClient: JetClientFactory?
    private var client: JetClient?
    private var connectionObservationTask: Task<Void, Never>?
    private var registrationCommandID = UUID()
    private var removalTrashCommandID = UUID()
    private var removalPermanentCommandID = UUID()
    private var accountCommandIDs: [String: UUID] = [:]

    init(makeJetClient: JetClientFactory? = nil) {
        self.makeJetClient = makeJetClient
    }

    var scenario: DesktopFixtureScenario? {
        guard case let .ready(scenario) = contentState else { return nil }
        return scenario
    }

    var canSubmitDraft: Bool {
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var setupSnapshot: JetSetupSnapshot? {
        guard case let .ready(snapshot) = setupState else { return nil }
        return snapshot
    }

    var selectedProject: JetProjectSummary? {
        setupSnapshot?.projects.projects.first { $0.id == selectedProjectID }
    }

    var selectedProjectName: String {
        selectedProject?.name ?? scenario?.project?.name ?? "Choose a Project"
    }

    var selectedHarnessName: String {
        setupSnapshot?.accounts.bindings.first?.label
            ?? setupSnapshot?.capabilities.authProviders.first?.harness
            ?? scenario?.capabilities.harnesses.first?.capitalized
            ?? "Choose"
    }

    var planeConnectionLabel: String {
        switch connectionState {
        case .disconnected: setupStateIsIdle ? "Not checked" : "Offline"
        case .connecting: "Connecting"
        case .connected: "Connected"
        case .reconnecting: "Reconnecting"
        case .failed: "Unavailable"
        }
    }

    var planeIsConnected: Bool {
        if case .connected = connectionState { return true }
        return false
    }

    private var setupStateIsIdle: Bool {
        if case .idle = setupState { return true }
        return false
    }

    func loadFoundationFixture() async {
        guard !didLoadFixtures else { return }
        didLoadFixtures = true

        guard let fixtureURL = Bundle.main.url(
            forResource: "presentation-v1",
            withExtension: "json"
        ) else {
            contentState = .failed("The shared desktop fixture is missing from this build.")
            return
        }

        let result = await Task.detached(priority: .userInitiated) {
            do {
                let data = try Data(contentsOf: fixtureURL, options: .mappedIfSafe)
                return FixtureLoadResult.success(try DesktopFixtureCorpus(data: data))
            } catch {
                return FixtureLoadResult.failure("Jet could not load the shared desktop fixture.")
            }
        }.value

        switch result {
        case let .success(corpus):
            scenarios = Dictionary(
                uniqueKeysWithValues: corpus.scenarios.map { ($0.state, $0) }
            )
            showScenario(.active, fallback: .ready)
        case let .failure(message):
            contentState = .failed(message)
        }
    }

    func loadSetup(openWhenIncomplete: Bool = false) async {
        guard makeJetClient != nil else { return }
        if setupSnapshot == nil { setupState = .loading }
        do {
            let snapshot = try await activeClient().setupSnapshot()
            setupState = .ready(snapshot)
            if snapshot.issue(for: .projects) == nil,
               !snapshot.projects.projects.contains(where: { $0.id == selectedProjectID })
            {
                selectedProjectID = snapshot.projects.projects.first?.id
            }
            let needsProject = snapshot.issue(for: .projects) == nil
                && snapshot.projects.projects.isEmpty
            let needsAccount = snapshot.issue(for: .accounts) == nil
                && snapshot.accounts.bindings.isEmpty
            if openWhenIncomplete, needsProject || needsAccount
            {
                sidebarSelection = .project
                isWorkPanelPresented = false
            }
        } catch {
            let failure = presentationError(error)
            setupState = .failed(failure)
            connectionState = failure.retryable
                ? .reconnecting(attempt: 1)
                : .failed(failure)
        }
    }

    func requestAddProject() {
        sidebarSelection = .project
        isWorkPanelPresented = false
        isProjectImporterPresented = true
        setupNotice = nil
    }

    func showProjects() {
        sidebarSelection = .project
        isWorkPanelPresented = false
        setupNotice = nil
    }

    func selectProject(_ projectID: UUID) {
        guard setupSnapshot?.projects.projects.contains(where: { $0.id == projectID }) == true else {
            return
        }
        selectedProjectID = projectID
        sidebarSelection = .project
        isWorkPanelPresented = false
        setupNotice = nil
    }

    func previewProject(at url: URL) async {
        guard setupOperation == nil else { return }
        setupOperation = "preview-project"
        setupNotice = nil
        projectPreview = nil
        do {
            projectPreview = try await activeClient().previewProject(path: url.path)
            registrationCommandID = UUID()
        } catch {
            setupNotice = presentationError(error).message
        }
        setupOperation = nil
    }

    func registerPreviewedProject() async {
        guard let projectPreview, projectPreview.canRegister, setupOperation == nil else { return }
        setupOperation = "register-project"
        setupNotice = nil
        do {
            let project = try await activeClient().registerProject(
                preview: projectPreview,
                commandID: registrationCommandID
            )
            self.projectPreview = nil
            setupNotice = "\(project.name) is ready."
            await loadSetup()
            selectedProjectID = project.id
        } catch {
            setupNotice = presentationError(error).message
        }
        setupOperation = nil
    }

    func prepareProjectRemoval(_ projectID: UUID) async {
        guard setupOperation == nil else { return }
        setupOperation = "preview-removal"
        setupNotice = nil
        permanentRemovalAllowed = false
        do {
            removalPreview = try await activeClient().previewProjectRemoval(projectID: projectID)
            removalTrashCommandID = UUID()
            removalPermanentCommandID = UUID()
        } catch {
            setupNotice = presentationError(error).message
        }
        setupOperation = nil
    }

    func cancelProjectRemoval() {
        removalPreview = nil
        permanentRemovalAllowed = false
    }

    func confirmProjectRemoval(typedName: String, permanently: Bool) async {
        guard let removalPreview, setupOperation == nil else { return }
        setupOperation = "remove-project"
        setupNotice = nil
        do {
            let disposal: JetProjectDisposal = permanently
                ? .permanent(acknowledgedWarning: removalPreview.permanentWarning)
                : .systemTrash
            let removed = try await activeClient().removeProject(
                preview: removalPreview,
                typedName: typedName,
                disposal: disposal,
                commandID: permanently ? removalPermanentCommandID : removalTrashCommandID
            )
            self.removalPreview = nil
            permanentRemovalAllowed = false
            let name = URL(fileURLWithPath: removed.root).lastPathComponent
            setupNotice = removed.disposition == "trashed"
                ? "\(name) was moved to Trash."
                : "\(name) was deleted."
            await loadSetup()
        } catch {
            let failure = presentationError(error)
            permanentRemovalAllowed = failure.code == "project.trash_unavailable"
            setupNotice = failure.message
        }
        setupOperation = nil
    }

    func connectHarness(_ provider: JetAuthProvider) async {
        guard setupOperation == nil else { return }
        setupOperation = "account-\(provider.provider)"
        setupNotice = nil
        let commandID = accountCommandIDs[provider.provider] ?? UUID()
        accountCommandIDs[provider.provider] = commandID
        do {
            let binding = try await activeClient().bindHarnessAccount(
                provider,
                commandID: commandID
            )
            accountCommandIDs.removeValue(forKey: provider.provider)
            setupNotice = "\(binding.label) is connected."
            await loadSetup()
        } catch {
            setupNotice = presentationError(error).message
        }
        setupOperation = nil
    }

    func skipRemotePairing() {
        remotePairingSkipped = true
        setupNotice = "Remote pairing was skipped. You can return here at any time."
    }

    func restore(selection: String, workPanel: String, panelPresented: Bool) {
        if let selection = SidebarDestination(rawValue: selection) {
            sidebarSelection = selection
        }
        if let workPanel = WorkPanelTab(rawValue: workPanel) {
            selectedWorkPanel = workPanel
        }
        isWorkPanelPresented = panelPresented
        applySidebarSelection()
    }

    func beginNewTask() {
        sidebarSelection = .newTask
        showScenario(.ready)
        isWorkPanelPresented = false
        actionNotice = nil
        composerFocusRequest += 1
    }

    func selectSearch() {
        sidebarSelection = .search
        actionNotice = "Search arrives with the Conversation list in Wave 1.3."
    }

    func applySidebarSelection() {
        actionNotice = nil
        switch sidebarSelection {
        case .newTask:
            showScenario(.ready)
            isWorkPanelPresented = false
            composerFocusRequest += 1
        case .search:
            actionNotice = "Search arrives with the Conversation list in Wave 1.3."
        case .needsAttention:
            showScenario(.approval, fallback: .recovery)
            isWorkPanelPresented = true
        case .project:
            showScenario(.ready)
            isWorkPanelPresented = false
        case .conversation:
            showScenario(.active)
            isWorkPanelPresented = true
        case .schedules:
            actionNotice = "Schedules are planned for Wave 3."
        case .planes:
            actionNotice = "Connection management is planned for Wave 3."
        }
    }

    func submitDraft() {
        guard canSubmitDraft else { return }
        // ASVS 2.1.1 and 2.2.2: Wave 1.1 keeps the draft in presentation
        // state. A later typed adapter must validate it at the trusted Plane
        // boundary before it can become a Command.
        actionNotice = "Task submission is not connected in this shell yet. Your draft was kept."
    }

    func showFixture(_ state: DesktopFixtureState) {
        actionNotice = nil
        showScenario(state)
    }

    func retryFixtureLoad() {
        didLoadFixtures = false
        contentState = .loading
        Task { await loadFoundationFixture() }
    }

    private func showScenario(
        _ state: DesktopFixtureState,
        fallback: DesktopFixtureState? = nil
    ) {
        if let scenario = scenarios[state] ?? fallback.flatMap({ scenarios[$0] }) {
            contentState = .ready(scenario)
        }
    }

    private func activeClient() async throws -> JetClient {
        if let client { return client }
        guard let makeJetClient else {
            throw JetClientFailure.presentation(.offline)
        }
        let client = try await makeJetClient()
        self.client = client
        observeConnection(of: client)
        return client
    }

    private func observeConnection(of client: JetClient) {
        connectionObservationTask?.cancel()
        connectionObservationTask = Task { [weak self] in
            let states = await client.connectionStates()
            for await state in states {
                guard !Task.isCancelled else { return }
                self?.connectionState = state
            }
        }
    }

    private func presentationError(_ error: Error) -> JetPresentationError {
        switch error {
        case let JetClientFailure.presentation(error): error
        case let JetClientFailure.commandOutcomeUnknown(commandID):
            JetPresentationError(
                category: .outcomeUnknown,
                code: "command.outcome_unknown",
                message: "Jet could not confirm this change. Refresh before trying again. Command \(commandID.uuidString.prefix(8)).",
                retryable: false,
                recoveryActions: []
            )
        default: .invalidResponse
        }
    }
}
