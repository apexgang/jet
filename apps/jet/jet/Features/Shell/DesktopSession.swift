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
    private struct PendingStart {
        let conversationID: UUID
        let craft: String
        let prompt: String
        let commandID: UUID
    }

    private struct PendingTurn {
        let conversationID: UUID
        let prompt: String
        let commandID: UUID
    }

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
    var conversations: [JetConversationSummary] = []
    var conversationCursor: UInt64 = 0
    var nextConversationPage: UUID?
    var selectedConversationID: UUID?
    var conversationSnapshot: JetConversationSnapshot?
    var conversationFreshness: JetConversationFreshness = .loading
    var conversationOperation: String?
    var timeline: [JetTimelineEntry] = []
    var searchText = ""
    var searchResult: JetSearchResult?
    var searchIsLoading = false

    private var scenarios: [DesktopFixtureState: DesktopFixtureScenario] = [:]
    private var didLoadFixtures = false
    private let makeJetClient: JetClientFactory?
    private var client: JetClient?
    private var connectionObservationTask: Task<Void, Never>?
    private var registrationCommandID = UUID()
    private var removalTrashCommandID = UUID()
    private var removalPermanentCommandID = UUID()
    private var accountCommandIDs: [String: UUID] = [:]
    private var createCommandProjectID: UUID?
    private var createCommandID = UUID()
    private var pendingStart: PendingStart?
    private var pendingTurn: PendingTurn?
    private var eventObservationTask: Task<Void, Never>?
    private var conversationRequest = 0
    private var searchRequest = 0

    init(makeJetClient: JetClientFactory? = nil) {
        self.makeJetClient = makeJetClient
    }

    var scenario: DesktopFixtureScenario? {
        guard case let .ready(scenario) = contentState else { return nil }
        return scenario
    }

    var usesLivePlane: Bool { makeJetClient != nil }

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
        if selectedConversationID != nil {
            guard let projectID = selectedConversation?.projectID else {
                return selectedConversation == nil ? "Project unavailable" : "No Project"
            }
            return setupSnapshot?.projects.projects.first(where: { $0.id == projectID })?.name
                ?? "Project unavailable"
        }
        if let selectedProject { return selectedProject.name }
        return usesLivePlane ? "Choose a Project" : scenario?.project?.name ?? "Choose a Project"
    }

    var selectedHarnessName: String {
        if let label = setupSnapshot?.accounts.bindings.first?.label
            ?? setupSnapshot?.capabilities.authProviders.first?.harness
        {
            return label
        }
        return usesLivePlane ? "Choose an Agent" : scenario?.capabilities.harnesses.first?.capitalized ?? "Choose"
    }

    var selectedCraftID: String? {
        setupSnapshot?.capabilities.crafts.first?.id
    }

    var selectedConversation: JetConversationSummary? {
        conversations.first { $0.id == selectedConversationID }
    }

    var selectedConversationTitle: String {
        selectedConversation?.title ?? "New task"
    }

    var selectedRun: JetRunSummary? {
        conversationSnapshot?.runs.last
    }

    var hasLiveRun: Bool {
        selectedRun?.lifecycle.isLive == true
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

    func loadConversations(restoring restoredID: UUID? = nil) async {
        guard makeJetClient != nil else { return }
        conversationRequest += 1
        let request = conversationRequest
        if conversations.isEmpty { conversationFreshness = .loading }
        do {
            let client = try await activeClient()
            let page = try await client.conversations()
            guard request == conversationRequest else { return }
            conversations = page.conversations
            conversationCursor = page.cursor
            nextConversationPage = page.nextPage
            conversationFreshness = .live
            let previousConversationID = selectedConversationID
            let candidate = restoredID ?? selectedConversationID
            var restoredSnapshot: JetConversationSnapshot?
            if let candidate,
               !conversations.contains(where: { $0.id == candidate })
            {
                restoredSnapshot = try? await client.conversation(candidate)
                guard request == conversationRequest else { return }
                if let restoredSnapshot {
                    conversations.append(restoredSnapshot.conversation)
                }
            }
            selectedConversationID = candidate.flatMap { wanted in
                conversations.contains(where: { $0.id == wanted }) ? wanted : nil
            } ?? conversations.first?.id
            if selectedConversationID != previousConversationID {
                timeline = []
                conversationSnapshot = nil
            }
            if let restoredSnapshot,
               restoredSnapshot.conversation.id == selectedConversationID
            {
                conversationSnapshot = restoredSnapshot
            } else if selectedConversationID != nil {
                await loadSelectedConversation()
            } else {
                conversationSnapshot = nil
            }
            if eventObservationTask == nil {
                observeEvents(after: page.cursor)
            }
        } catch {
            guard request == conversationRequest else { return }
            let failure = presentationError(error)
            conversationFreshness = conversations.isEmpty ? .failed : .cached
            actionNotice = failure.message
        }
    }

    func loadMoreConversations() async {
        guard let nextConversationPage, conversationOperation == nil else { return }
        conversationOperation = "page"
        do {
            let page = try await activeClient().nextConversations(nextConversationPage)
            let known = Set(conversations.map(\.id))
            conversations.append(contentsOf: page.conversations.filter { !known.contains($0.id) })
            self.nextConversationPage = page.nextPage
            conversationFreshness = .live
        } catch {
            let failure = presentationError(error)
            if failure.code == "pagination.stale" {
                await loadConversations()
            } else {
                actionNotice = failure.message
            }
        }
        conversationOperation = nil
    }

    func searchConversations() async {
        searchRequest += 1
        let request = searchRequest
        let text = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            searchResult = nil
            searchIsLoading = false
            return
        }
        searchIsLoading = true
        do {
            let result = try await activeClient().searchConversations(text)
            guard request == searchRequest else { return }
            searchResult = result
        } catch {
            guard request == searchRequest else { return }
            actionNotice = presentationError(error).message
        }
        if request == searchRequest { searchIsLoading = false }
    }

    func selectConversation(_ conversationID: UUID) {
        if selectedConversationID != conversationID {
            timeline = []
            conversationSnapshot = nil
        }
        selectedConversationID = conversationID
        sidebarSelection = .conversation
        isWorkPanelPresented = true
        actionNotice = nil
        Task { await loadSelectedConversation() }
    }

    func selectSearchHit(_ conversationID: UUID) {
        selectConversation(conversationID)
    }

    private func loadSelectedConversation() async {
        guard let selectedConversationID else { return }
        conversationOperation = "snapshot"
        do {
            let snapshot = try await activeClient().conversation(selectedConversationID)
            guard self.selectedConversationID == selectedConversationID else { return }
            conversationSnapshot = snapshot
            mergeConversation(snapshot.conversation)
            conversationFreshness = .live
        } catch {
            guard self.selectedConversationID == selectedConversationID else { return }
            conversationFreshness = conversationSnapshot == nil ? .failed : .cached
            actionNotice = presentationError(error).message
        }
        conversationOperation = nil
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
        selectedConversationID = nil
        conversationSnapshot = nil
        timeline = []
        isWorkPanelPresented = false
        actionNotice = nil
        composerFocusRequest += 1
    }

    func selectSearch() {
        sidebarSelection = .search
        isWorkPanelPresented = false
        actionNotice = nil
    }

    func applySidebarSelection() {
        actionNotice = nil
        switch sidebarSelection {
        case .newTask:
            selectedConversationID = nil
            conversationSnapshot = nil
            timeline = []
            isWorkPanelPresented = false
            composerFocusRequest += 1
        case .search:
            isWorkPanelPresented = false
        case .needsAttention:
            if usesLivePlane {
                actionNotice = "The attention inbox arrives with Run controls in Wave 2.1."
                isWorkPanelPresented = false
            } else {
                showScenario(.approval, fallback: .recovery)
                isWorkPanelPresented = true
            }
        case .project:
            showScenario(.ready)
            isWorkPanelPresented = false
        case .conversation:
            isWorkPanelPresented = true
        case .schedules:
            actionNotice = "Schedules are planned for Wave 3."
        case .planes:
            actionNotice = "Connection management is planned for Wave 3."
        }
    }

    func submitDraft() async {
        guard canSubmitDraft, conversationOperation == nil else { return }
        guard planeIsConnected else {
            actionNotice = "Reconnect to the Plane before sending. Your draft was kept."
            return
        }
        guard let craft = selectedCraftID else {
            actionNotice = "Install an available Craft before starting work."
            return
        }

        conversationOperation = "send"
        actionNotice = nil
        let prompt = draft
        do {
            var conversationID = selectedConversationID
            if conversationID == nil {
                guard let selectedProjectID else {
                    actionNotice = "Choose a Project before starting a task."
                    conversationOperation = nil
                    return
                }
                if createCommandProjectID != selectedProjectID {
                    createCommandProjectID = selectedProjectID
                    createCommandID = UUID()
                }
                let conversation = try await activeClient().createConversation(
                    projectID: selectedProjectID,
                    commandID: createCommandID
                )
                createCommandProjectID = nil
                createCommandID = UUID()
                mergeConversation(conversation)
                conversationID = conversation.id
                self.selectedConversationID = conversation.id
                sidebarSelection = .conversation
                isWorkPanelPresented = true
                conversationSnapshot = try await activeClient().conversation(conversation.id)
            }

            guard let conversationID else {
                conversationOperation = nil
                actionNotice = "Jet could not prepare this Conversation. Try again."
                return
            }
            if hasLiveRun {
                let pending = pendingTurnFor(
                    conversationID: conversationID,
                    prompt: prompt
                )
                _ = try await activeClient().submitTurn(
                    conversationID: conversationID,
                    prompt: prompt,
                    commandID: pending.commandID
                )
                pendingTurn = nil
            } else {
                let pending = pendingStartFor(
                    conversationID: conversationID,
                    craft: craft,
                    prompt: prompt
                )
                _ = try await activeClient().startRun(
                    conversationID: conversationID,
                    craft: craft,
                    prompt: prompt,
                    commandID: pending.commandID
                )
                pendingStart = nil
            }
            draft = ""
            actionNotice = "Sent to the Plane."
            await loadSelectedConversation()
        } catch {
            actionNotice = presentationError(error).message
            await loadConversations()
        }
        conversationOperation = nil
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

    private func mergeConversation(_ conversation: JetConversationSummary) {
        if let index = conversations.firstIndex(where: { $0.id == conversation.id }) {
            conversations[index] = conversation
        } else {
            conversations.insert(conversation, at: 0)
        }
    }

    private func pendingStartFor(
        conversationID: UUID,
        craft: String,
        prompt: String
    ) -> PendingStart {
        if let pendingStart,
           pendingStart.conversationID == conversationID,
           pendingStart.craft == craft,
           pendingStart.prompt == prompt
        {
            return pendingStart
        }
        let pending = PendingStart(
            conversationID: conversationID,
            craft: craft,
            prompt: prompt,
            commandID: UUID()
        )
        pendingStart = pending
        return pending
    }

    private func pendingTurnFor(
        conversationID: UUID,
        prompt: String
    ) -> PendingTurn {
        if let pendingTurn,
           pendingTurn.conversationID == conversationID,
           pendingTurn.prompt == prompt
        {
            return pendingTurn
        }
        let pending = PendingTurn(
            conversationID: conversationID,
            prompt: prompt,
            commandID: UUID()
        )
        pendingTurn = pending
        return pending
    }

    private func observeEvents(after initialCursor: UInt64) {
        eventObservationTask?.cancel()
        eventObservationTask = Task { [weak self] in
            guard let self else { return }
            var cursor = initialCursor
            while !Task.isCancelled {
                do {
                    let client = try await activeClient()
                    let events = await client.eventStream(after: cursor)
                    for try await event in events {
                        guard !Task.isCancelled else { return }
                        cursor = event.sequence
                        await receive(event)
                    }
                } catch {
                    let failure = presentationError(error)
                    if failure.code == "event.cursor_expired"
                        || failure.code == "event.cursor_ahead"
                    {
                        timeline = []
                        actionNotice = "The activity cursor expired. Jet refreshed the full Conversation snapshot."
                        await loadConversations()
                        cursor = conversationCursor
                        continue
                    }
                    if conversationSnapshot != nil { conversationFreshness = .cached }
                    actionNotice = failure.message
                    try? await Task.sleep(for: .seconds(1))
                }
            }
        }
    }

    private func receive(_ event: JetEvent) async {
        if event.conversationID == selectedConversationID {
            let projections = event.timelineProjections()
            if projections.isEmpty {
                groupRawEvent(sequence: event.sequence)
            } else {
                for projection in projections {
                    mergeTimeline(projection)
                }
            }
            if event.kind == "run.lifecycle_changed"
                || event.kind == "run.activity_changed"
                || event.kind == "turn.changed"
            {
                await loadSelectedConversation()
            }
        }
        if ["conversation.created", "conversation.name_changed", "conversation.trashed"]
            .contains(event.kind)
        {
            await loadConversations()
        }
    }

    private func mergeTimeline(_ entry: JetTimelineEntry) {
        if entry.kind == .user,
           let index = timeline.firstIndex(where: { $0.id == entry.id })
        {
            timeline[index].text += entry.text
            timeline[index].sequence = entry.sequence
        } else {
            timeline.append(entry)
            if timeline.count > 256 { timeline.removeFirst(timeline.count - 256) }
        }
    }

    private func groupRawEvent(sequence: UInt64) {
        if let index = timeline.indices.last, timeline[index].rawCount > 0 {
            timeline[index].rawCount += 1
            timeline[index].text = "\(timeline[index].rawCount) background updates"
            timeline[index].sequence = sequence
        } else {
            timeline.append(
                JetTimelineEntry(
                    id: "raw-\(sequence)",
                    kind: .activity,
                    text: "1 background update",
                    sequence: sequence,
                    rawCount: 1
                )
            )
        }
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
                if case .connected = state,
                   self?.conversationFreshness == .cached
                {
                    await self?.loadConversations()
                } else if case .reconnecting = state,
                          self?.conversationSnapshot != nil
                {
                    self?.conversationFreshness = .cached
                } else if case .disconnected = state,
                          self?.conversationSnapshot != nil
                {
                    self?.conversationFreshness = .cached
                }
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
