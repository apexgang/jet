import CryptoKit
import Foundation
import Observation

typealias JetClientFactory = @Sendable () async throws -> JetClient
typealias JetRemoteClientFactory = @Sendable (String) async throws -> JetClient
typealias JetRemotePairingClaimer = @Sendable (String, String, UUID) async throws -> JetRemotePairingClaim
typealias JetRemotePairingCompleter = @Sendable (JetRemotePairingClaim, UUID) async throws -> JetPairedClientSummary
typealias JetNotificationPreference = @MainActor (JetNotificationKind) -> Bool

enum SidebarDestination: String, CaseIterable, Hashable, Sendable {
    case newTask
    case search
    case needsAttention
    case project
    case conversation
    case schedules
    case planes
    case trash

    /// Retired destinations that a restored scene maps back to a task.
    var restoresAsConversation: Bool {
        switch self {
        case .search, .needsAttention, .schedules, .planes: true
        case .newTask, .project, .conversation, .trash: false
        }
    }
}

enum WorkPanelTab: String, CaseIterable, Hashable, Sendable {
    case changes
    case files
    case terminal
    case run

    /// Raw values stay for SceneStorage. `files` is Changes in edit mode and
    /// `run` is titled Activity.
    var title: String {
        switch self {
        case .changes: "Changes"
        case .files: "Changes"
        case .terminal: "Terminal"
        case .run: "Activity"
        }
    }

    /// The Details segments, in order.
    static let inspectorTabs: [WorkPanelTab] = [.changes, .terminal, .run]
}

enum WorkCheckpointKind: String, CaseIterable, Hashable, Sendable {
    case current
    case final
    case turn
    case historical

    var title: String {
        switch self {
        case .current: "Current"
        case .final: "Final"
        case .turn: "Turn"
        case .historical: "Historical Range"
        }
    }
}

struct TerminalTranscriptDecoder {
    private enum EscapeState {
        case text
        case escape
        case controlSequence
        case operatingSystemCommand
        case operatingSystemCommandEscape
    }

    private var pending = Data()
    private var escapeState = EscapeState.text

    mutating func decode(_ bytes: Data, final: Bool = false) -> String {
        pending.append(bytes)
        let split = final ? pending.count : completeUTF8PrefixLength(in: pending)
        let complete = pending.prefix(split)
        pending.removeFirst(split)
        if final, !pending.isEmpty {
            pending.removeAll(keepingCapacity: true)
        }
        return sanitize(String(decoding: complete, as: UTF8.self))
    }

    private func completeUTF8PrefixLength(in data: Data) -> Int {
        guard !data.isEmpty else { return 0 }
        let bytes = [UInt8](data)
        let lowerBound = max(0, bytes.count - 4)
        for index in stride(from: bytes.count - 1, through: lowerBound, by: -1) {
            let byte = bytes[index]
            if byte & 0b1100_0000 == 0b1000_0000 { continue }
            let expected: Int
            switch byte {
            case 0xC2 ... 0xDF: expected = 2
            case 0xE0 ... 0xEF: expected = 3
            case 0xF0 ... 0xF4: expected = 4
            default: return bytes.count
            }
            return bytes.count - index < expected ? index : bytes.count
        }
        return bytes.count
    }

    private mutating func sanitize(_ value: String) -> String {
        var result = ""
        for scalar in value.unicodeScalars {
            switch escapeState {
            case .text:
                if scalar.value == 0x1B {
                    escapeState = .escape
                } else if scalar.value == 0x0A || scalar.value == 0x0D || scalar.value == 0x09
                    || (scalar.value >= 0x20 && scalar.value != 0x7F)
                {
                    result.unicodeScalars.append(scalar)
                }
            case .escape:
                if scalar == "[" { escapeState = .controlSequence }
                else if scalar == "]" { escapeState = .operatingSystemCommand }
                else { escapeState = .text }
            case .controlSequence:
                if (0x40 ... 0x7E).contains(scalar.value) { escapeState = .text }
            case .operatingSystemCommand:
                if scalar.value == 0x07 { escapeState = .text }
                else if scalar.value == 0x1B { escapeState = .operatingSystemCommandEscape }
            case .operatingSystemCommandEscape:
                if scalar == "\\" { escapeState = .text }
                else if scalar.value != 0x1B { escapeState = .operatingSystemCommand }
            }
        }
        return result
    }
}

struct WorkRefreshContinuity {
    static func shouldLoadNextPage(
        loadedCount: Int,
        targetCount: Int,
        selectedPath: String?,
        loadedPaths: Set<String>,
        hasNextPage: Bool
    ) -> Bool {
        guard hasNextPage else { return false }
        return loadedCount < targetCount
            || selectedPath.map { !loadedPaths.contains($0) } == true
    }
}

// Internal for DesktopSession+*.swift extensions. Views use DesktopSession+Intents.
@MainActor
@Observable
final class DesktopSession {
    struct PlaneConversationLoad: Sendable {
        let planeRegistryID: UUID
        let result: Result<JetConversationPage, JetPresentationError>
    }

    struct PlaneSearchLoad: Sendable {
        let planeRegistryID: UUID
        let planeName: String
        let result: Result<JetSearchResult, JetPresentationError>
    }

    struct PlaneConversationPageLoad: Sendable {
        let planeRegistryID: UUID
        let result: Result<JetConversationPage, JetPresentationError>
    }

    struct PendingStart {
        let conversationID: UUID
        let craft: String
        let prompt: String
        let commandID: UUID
    }

    struct PendingTurn {
        let conversationID: UUID
        let prompt: String
        let commandID: UUID
    }

    struct RunControlKey: Hashable {
        let runID: UUID
        let control: JetRunControl
    }

    struct PairingGateIntent: Hashable {
        let planeRegistryID: UUID
        let open: Bool
    }

    struct PairingConfirmationIntent: Hashable {
        let planeRegistryID: UUID
        let offerID: UUID
    }

    struct PairedClientAccessIntent: Hashable {
        let planeRegistryID: UUID
        let clientID: UUID
        let access: JetPairedClientAccess
    }

    struct PairedClientIntent: Hashable {
        let planeRegistryID: UUID
        let clientID: UUID
    }

    struct PendingWorkEdit {
        let path: String
        let content: String
        let commandID: UUID
    }

    struct PendingReview {
        let path: String
        let line: UInt32
        let comment: String
        let commandID: UUID
    }

    struct PendingGitDelivery {
        let request: JetGitDeliveryRequest
        let commandID: UUID
    }

    enum ContentState {
        case loading
        case ready(DesktopFixtureScenario)
        case failed(String)
    }

    enum FixtureLoadResult: Sendable {
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
    var selectedWorkPanel: WorkPanelTab = .changes
    var isWorkPanelPresented = false
    var chosenCraftID: String?
    /// Each destination's draft, in memory only.
    var drafts: [DraftKey: String] = [:]
    var composerFocusRequest = 0
    /// Asks the shell to reveal the sidebar (for example before focusing search).
    var revealSidebarRequest = 0
    var searchFocusRequest = 0
    /// Asks the scene to open the Settings window after `requestedSettingsPane` changed.
    var settingsOpenRequest = 0
    /// The line above the composer.
    var composerNotice: ComposerNotice?
    var actionError: JetPresentationError?
    var presentedSheet: ShellSheet?
    var pendingNavigation: PendingNavigation?
    /// The person's own operation; background refreshes use the flags below.
    var userOperation: UserOperation?
    var isRefreshingConversation = false
    var isLoadingMoreConversations = false
    /// Set by Interrupt and Reply so the composer asks what to do instead.
    var interruptThenReply = false
    var composerPlaceholderOverride: String?
    /// Bounded setup retries at 2, 5 and 15 seconds.
    var setupRetryAttempt = 0
    var setupRetryLimit = DesktopSession.setupRetryDelays.count
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
    var planes: [JetPlanePresentation]
    var remoteProfiles: [JetRemotePlaneProfile]
    var newTaskPlaneRegistryID: UUID
    var remotePlaneName = ""
    var remoteSSHEndpoint = ""
    var remotePairingSecret = ""
    var remotePairingClaim: JetRemotePairingClaim?
    var remotePairingOperation: String?
    var remotePairingNotice: String?
    var openedPairing: JetOpenedPairing?
    var openedPairingPlaneID: UUID?
    var pairedClientPendingRevocation: JetPairedClientSummary?
    var pairedClientPendingRevocationPlaneID: UUID?
    var conversations: [JetConversationSummary] = []
    var conversationCursor: UInt64 = 0
    var nextConversationPage: UUID?
    var selectedConversationID: UUID?
    var conversationSnapshot: JetConversationSnapshot?
    var conversationFreshness: JetConversationFreshness = .loading
    var supervisionOperation: String?
    var timeline: [JetTimelineEntry] = []
    var turnQueue: JetTurnQueue?
    var runExecution: JetRunExecution?
    var runControlConfirmation: JetRunControl?
    var searchText = ""
    var searchResult: JetFederatedSearchResult?
    var searchIsLoading = false
    var workDiff: JetChangeDiff?
    var workFiles: [JetChangedFile] = []
    var workNextPage: UUID?
    var workPatch = ""
    var workTarget: JetFileTarget?
    var workTerminals: [JetWorkspaceTerminal] = []
    var workOperation: String?
    var workError: JetPresentationError?
    var workNotice: String?
    var workNoticeError: JetPresentationError?
    var checkpointKind: WorkCheckpointKind = .current
    var checkpointTurn: UInt32 = 1
    var checkpointFromTurn: UInt32 = 0
    var checkpointToTurn: UInt32 = 1
    var selectedWorkFilePath: String?
    var editableFile: JetEditableFile?
    var fileDraft = ""
    var reviewLine: UInt32 = 1
    var reviewComment = ""
    var selectedTerminalID: UUID?
    var attachedTerminalID: UUID?
    var terminalInput = ""
    var terminalOutput: [UUID: String] = [:]
    var terminalRows: UInt16 = 24
    var terminalColumns: UInt16 = 80
    var workScrollAnchors: [WorkPanelTab: String] = [:]
    var gitDeliveries: [JetGitDelivery] = []
    var gitDeliveryChoice: JetGitDeliveryChoice = .commit
    var gitBranchName = ""
    var gitRemoteName = "origin"
    var gitBaseBranch = ""
    var gitDeliveryOperation: String?
    var gitDeliveryNotice: String?
    var gitDeliveryError: JetPresentationError?
    var gitDeliveryConfirmation: JetGitDeliveryRequest?
    var gitDeliveryAcknowledgementConfirmation: JetGitDelivery?
    var gitDeliveryAdmissionUncertain: JetGitDeliveryRequest?
    var notificationAuthorization: JetNotificationAuthorization = .notDetermined
    var notificationError: String?
    var requestedSettingsPane: JetSettingsPane?

    var scenarios: [DesktopFixtureState: DesktopFixtureScenario] = [:]
    var didLoadFixtures = false
    let makeJetClient: JetClientFactory?
    let makeRemoteClient: JetRemoteClientFactory?
    let claimRemotePairing: JetRemotePairingClaimer?
    let completeRemotePairing: JetRemotePairingCompleter?
    let saveRemoteProfiles: @MainActor ([JetRemotePlaneProfile]) -> Void
    let localPlaneRegistryID: UUID
    let notifications: (any JetNotificationDelivering)?
    let notificationPreference: JetNotificationPreference
    var client: JetClient?
    var remoteClients: [UUID: JetClient] = [:]
    var conversationPlaneRegistryIDs: [UUID: UUID] = [:]
    var planeConversationCursors: [UUID: UInt64] = [:]
    var planeNextConversationPages: [UUID: UUID] = [:]
    var planeConversations: [UUID: [JetConversationSummary]] = [:]
    var planeConnectionObservationTasks: [UUID: Task<Void, Never>] = [:]
    var connectionObservationTask: Task<Void, Never>?
    var registrationCommandID = UUID()
    var removalTrashCommandID = UUID()
    var removalPermanentCommandID = UUID()
    var accountCommandIDs: [String: UUID] = [:]
    var createCommandProjectID: UUID?
    var createCommandID = UUID()
    var pendingStart: PendingStart?
    var pendingTurn: PendingTurn?
    var withdrawalCommandIDs: [UUID: UUID] = [:]
    var runControlCommandIDs: [RunControlKey: UUID] = [:]
    var approvalRetryCommandIDs: [UUID: UUID] = [:]
    var eventObservationTasks: [UUID: Task<Void, Never>] = [:]
    var conversationRequest = 0
    var searchRequest = 0
    var workRequest = 0
    var workFileRequest = 0
    var workArtifactBytes = Data()
    var pendingWorkEdit: PendingWorkEdit?
    var pendingReview: PendingReview?
    var terminalOpenCommandID = UUID()
    var terminalCloseCommandIDs: [UUID: UUID] = [:]
    var terminalOffsets: [UUID: UInt64] = [:]
    var terminalDecoders: [UUID: TerminalTranscriptDecoder] = [:]
    var lastTerminalSize: (terminalID: UUID, rows: UInt16, columns: UInt16)?
    var terminalStreamTerminalID: UUID?
    var terminalObservationTask: Task<Void, Never>?
    var gitDeliveryObservationTask: Task<Void, Never>?
    var pendingGitDelivery: PendingGitDelivery?
    var gitDeliveryAcknowledgementCommandIDs: [UUID: UUID] = [:]
    var pairingGateCommandIDs: [PairingGateIntent: UUID] = [:]
    var openPairingCommandIDs: [UUID: UUID] = [:]
    var confirmPairingCommandIDs: [PairingConfirmationIntent: UUID] = [:]
    var pairedClientAccessCommandIDs: [PairedClientAccessIntent: UUID] = [:]
    var pairedClientRevokeCommandIDs: [PairedClientIntent: UUID] = [:]
    var remotePairingClaimCommandID = UUID()
    var remotePairingCompleteCommandID = UUID()

    let statusStore: TaskStatusStore
    let transcripts: TranscriptStore
    let memory: ClientMemory
    let deliveries: DeliveryCoordinator
    let notificationRouter: JetNotificationRouter
    /// A session seeded for previews and screenshots: it renders live paths but
    /// never reaches a Plane.
    let isPreviewSession: Bool
    @ObservationIgnored var setupRetryTask: Task<Void, Never>?
    @ObservationIgnored var selectionLoadTask: Task<Void, Never>?
    @ObservationIgnored var selectionGeneration = 0
    @ObservationIgnored var conversationRefreshes = 0
    /// Keeps a scope chosen through `showChanges` when the next change load starts.
    @ObservationIgnored var keepsRequestedCheckpoint = false
    @ObservationIgnored var restoreCommandIDs: [UUID: UUID] = [:]
    /// Tasks this session saw moved to Jet Trash. A trashed task can still be
    /// queried by ID, so a refresh must not bring it back as the open task.
    @ObservationIgnored var trashedConversationIDs: Set<UUID> = []

    static let setupRetryDelays: [Duration] = [.seconds(2), .seconds(5), .seconds(15)]

    init(
        makeJetClient: JetClientFactory? = nil,
        localPlaneRegistryID: UUID = UUID(),
        remoteProfiles: [JetRemotePlaneProfile] = [],
        makeRemoteClient: JetRemoteClientFactory? = nil,
        claimRemotePairing: JetRemotePairingClaimer? = nil,
        completeRemotePairing: JetRemotePairingCompleter? = nil,
        saveRemoteProfiles: @escaping @MainActor ([JetRemotePlaneProfile]) -> Void = { _ in },
        notifications: (any JetNotificationDelivering)? = nil,
        notificationPreference: @escaping JetNotificationPreference = {
            JetNotificationPreferences.isEnabled($0)
        },
        memory: ClientMemory? = nil,
        isPreviewSession: Bool = false
    ) {
        let memory = memory ?? ClientMemory()
        self.memory = memory
        statusStore = TaskStatusStore(memory: memory)
        transcripts = TranscriptStore()
        deliveries = DeliveryCoordinator()
        notificationRouter = JetNotificationRouter(
            notifications: notifications,
            preference: notificationPreference
        )
        self.isPreviewSession = isPreviewSession
        self.makeJetClient = makeJetClient
        self.localPlaneRegistryID = localPlaneRegistryID
        self.remoteProfiles = remoteProfiles
        self.makeRemoteClient = makeRemoteClient
        self.claimRemotePairing = claimRemotePairing
        self.completeRemotePairing = completeRemotePairing
        self.saveRemoteProfiles = saveRemoteProfiles
        self.notifications = notifications
        self.notificationPreference = notificationPreference
        newTaskPlaneRegistryID = localPlaneRegistryID
        planes = [
            JetPlanePresentation(
                id: localPlaneRegistryID,
                name: "This Mac",
                endpoint: nil,
                isLocal: true,
                planeID: nil,
                connection: .disconnected,
                snapshot: nil,
                failure: nil,
                conversationCursor: nil
            ),
        ] + remoteProfiles.map {
            JetPlanePresentation(
                id: $0.id,
                name: $0.name,
                endpoint: $0.endpoint,
                isLocal: false,
                planeID: $0.planeID,
                connection: .disconnected,
                snapshot: nil,
                failure: nil,
                conversationCursor: nil
            )
        }
        configureStores()
    }

    /// Wires the stores to this session's clients. Closures hold the session weakly.
    private func configureStores() {
        notificationRouter.onDeliveryFailure = { [weak self] in
            self?.notificationError = String(localized: "Jet couldn't show a notification.")
        }
        statusStore.configure { [weak self] conversationID, planeRegistryID in
            guard let self else { throw CancellationError() }
            let client = try await self.client(for: planeRegistryID)
            let snapshot = try await client.conversation(conversationID)
            var execution: JetRunExecution?
            if let runID = snapshot.runs.last?.id {
                execution = try await client.runExecution(runID: runID)
            }
            return (
                cursor: max(snapshot.cursor, execution?.cursor ?? 0),
                snapshot: snapshot,
                execution: execution
            )
        }
        transcripts.configure { [weak self] planeRegistryID, cursor in
            guard let self else { throw CancellationError() }
            return try await self.client(for: planeRegistryID).events(after: cursor)
        }
        deliveries.configure(
            submit: { [weak self] key, request, commandID in
                guard let self else { throw CancellationError() }
                return try await self.client(for: key.planeRegistryID)
                    .deliverGit(request, commandID: commandID)
                    .deliveryID
            },
            fetch: { [weak self] key in
                guard let self else { throw CancellationError() }
                return try await self.client(for: key.planeRegistryID)
                    .gitDeliveries(conversationID: key.conversationID)
            },
            onDeliveries: { [weak self] key, deliveries in
                guard let self else { return }
                self.statusStore.recordGitDeliveries(deliveries, conversationID: key.conversationID)
                if self.selectedConversationID == key.conversationID {
                    self.gitDeliveries = deliveries
                }
            }
        )
    }

    /// Cancels work tied to this session's lifetime.
    func teardown() {
        setupRetryTask?.cancel()
        setupRetryTask = nil
        selectionLoadTask?.cancel()
        selectionLoadTask = nil
        for task in eventObservationTasks.values { task.cancel() }
        eventObservationTasks = [:]
        for task in planeConnectionObservationTasks.values { task.cancel() }
        planeConnectionObservationTasks = [:]
        connectionObservationTask?.cancel()
        connectionObservationTask = nil
        gitDeliveryObservationTask?.cancel()
        gitDeliveryObservationTask = nil
        transcripts.cancelReplay()
        detachCurrentTerminal()
    }

    /// The draft of the current destination.
    var draftKey: DraftKey {
        if sidebarSelection == .newTask { return .newTask }
        return selectedConversationID.map { .conversation($0) } ?? .newTask
    }

    var draft: String {
        get { drafts[draftKey] ?? "" }
        set {
            let key = draftKey
            if newValue.isEmpty {
                drafts.removeValue(forKey: key)
            } else {
                drafts[key] = newValue
            }
            if composerNotice?.kind == .confirmation { composerNotice = nil }
        }
    }

    var hasNewTaskDraft: Bool {
        !(drafts[.newTask] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// Compatibility for views that predate `userOperation`.
    var conversationOperation: String? { userOperation?.rawValue }

    /// Compatibility bridge to `composerNotice`; setting text shows an info notice.
    var actionNotice: String? {
        get { composerNotice?.text }
        set { composerNotice = newValue.map { ComposerNotice(kind: .info, text: $0) } }
    }

    var scenario: DesktopFixtureScenario? {
        guard case let .ready(scenario) = contentState else { return nil }
        return scenario
    }

    var usesLivePlane: Bool { makeJetClient != nil }

    var canSubmitDraft: Bool {
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && draftBytes <= JetTurnQueue.maximumPromptBytes
            && !queueIsFull
    }

    var draftBytes: Int { draft.utf8.count }
    var queueIsFull: Bool {
        (turnQueue?.turns.count ?? 0) >= JetTurnQueue.maximumEntries
    }

    var attentionCount: Int {
        let approvals = timeline.filter {
            $0.approval?.state == .requested || $0.approval?.state == .unavailable
        }.count
        return approvals + (runExecution?.needsAttention == true ? 1 : 0)
    }

    var setupSnapshot: JetSetupSnapshot? {
        guard case let .ready(snapshot) = setupState else { return nil }
        return snapshot
    }

    var selectedPlaneRegistryID: UUID {
        selectedConversationID.flatMap { conversationPlaneRegistryIDs[$0] }
            ?? newTaskPlaneRegistryID
    }

    var selectedPlane: JetPlanePresentation? {
        planes.first { $0.id == selectedPlaneRegistryID }
    }

    var selectedPlaneName: String { selectedPlane?.name ?? "This Mac" }

    var selectedSetupSnapshot: JetSetupSnapshot? {
        selectedPlane?.snapshot ?? (selectedPlaneRegistryID == localPlaneRegistryID ? setupSnapshot : nil)
    }

    var allProjects: [JetPlaneProject] {
        planes.flatMap { plane in
            (plane.snapshot?.projects.projects ?? []).map {
                JetPlaneProject(planeRegistryID: plane.id, planeName: plane.name, project: $0)
            }
        }
    }

    var hasMoreConversations: Bool { !planeNextConversationPages.isEmpty }

    var unavailablePlanes: [JetPlanePresentation] {
        planes.filter { $0.failure != nil }
    }

    var selectedProject: JetProjectSummary? {
        selectedSetupSnapshot?.projects.projects.first { $0.id == selectedProjectID }
    }

    var selectedProjectName: String {
        if selectedConversationID != nil {
            guard let projectID = selectedConversation?.projectID else {
                return selectedConversation == nil ? "Project unavailable" : "No Project"
            }
            return selectedSetupSnapshot?.projects.projects.first(where: { $0.id == projectID })?.name
                ?? "Project unavailable"
        }
        if let selectedProject { return selectedProject.name }
        return usesLivePlane ? "Choose a Project" : scenario?.project?.name ?? "Choose a Project"
    }

    var selectedHarnessName: String {
        guard let craft = selectedSetupSnapshot?.capabilities.crafts.first(where: { $0.id == selectedCraftID })
        else { return String(localized: "Choose an Assistant") }
        return Self.harnessLabel(craft.harnesses.first ?? craft.id)
    }

    var selectedCraftID: String? {
        chosenCraftID ?? selectedSetupSnapshot?.capabilities.crafts.first?.id
    }

    static func harnessLabel(_ name: String) -> String {
        switch name {
        case "codex": "Codex"
        case "claude-code": "Claude Code"
        default: name
        }
    }

    var canStartTask: Bool {
        planeIsConnected && selectedProject != nil
            && selectedSetupSnapshot?.capabilities.crafts.contains(where: { $0.id == selectedCraftID }) == true
    }

    func chooseCraft(_ id: String) {
        guard userOperation == nil,
              selectedSetupSnapshot?.capabilities.crafts.contains(where: { $0.id == id }) == true
        else { return }
        chosenCraftID = id
    }

    /// An explicit "New Task in Project" command, so it also focuses the composer.
    func useProjectForNewTask(_ id: UUID, on planeID: UUID) {
        selectProject(id, on: planeID)
        beginNewTask()
        composerFocusRequest += 1
    }

    var selectedConversation: JetConversationSummary? {
        conversations.first { $0.id == selectedConversationID }
    }

    var selectedConversationTitle: String {
        selectedConversation?.title ?? String(localized: "New Task")
    }

    var selectedRun: JetRunSummary? {
        runExecution?.run ?? conversationSnapshot?.runs.last
    }

    var selectedWorkFile: JetChangedFile? {
        guard let selectedWorkFilePath else { return nil }
        return workFiles.first { $0.path == selectedWorkFilePath }
    }

    var selectedChangeScope: JetChangeScope {
        switch checkpointKind {
        case .current: .current
        case .final: .final
        case .turn: .turn(max(1, checkpointTurn))
        case .historical:
            .historical(
                fromTurn: checkpointFromTurn,
                toTurn: max(1, checkpointToTurn)
            )
        }
    }

    var canApplyWorkCheckpoint: Bool {
        guard workOperation == nil else { return false }
        let latestTurn = workDiff?.latestTurn ?? 0
        switch checkpointKind {
        case .current:
            return true
        case .final:
            return selectedRun?.lifecycle.isLive != true
        case .turn:
            return checkpointTurn > 0 && checkpointTurn <= latestTurn
        case .historical:
            return checkpointFromTurn <= checkpointToTurn && checkpointToTurn <= latestTurn
        }
    }

    var workPatchBytesLoaded: UInt64 { UInt64(workArtifactBytes.count) }

    var gitDeliveryUnavailableReason: String? {
        guard usesLivePlane else { return String(localized: "Connect to a computer to keep changes.") }
        guard planeIsConnected else { return String(localized: "Not connected to \(selectedPlaneName).") }
        guard selectedSetupSnapshot?.capabilities.gitIsAvailable == true else {
            return String(localized: "Git isn't available on \(selectedPlaneName).")
        }
        guard workDiff != nil else { return String(localized: "Load the changes first.") }
        return nil
    }

    var canPrepareGitDelivery: Bool {
        gitDeliveryUnavailableReason == nil && gitDeliveryOperation == nil
    }

    var hasLiveRun: Bool {
        selectedRun?.lifecycle.isLive == true
    }

    var canInterruptTurn: Bool {
        guard let run = selectedRun, run.lifecycle == .active else { return false }
        return turnQueue?.turns.contains { $0.runID == run.id && $0.state == .active } == true
    }

    var canStopRun: Bool {
        selectedRun?.lifecycle == .active
    }

    var planeConnectionLabel: String {
        switch connectionState {
        case .disconnected where setupStateIsLoading: "Starting"
        case .disconnected: setupStateIsIdle ? "Not checked" : "Offline"
        case .connecting: "Connecting"
        case .connected: "Connected"
        case .reconnecting: "Reconnecting"
        case .failed: "Unavailable"
        }
    }

    var planeIsConnected: Bool {
        if selectedPlaneRegistryID == localPlaneRegistryID {
            if case .connected = connectionState { return true }
            return false
        }
        if case .connected = selectedPlane?.connection { return true }
        return false
    }

    func settingsAccess() async throws -> any JetSettingsAccess {
        try await activeClient()
    }

    func recoveryAccess(for planeRegistryID: UUID) async throws -> any JetRecoveryAccess {
        try await client(for: planeRegistryID)
    }

    func requestSettings(_ pane: JetSettingsPane) {
        requestedSettingsPane = pane
    }

    var setupStateIsIdle: Bool {
        if case .idle = setupState { return true }
        return false
    }

    var setupStateIsLoading: Bool {
        if case .loading = setupState { return true }
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

    /// Loads This Mac's setup. It never navigates: New Task shows what is missing
    /// inline. `openWhenIncomplete` is kept for source compatibility.
    func loadSetup(openWhenIncomplete: Bool = false) async {
        guard makeJetClient != nil, !isPreviewSession else { return }
        if setupSnapshot == nil { setupState = .loading }
        do {
            let snapshot = try await client(for: localPlaneRegistryID).setupSnapshot()
            setupRetryTask?.cancel()
            setupRetryTask = nil
            setupRetryAttempt = 0
            setupState = .ready(snapshot)
            updatePlane(localPlaneRegistryID) { plane in
                plane.planeID = snapshot.status.planeID
                plane.snapshot = snapshot
                plane.failure = nil
                plane.conversationCursor = snapshot.status.cursor
            }
            if snapshot.issue(for: .projects) == nil,
               newTaskPlaneRegistryID == localPlaneRegistryID,
               !snapshot.projects.projects.contains(where: { $0.id == selectedProjectID })
            {
                selectedProjectID = snapshot.projects.projects.first?.id
            }
        } catch {
            let failure = presentationError(error)
            setupState = .failed(failure)
            updatePlane(localPlaneRegistryID) { plane in
                plane.failure = failure
            }
            scheduleSetupRetry(after: failure)
        }
        await loadRemotePlaneSnapshots()
    }

    /// Retries a retryable setup failure at 2, 5 and 15 seconds. Reconnecting is
    /// shown only while a retry is really scheduled; afterwards setup has failed.
    func scheduleSetupRetry(after failure: JetPresentationError) {
        setupRetryTask?.cancel()
        setupRetryTask = nil
        guard failure.retryable, setupRetryAttempt < setupRetryLimit else {
            connectionState = .failed(failure)
            return
        }
        setupRetryAttempt += 1
        let delays = Self.setupRetryDelays
        let delay = delays[min(setupRetryAttempt, delays.count) - 1]
        connectionState = .reconnecting(attempt: setupRetryAttempt)
        setupRetryTask = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled, let self else { return }
            self.setupRetryTask = nil
            await self.loadSetup()
            if self.setupSnapshot != nil { await self.loadConversations() }
        }
    }

    /// Refreshes every computer's task list. A refresh never replaces New Task, a
    /// project page, Jet Trash or the open task; selection falls back to New Task
    /// only when the open task disappeared.
    func loadConversations(restoring restoredID: UUID? = nil) async {
        guard makeJetClient != nil, !isPreviewSession else { return }
        conversationRequest += 1
        let request = conversationRequest
        if conversations.isEmpty { conversationFreshness = .loading }
        var targets: [(UUID, JetClient)] = []
        for plane in planes {
            do {
                targets.append((plane.id, try await client(for: plane.id)))
            } catch {
                let failure = presentationError(error)
                updatePlane(plane.id) { $0.failure = failure }
            }
        }

        let loads = await withTaskGroup(
            of: PlaneConversationLoad.self,
            returning: [PlaneConversationLoad].self
        ) { group in
            for (planeID, client) in targets {
                group.addTask {
                    do {
                        return PlaneConversationLoad(
                            planeRegistryID: planeID,
                            result: .success(try await client.conversations())
                        )
                    } catch let failure as JetClientFailure {
                        let presentation: JetPresentationError = switch failure {
                        case let .presentation(error): error
                        case .commandOutcomeUnknown: .invalidResponse
                        }
                        return PlaneConversationLoad(
                            planeRegistryID: planeID,
                            result: .failure(presentation)
                        )
                    } catch {
                        return PlaneConversationLoad(
                            planeRegistryID: planeID,
                            result: .failure(.invalidResponse)
                        )
                    }
                }
            }
            var values: [PlaneConversationLoad] = []
            for await value in group { values.append(value) }
            return values
        }
        guard request == conversationRequest else { return }

        var successfulLoads = 0
        for load in loads {
            switch load.result {
            case let .success(page):
                successfulLoads += 1
                for conversation in planeConversations[load.planeRegistryID, default: []]
                where conversationPlaneRegistryIDs[conversation.id] == load.planeRegistryID {
                    conversationPlaneRegistryIDs.removeValue(forKey: conversation.id)
                }
                planeConversations[load.planeRegistryID] = page.conversations
                planeConversationCursors[load.planeRegistryID] = page.cursor
                if let next = page.nextPage {
                    planeNextConversationPages[load.planeRegistryID] = next
                } else {
                    planeNextConversationPages.removeValue(forKey: load.planeRegistryID)
                }
                for conversation in page.conversations {
                    conversationPlaneRegistryIDs[conversation.id] = load.planeRegistryID
                }
                updatePlane(load.planeRegistryID) { plane in
                    plane.failure = nil
                    plane.conversationCursor = page.cursor
                }
                observeEvents(for: load.planeRegistryID, after: page.cursor)
            case let .failure(failure):
                updatePlane(load.planeRegistryID) { $0.failure = failure }
            }
        }
        rebuildConversationAggregation()
        conversationCursor = planeConversationCursors[localPlaneRegistryID] ?? 0
        nextConversationPage = planeNextConversationPages.values.first
        conversationFreshness = successfulLoads > 0
            ? .live
            : (conversations.isEmpty ? .failed : .cached)

        // New Task, a project page and Jet Trash keep their selection. The list
        // shows load failures itself, so no composer notice is written here.
        guard sidebarSelection == .conversation else { return }
        let previousConversationID = selectedConversationID
        let candidate = restoredID ?? selectedConversationID
        var restoredSnapshot: JetConversationSnapshot?
        if let candidate,
           !conversations.contains(where: { $0.id == candidate }),
           !trashedConversationIDs.contains(candidate)
        {
            for (planeID, client) in targets {
                if let snapshot = try? await client.conversation(candidate) {
                    restoredSnapshot = snapshot
                    planeConversations[planeID, default: []].append(snapshot.conversation)
                    conversationPlaneRegistryIDs[candidate] = planeID
                    rebuildConversationAggregation()
                    break
                }
            }
        }
        // The person may have opened another task while this refresh waited; their
        // choice stands and loads itself.
        guard request == conversationRequest,
              sidebarSelection == .conversation,
              selectedConversationID == previousConversationID
        else { return }
        let resolvedConversationID = Self.resolveSelection(
            candidate: candidate,
            previous: previousConversationID,
            available: conversations.map(\.id),
            isNewTask: false
        )
        guard let resolvedConversationID else {
            if successfulLoads < planes.count,
               let candidate,
               !trashedConversationIDs.contains(candidate)
            {
                // A computer couldn't be reached, so the task may still exist: keep it
                // open with its saved view instead of calling it gone.
                if candidate != previousConversationID { switchConversation(to: candidate) }
                return
            }
            beginNewTask()
            if candidate != nil {
                composerNotice = ComposerNotice(
                    kind: .info,
                    text: String(localized: "That task is no longer available.")
                )
            }
            return
        }
        if resolvedConversationID != previousConversationID {
            switchConversation(to: resolvedConversationID)
        }
        if let restoredSnapshot,
           restoredSnapshot.conversation.id == selectedConversationID
        {
            conversationSnapshot = restoredSnapshot
            recordSelectedSnapshot(restoredSnapshot)
            await loadRunSupervision()
        } else {
            await loadSelectedConversation()
        }
    }

    /// The task a refresh keeps selected: the wanted task, else the one already
    /// open, never another task and never a task while New Task is showing.
    static func resolveSelection(
        candidate: UUID?,
        previous: UUID?,
        available: [UUID],
        isNewTask: Bool
    ) -> UUID? {
        guard !isNewTask else { return nil }
        let available = Set(available)
        if let candidate, available.contains(candidate) { return candidate }
        if let previous, available.contains(previous) { return previous }
        return nil
    }

    func loadMoreConversations() async {
        guard !isPreviewSession,
              !planeNextConversationPages.isEmpty,
              !isLoadingMoreConversations
        else { return }
        isLoadingMoreConversations = true
        let pending = planeNextConversationPages
        let loads = await withTaskGroup(
            of: PlaneConversationPageLoad.self,
            returning: [PlaneConversationPageLoad].self
        ) { group in
            for (planeID, cursor) in pending {
                do {
                    let client = try await client(for: planeID)
                    group.addTask {
                        do {
                            return PlaneConversationPageLoad(
                                planeRegistryID: planeID,
                                result: .success(try await client.nextConversations(cursor))
                            )
                        } catch let failure as JetClientFailure {
                            let presentation: JetPresentationError = switch failure {
                            case let .presentation(error): error
                            case .commandOutcomeUnknown: .invalidResponse
                            }
                            return PlaneConversationPageLoad(
                                planeRegistryID: planeID,
                                result: .failure(presentation)
                            )
                        } catch {
                            return PlaneConversationPageLoad(
                                planeRegistryID: planeID,
                                result: .failure(.invalidResponse)
                            )
                        }
                    }
                } catch {
                    updatePlane(planeID) { $0.failure = presentationError(error) }
                }
            }
            var values: [PlaneConversationPageLoad] = []
            for await value in group { values.append(value) }
            return values
        }
        var requiresSnapshot = false
        for load in loads {
            switch load.result {
            case let .success(page):
                let known = Set(planeConversations[load.planeRegistryID, default: []].map(\.id))
                let additions = page.conversations.filter { !known.contains($0.id) }
                planeConversations[load.planeRegistryID, default: []].append(contentsOf: additions)
                for conversation in additions {
                    conversationPlaneRegistryIDs[conversation.id] = load.planeRegistryID
                }
                if let next = page.nextPage {
                    planeNextConversationPages[load.planeRegistryID] = next
                } else {
                    planeNextConversationPages.removeValue(forKey: load.planeRegistryID)
                }
                updatePlane(load.planeRegistryID) { $0.failure = nil }
            case let .failure(failure):
                updatePlane(load.planeRegistryID) { $0.failure = failure }
                requiresSnapshot = requiresSnapshot || failure.restart?.requiresPaginationSnapshot == true
            }
        }
        rebuildConversationAggregation()
        nextConversationPage = planeNextConversationPages.values.first
        if requiresSnapshot { await loadConversations() }
        isLoadingMoreConversations = false
    }

    /// Searches every computer. Failures stay with the results; they never write
    /// the composer notice.
    func searchConversations() async {
        guard !isPreviewSession else { return }
        searchRequest += 1
        let request = searchRequest
        let text = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            searchResult = nil
            searchIsLoading = false
            return
        }
        searchIsLoading = true
        var targets: [(UUID, String, JetClient)] = []
        for plane in planes {
            do {
                targets.append((plane.id, plane.name, try await client(for: plane.id)))
            } catch {
                updatePlane(plane.id) { $0.failure = presentationError(error) }
            }
        }
        let loads = await withTaskGroup(
            of: PlaneSearchLoad.self,
            returning: [PlaneSearchLoad].self
        ) { group in
            for (planeID, planeName, client) in targets {
                group.addTask {
                    do {
                        return PlaneSearchLoad(
                            planeRegistryID: planeID,
                            planeName: planeName,
                            result: .success(try await client.searchConversations(text))
                        )
                    } catch let failure as JetClientFailure {
                        let presentation: JetPresentationError = switch failure {
                        case let .presentation(error): error
                        case .commandOutcomeUnknown: .invalidResponse
                        }
                        return PlaneSearchLoad(
                            planeRegistryID: planeID,
                            planeName: planeName,
                            result: .failure(presentation)
                        )
                    } catch {
                        return PlaneSearchLoad(
                            planeRegistryID: planeID,
                            planeName: planeName,
                            result: .failure(.invalidResponse)
                        )
                    }
                }
            }
            var values: [PlaneSearchLoad] = []
            for await value in group { values.append(value) }
            return values
        }
        guard request == searchRequest else { return }
        var hits: [JetFederatedSearchHit] = []
        var cursors: [UUID: UInt64] = [:]
        var indexedThrough: [UUID: UInt64] = [:]
        var failures: [UUID: JetPresentationError] = [:]
        for load in loads.sorted(by: { $0.planeName < $1.planeName }) {
            switch load.result {
            case let .success(result):
                cursors[load.planeRegistryID] = result.cursor
                indexedThrough[load.planeRegistryID] = result.indexedThrough
                hits.append(contentsOf: result.hits.map {
                    JetFederatedSearchHit(
                        planeRegistryID: load.planeRegistryID,
                        planeName: load.planeName,
                        hit: $0
                    )
                })
                for hit in result.hits {
                    conversationPlaneRegistryIDs[hit.conversationID] = load.planeRegistryID
                }
                updatePlane(load.planeRegistryID) { $0.failure = nil }
            case let .failure(failure):
                failures[load.planeRegistryID] = failure
                updatePlane(load.planeRegistryID) { $0.failure = failure }
            }
        }
        searchResult = JetFederatedSearchResult(
            hits: hits,
            cursors: cursors,
            indexedThrough: indexedThrough,
            failures: failures
        )
        if request == searchRequest { searchIsLoading = false }
    }

    /// Opens a task. State switches at once, showing the cached transcript; the
    /// snapshot load is debounced by 150 ms so arrowing through rows stays calm.
    func selectConversation(_ conversationID: UUID) {
        if selectedConversationID != conversationID {
            switchConversation(to: conversationID)
        }
        sidebarSelection = .conversation
        composerNotice = nil
        markSelectedRepliesSeen()
        statusStore.noteSelected(conversationID)
        scheduleSelectedConversationLoad()
    }

    /// Replaces the open task's state with another task's, keeping the previous
    /// transcript in the cache.
    func switchConversation(to conversationID: UUID) {
        stashSelectedTranscript()
        detachCurrentTerminal()
        selectionLoadTask?.cancel()
        // A history replay belongs to the task being left.
        transcripts.cancelReplay()
        conversationSnapshot = nil
        turnQueue = nil
        runExecution = nil
        resetWorkPanel()
        selectedConversationID = conversationID
        timeline = transcripts.entries(for: conversationID)
        interruptThenReply = false
        composerPlaceholderOverride = nil
    }

    /// Leaves the open task for New Task, a project page or Jet Trash.
    func leaveConversation() {
        stashSelectedTranscript()
        detachCurrentTerminal()
        selectionLoadTask?.cancel()
        transcripts.cancelReplay()
        selectedConversationID = nil
        conversationSnapshot = nil
        turnQueue = nil
        runExecution = nil
        timeline = []
        resetWorkPanel()
        interruptThenReply = false
        composerPlaceholderOverride = nil
    }

    /// Saves the open task's timeline into the transcript cache and marks the
    /// replies the person saw there.
    func stashSelectedTranscript() {
        guard let selectedConversationID else { return }
        markSelectedRepliesSeen()
        transcripts.save(timeline, for: selectedConversationID)
    }

    /// The open task is read: its newest reply counts as seen. This runs when a task
    /// is opened and left, not per Event, so streaming never writes preferences.
    func markSelectedRepliesSeen() {
        guard let selectedConversationID else { return }
        let latestReply = max(
            statusStore.facts[selectedConversationID]?.lastReplySequence ?? 0,
            timeline.last { $0.kind == .agent }?.sequence ?? 0
        )
        if latestReply > 0 { memory.markSeen(selectedConversationID, sequence: latestReply) }
    }

    func scheduleSelectedConversationLoad() {
        selectionGeneration += 1
        let generation = selectionGeneration
        selectionLoadTask?.cancel()
        selectionLoadTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(150))
            guard !Task.isCancelled, let self, self.selectionGeneration == generation else { return }
            await self.loadSelectedConversation()
        }
    }

    func selectSearchHit(_ hit: JetFederatedSearchHit) {
        conversationPlaneRegistryIDs[hit.hit.conversationID] = hit.planeRegistryID
        selectConversation(hit.hit.conversationID)
    }

    func loadSelectedConversation() async {
        guard usesLivePlane, !isPreviewSession,
              let selectedConversationID
        else { return }
        conversationRefreshes += 1
        isRefreshingConversation = true
        defer {
            conversationRefreshes -= 1
            isRefreshingConversation = conversationRefreshes > 0
        }
        do {
            let snapshot = try await activeClient().conversation(selectedConversationID)
            guard self.selectedConversationID == selectedConversationID else { return }
            conversationSnapshot = snapshot
            mergeConversation(snapshot.conversation)
            conversationFreshness = .live
            recordSelectedSnapshot(snapshot)
            await loadRunSupervision()
        } catch {
            guard self.selectedConversationID == selectedConversationID else { return }
            let failure = presentationError(error)
            // A cancelled load says nothing about the task: its view stays as it was.
            guard !Task.isCancelled, failure.category != .cancelled else { return }
            conversationFreshness = conversationSnapshot == nil ? .failed : .cached
            // Offline is shown by the banner and the send blocker, not a notice.
            if failure.category != .offline {
                composerNotice = ComposerNotice(
                    kind: .warning,
                    text: String(localized: "Jet couldn't refresh this task."),
                    action: .tryAgainConnection
                )
            }
        }
    }

    /// Feeds a fresh snapshot of the open task to the status and transcript stores.
    func recordSelectedSnapshot(_ snapshot: JetConversationSnapshot) {
        let execution = runExecution?.run.conversationID == snapshot.conversation.id
            ? runExecution
            : nil
        statusStore.record(
            snapshot: snapshot,
            execution: execution,
            cursor: max(snapshot.cursor, execution?.cursor ?? 0)
        )
        let planeRegistryID = conversationPlaneRegistryIDs[snapshot.conversation.id]
            ?? selectedPlaneRegistryID
        transcripts.didLoad(
            snapshot: snapshot,
            planeRegistryID: planeRegistryID,
            headCursor: planeConversationCursors[planeRegistryID] ?? snapshot.cursor
        )
    }

    func loadRunSupervision() async {
        guard !isPreviewSession else { return }
        guard usesLivePlane, let conversationID = selectedConversationID else {
            turnQueue = nil
            runExecution = nil
            return
        }
        // A background refresh never sets `supervisionOperation`, so it can't
        // block Interrupt, Stop Assistant or Remove.
        do {
            let client = try await activeClient()
            async let queue = client.turnQueue(conversationID: conversationID)
            if let runID = conversationSnapshot?.runs.last?.id {
                async let execution = client.runExecution(runID: runID)
                let (nextQueue, nextExecution) = try await (queue, execution)
                guard selectedConversationID == conversationID else { return }
                guard nextExecution.run.conversationID == conversationID else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                turnQueue = nextQueue
                runExecution = nextExecution
            } else {
                let nextQueue = try await queue
                guard selectedConversationID == conversationID else { return }
                turnQueue = nextQueue
                runExecution = nil
            }
            if let snapshot = conversationSnapshot, snapshot.conversation.id == conversationID {
                recordSelectedSnapshot(snapshot)
            }
        } catch {
            guard selectedConversationID == conversationID else { return }
            let failure = presentationError(error)
            // A cancelled refresh is not a failure; whoever cancelled it moves on.
            guard !Task.isCancelled, failure.category != .cancelled else { return }
            if failure.category != .offline {
                composerNotice = ComposerNotice(
                    kind: .warning,
                    text: String(localized: "Jet couldn't refresh this task."),
                    action: .tryAgainConnection
                )
            }
        }
        if selectedConversationID == conversationID, selectedRun != nil {
            await loadWorkPanel()
        } else if selectedConversationID == conversationID {
            resetWorkPanel()
        }
    }

    func loadWorkPanel(preserveContinuity: Bool = true) async {
        guard !isPreviewSession else {
            keepsRequestedCheckpoint = false
            return
        }
        guard usesLivePlane,
              let conversationID = selectedConversationID,
              let run = selectedRun
        else {
            keepsRequestedCheckpoint = false
            resetWorkPanel()
            return
        }
        workRequest += 1
        let request = workRequest
        workOperation = "refresh"
        workError = nil
        workNotice = nil
        workNoticeError = nil
        let previousRunID = workDiff?.runID
        // Another Run starts at All Changes unless `showChanges` asked for a scope.
        if previousRunID != run.id, !keepsRequestedCheckpoint {
            checkpointKind = .current
        }
        keepsRequestedCheckpoint = false
        let requestedScope = selectedChangeScope
        let previousTarget = workTarget
        let previousSelectedPath = preserveContinuity
            && previousRunID == run.id
            && workDiff?.scope == requestedScope
            ? selectedWorkFilePath
            : nil
        let targetFileCount = preserveContinuity
            && previousRunID == run.id
            && workDiff?.scope == requestedScope
            ? workFiles.count
            : 0
        do {
            let client = try await activeClient()
            let diff = try await client.changeDiff(runID: run.id, scope: requestedScope)
            guard request == workRequest,
                  selectedConversationID == conversationID,
                  selectedRun?.id == run.id,
                  diff.runID == run.id
            else { return }

            let target: JetFileTarget?
            if let workspaceID = diff.workspaceID {
                target = .workspace(workspaceID)
            } else if let projectID = selectedConversation?.projectID {
                target = .project(projectID)
            } else {
                target = nil
            }
            let preview = Data(diff.patch.utf8)
            guard UInt64(preview.count) <= diff.artifact.size else {
                throw JetClientFailure.presentation(.invalidResponse)
            }

            var refreshedFiles = diff.files
            var refreshedNextPage = diff.nextPage
            while WorkRefreshContinuity.shouldLoadNextPage(
                loadedCount: refreshedFiles.count,
                targetCount: targetFileCount,
                selectedPath: previousSelectedPath,
                loadedPaths: Set(refreshedFiles.map(\.path)),
                hasNextPage: refreshedNextPage != nil
            ), let pageCursor = refreshedNextPage {
                let page = try await client.nextChangeDiff(pageCursor)
                guard page.runID == run.id, page.scope == requestedScope else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                let known = Set(refreshedFiles.map(\.path))
                refreshedFiles.append(contentsOf: page.files.filter { !known.contains($0.path) })
                refreshedNextPage = page.nextPage
            }

            workDiff = diff
            workFiles = refreshedFiles
            workNextPage = refreshedNextPage
            workPatch = diff.patch
            workArtifactBytes = preview
            workTarget = target
            if previousTarget != target
                || !workFiles.contains(where: { $0.path == selectedWorkFilePath })
            {
                selectedWorkFilePath = workFiles.first?.path
                editableFile = nil
                fileDraft = ""
            }

            if let workspaceID = diff.workspaceID {
                do {
                    let terminals = try await client.workspaceTerminals(workspaceID: workspaceID)
                    guard request == workRequest else { return }
                    workTerminals = terminals
                    if !terminals.contains(where: { $0.id == selectedTerminalID }) {
                        selectedTerminalID = terminals.first(where: { $0.state == .open })?.id
                            ?? terminals.first?.id
                    }
                    if let terminalStreamTerminalID,
                       !terminals.contains(where: { $0.id == terminalStreamTerminalID })
                    {
                        detachCurrentTerminal()
                    }
                } catch {
                    workTerminals = []
                    workNotice = String(localized: "Changes loaded, but terminals aren't available right now.")
                }
            } else {
                workTerminals = []
                selectedTerminalID = nil
                detachCurrentTerminal()
            }
        } catch {
            guard request == workRequest else { return }
            let failure = presentationError(error)
            workError = failure
            applyRevisionConflict(failure)
        }
        if request == workRequest {
            workOperation = nil
            await loadGitDeliveries()
        }
    }

    func loadGitDeliveries(startObservation: Bool = true) async {
        guard !isPreviewSession else { return }
        guard usesLivePlane, let conversationID = selectedConversationID else {
            gitDeliveries = []
            gitDeliveryError = nil
            return
        }
        gitDeliveryOperation = "refresh"
        gitDeliveryError = nil
        do {
            let deliveries = try await activeClient().gitDeliveries(
                conversationID: conversationID
            )
            statusStore.recordGitDeliveries(deliveries, conversationID: conversationID)
            guard selectedConversationID == conversationID else { return }
            gitDeliveries = deliveries
        } catch {
            guard selectedConversationID == conversationID else { return }
            gitDeliveryError = presentationError(error)
        }
        if selectedConversationID == conversationID {
            gitDeliveryOperation = nil
            if startObservation { startGitDeliveryObservationIfNeeded() }
        }
    }

    func showGitDelivery(_ choice: JetGitDeliveryChoice) {
        gitDeliveryChoice = choice
        selectedWorkPanel = .changes
        isWorkPanelPresented = true
        workScrollAnchors[.changes] = "delivery-heading"
    }

    func prepareGitDelivery() {
        gitDeliveryNotice = nil
        gitDeliveryError = nil
        guard canPrepareGitDelivery else {
            gitDeliveryNotice = gitDeliveryUnavailableReason
            return
        }
        do {
            gitDeliveryConfirmation = try makeGitDeliveryRequest()
        } catch let error as JetPresentationError {
            gitDeliveryError = error
            gitDeliveryNotice = plainMessage(for: error)
        } catch {
            gitDeliveryError = .invalidResponse
            gitDeliveryNotice = plainMessage(for: .invalidResponse)
        }
    }

    func cancelGitDeliveryConfirmation() {
        gitDeliveryConfirmation = nil
    }

    func confirmGitDelivery() async {
        guard let request = gitDeliveryConfirmation else { return }
        gitDeliveryConfirmation = nil
        if pendingGitDelivery?.request != request {
            pendingGitDelivery = PendingGitDelivery(request: request, commandID: UUID())
        }
        await submitPendingGitDelivery()
    }

    func retryGitDeliveryAdmission() async {
        guard pendingGitDelivery != nil else { return }
        await submitPendingGitDelivery()
    }

    func reviewRetry(_ delivery: JetGitDelivery) {
        guard delivery.canRetry else { return }
        gitDeliveryConfirmation = JetGitDeliveryRequest(
            conversationID: delivery.conversationID,
            checkpoint: delivery.checkpoint,
            operation: delivery.operation
        )
    }

    func reviewGitDeliveryAcknowledgement(_ delivery: JetGitDelivery) {
        guard delivery.needsAcknowledgement else { return }
        gitDeliveryAcknowledgementConfirmation = delivery
    }

    func cancelGitDeliveryAcknowledgement() {
        gitDeliveryAcknowledgementConfirmation = nil
    }

    func confirmGitDeliveryAcknowledgement() async {
        guard let delivery = gitDeliveryAcknowledgementConfirmation,
              delivery.needsAcknowledgement
        else { return }
        gitDeliveryAcknowledgementConfirmation = nil
        gitDeliveryOperation = "acknowledge"
        gitDeliveryNotice = nil
        gitDeliveryError = nil
        let commandID = gitDeliveryAcknowledgementCommandIDs[delivery.id] ?? UUID()
        gitDeliveryAcknowledgementCommandIDs[delivery.id] = commandID
        do {
            _ = try await activeClient().acknowledgeGitDelivery(
                deliveryID: delivery.id,
                commandID: commandID
            )
            gitDeliveryAcknowledgementCommandIDs[delivery.id] = nil
            gitDeliveryNotice = String(localized: "Marked as checked. Jet didn't repeat or undo the Git step.")
            await loadGitDeliveries()
        } catch {
            gitDeliveryError = presentationError(error)
            gitDeliveryNotice = gitDeliveryError.map(plainMessage(for:))
        }
        gitDeliveryOperation = nil
    }

    func refreshNotificationAuthorization() async {
        guard let notifications else { return }
        notificationAuthorization = await notifications.authorizationStatus()
    }

    func requestNotificationAuthorization() async -> Bool {
        guard let notifications else { return false }
        notificationError = nil
        do {
            notificationAuthorization = try await notifications.requestAuthorization()
            if notificationAuthorization == .denied {
                notificationError = String(localized: "Notifications are turned off for Jet in System Settings.")
            }
        } catch {
            notificationError = String(localized: "Jet couldn't update notification permission.")
        }
        return notificationAuthorization.permitsDelivery
    }

    func makeGitDeliveryRequest() throws -> JetGitDeliveryRequest {
        guard let conversationID = selectedConversationID, let diff = workDiff else {
            throw JetPresentationError.invalidInput(
                code: "git.checkpoint_unavailable",
                message: String(localized: "Load the changes before keeping them.")
            )
        }
        let operation: JetGitOperation
        let checkpoint: JetGitCheckpoint?
        switch gitDeliveryChoice {
        case .branch:
            operation = .branch(name: try validatedGitInput(gitBranchName, label: "branch"))
            checkpoint = nil
        case .commit:
            guard diff.latestTurn > 0 else {
                throw JetPresentationError.invalidInput(
                    code: "git.checkpoint_unavailable",
                    message: String(localized: "No finished reply has changes to commit yet.")
                )
            }
            operation = .commit
            checkpoint = JetGitCheckpoint(runID: diff.runID, turn: diff.latestTurn)
        case .push:
            operation = .push(remote: try validatedGitInput(gitRemoteName, label: "remote"))
            checkpoint = nil
        case .draftPullRequest:
            guard diff.latestTurn > 0 else {
                throw JetPresentationError.invalidInput(
                    code: "git.checkpoint_unavailable",
                    message: String(localized: "No finished reply has changes for a pull request yet.")
                )
            }
            let remote = try validatedGitInput(gitRemoteName, label: "remote")
            let base = gitBaseBranch.isEmpty
                ? nil
                : try validatedGitInput(gitBaseBranch, label: "base branch")
            operation = .draftPullRequest(remote: remote, base: base)
            checkpoint = JetGitCheckpoint(runID: diff.runID, turn: diff.latestTurn)
        }
        return JetGitDeliveryRequest(
            conversationID: conversationID,
            checkpoint: checkpoint,
            operation: operation
        )
    }

    func validatedGitInput(_ value: String, label: String) throws -> String {
        guard !value.isEmpty,
              value.utf8.count <= 255,
              !value.unicodeScalars.contains(where: CharacterSet.whitespacesAndNewlines.contains),
              !value.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        else {
            let message = switch label {
            case "branch": String(localized: "Enter a branch name without spaces, up to 255 bytes.")
            case "remote": String(localized: "Enter a remote name without spaces, up to 255 bytes.")
            default: String(localized: "Enter a base branch without spaces, up to 255 bytes.")
            }
            throw JetPresentationError.invalidInput(
                code: "git.\(label.replacingOccurrences(of: " ", with: "_"))_invalid",
                message: message
            )
        }
        return value
    }

    func submitPendingGitDelivery() async {
        guard let pendingGitDelivery else { return }
        gitDeliveryOperation = "submit"
        gitDeliveryNotice = nil
        gitDeliveryError = nil
        do {
            _ = try await activeClient().deliverGit(
                pendingGitDelivery.request,
                commandID: pendingGitDelivery.commandID
            )
            self.pendingGitDelivery = nil
            gitDeliveryAdmissionUncertain = nil
            gitDeliveryNotice = String(localized: "Jet started this Git step. Its result appears below.")
            await loadGitDeliveries()
        } catch let failure as JetClientFailure {
            guard case .commandOutcomeUnknown = failure else {
                self.pendingGitDelivery = nil
                gitDeliveryError = presentationError(failure)
                gitDeliveryNotice = gitDeliveryError.map(plainMessage(for:))
                gitDeliveryOperation = nil
                return
            }
            gitDeliveryAdmissionUncertain = pendingGitDelivery.request
            gitDeliveryError = presentationError(failure)
            gitDeliveryNotice = String(localized: "Jet couldn't confirm it received the request. Check the status before sending the same request again.")
        } catch {
            self.pendingGitDelivery = nil
            gitDeliveryError = presentationError(error)
            gitDeliveryNotice = gitDeliveryError.map(plainMessage(for:))
        }
        gitDeliveryOperation = nil
    }

    func startGitDeliveryObservationIfNeeded() {
        gitDeliveryObservationTask?.cancel()
        guard gitDeliveries.contains(where: { $0.outcome == .pending }),
              let conversationID = selectedConversationID
        else {
            gitDeliveryObservationTask = nil
            return
        }
        gitDeliveryObservationTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await ContinuousClock().sleep(for: .seconds(2))
                guard !Task.isCancelled, self?.selectedConversationID == conversationID else {
                    return
                }
                await self?.loadGitDeliveries(startObservation: false)
                guard self?.gitDeliveries.contains(where: { $0.outcome == .pending }) == true else {
                    return
                }
            }
        }
    }

    func applyWorkCheckpoint() async {
        guard canApplyWorkCheckpoint else { return }
        selectedWorkFilePath = nil
        editableFile = nil
        fileDraft = ""
        await loadWorkPanel(preserveContinuity: false)
    }

    func applyWorkRecovery(_ action: JetRecoveryAction) async {
        workNoticeError = nil
        switch action {
        case .refreshFile:
            if let selectedWorkFilePath {
                await selectWorkFile(selectedWorkFilePath)
            }
        case let .refreshConversation(conversationID):
            guard conversationID == selectedConversationID else { return }
            await loadSelectedConversation()
        case let .refreshRun(runID):
            guard runID == selectedRun?.id else { return }
            await loadRunSupervision()
        case let .resumeEvents(after):
            observeEvents(for: selectedPlaneRegistryID, after: after)
            workNotice = String(localized: "Activity reconnected.")
        }
    }

    func loadMoreWorkFiles() async {
        guard workOperation == nil,
              let cursor = workNextPage,
              let runID = workDiff?.runID
        else { return }
        workOperation = "files"
        workNotice = nil
        workNoticeError = nil
        do {
            let page = try await activeClient().nextChangeDiff(cursor)
            guard page.runID == runID, page.scope == workDiff?.scope else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            let known = Set(workFiles.map(\.path))
            workFiles.append(contentsOf: page.files.filter { !known.contains($0.path) })
            workNextPage = page.nextPage
        } catch {
            let failure = presentationError(error)
            workNotice = plainMessage(for: failure)
            workNoticeError = failure
            applyRevisionConflict(failure)
            if failure.restart?.requiresPaginationSnapshot == true {
                await loadWorkPanel()
            }
        }
        workOperation = nil
    }

    func loadMorePatch() async {
        guard workOperation == nil,
              let diff = workDiff,
              diff.patchTruncated,
              diff.artifact.availability == .stored,
              UInt64(workArtifactBytes.count) < diff.artifact.size
        else { return }
        workOperation = "patch"
        workNotice = nil
        workNoticeError = nil
        do {
            let chunk = try await activeClient().changeArtifact(
                sha256: diff.artifact.sha256,
                offset: UInt64(workArtifactBytes.count)
            )
            guard chunk.artifact == diff.artifact,
                  chunk.offset == UInt64(workArtifactBytes.count),
                  !chunk.bytes.isEmpty,
                  UInt64(workArtifactBytes.count + chunk.bytes.count) <= diff.artifact.size
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            workArtifactBytes.append(chunk.bytes)
            workPatch = String(decoding: workArtifactBytes, as: UTF8.self)
            if UInt64(workArtifactBytes.count) == diff.artifact.size {
                let digest = SHA256.hash(data: workArtifactBytes)
                    .map { String(format: "%02x", $0) }
                    .joined()
                guard digest == diff.artifact.sha256 else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                workNotice = String(localized: "All changes loaded.")
            }
        } catch {
            recordWorkFailure(error)
        }
        workOperation = nil
    }

    func selectWorkFile(_ path: String) async {
        guard workFiles.contains(where: { $0.path == path }),
              let target = workTarget
        else { return }
        workFileRequest += 1
        let request = workFileRequest
        selectedWorkFilePath = path
        selectedWorkPanel = .files
        workOperation = "file"
        workNotice = nil
        workNoticeError = nil
        editableFile = nil
        fileDraft = ""
        do {
            let file = try await activeClient().editableFile(target: target, path: path)
            guard request == workFileRequest,
                  selectedWorkFilePath == path,
                  file.target == target,
                  file.path == path
            else { return }
            editableFile = file
            fileDraft = file.content ?? ""
            pendingWorkEdit = nil
        } catch {
            if request == workFileRequest { recordWorkFailure(error) }
        }
        if request == workFileRequest { workOperation = nil }
    }

    func saveSelectedWorkFile() async {
        guard workOperation == nil,
              let target = workTarget,
              let file = editableFile,
              file.path == selectedWorkFilePath
        else { return }
        workOperation = "save"
        workNotice = nil
        workNoticeError = nil
        let pending: PendingWorkEdit
        if let existing = pendingWorkEdit,
           existing.path == file.path,
           existing.content == fileDraft
        {
            pending = existing
        } else {
            pending = PendingWorkEdit(path: file.path, content: fileDraft, commandID: UUID())
            pendingWorkEdit = pending
        }
        do {
            let revision = try await activeClient().applyUserEdit(
                target: target,
                path: file.path,
                expectedRevision: file.revision,
                content: fileDraft,
                commandID: pending.commandID
            )
            editableFile = JetEditableFile(
                cursor: file.cursor,
                target: file.target,
                path: file.path,
                content: fileDraft,
                revision: revision
            )
            pendingWorkEdit = nil
            workNotice = String(localized: "Saved.")
        } catch {
            recordWorkFailure(error)
        }
        workOperation = nil
    }

    func submitSelectedReview() async {
        let trimmed = reviewComment.trimmingCharacters(in: .whitespacesAndNewlines)
        guard workOperation == nil,
              let conversationID = selectedConversationID,
              let path = selectedWorkFilePath,
              reviewLine > 0,
              !trimmed.isEmpty
        else { return }
        workOperation = "review"
        workNotice = nil
        workNoticeError = nil
        let pending: PendingReview
        if let existing = pendingReview,
           existing.path == path,
           existing.line == reviewLine,
           existing.comment == trimmed
        {
            pending = existing
        } else {
            pending = PendingReview(
                path: path,
                line: reviewLine,
                comment: trimmed,
                commandID: UUID()
            )
            pendingReview = pending
        }
        do {
            _ = try await activeClient().submitReview(
                conversationID: conversationID,
                path: path,
                line: reviewLine,
                comment: trimmed,
                commandID: pending.commandID
            )
            pendingReview = nil
            reviewComment = ""
            workNotice = String(localized: "Comment sent as a message.")
            await loadRunSupervision()
        } catch {
            recordWorkFailure(error)
        }
        workOperation = nil
    }

    func createWorkspaceTerminal() async {
        guard workOperation == nil, let workspaceID = workDiff?.workspaceID else { return }
        workOperation = "open-terminal"
        workNotice = nil
        workNoticeError = nil
        do {
            let terminal = try await activeClient().openTerminal(
                workspaceID: workspaceID,
                rows: terminalRows,
                columns: terminalColumns,
                commandID: terminalOpenCommandID
            )
            terminalOpenCommandID = UUID()
            workTerminals.removeAll { $0.id == terminal.id }
            workTerminals.append(terminal)
            selectedTerminalID = terminal.id
            await attachSelectedTerminal()
        } catch {
            recordWorkFailure(error)
        }
        workOperation = nil
    }

    func attachSelectedTerminal() async {
        guard let terminalID = selectedTerminalID,
              workTerminals.contains(where: { $0.id == terminalID && $0.state == .open })
        else { return }
        detachCurrentTerminal()
        workNotice = nil
        workNoticeError = nil
        do {
            let client = try await activeClient()
            let stream = try await client.attachTerminal(
                terminalID: terminalID,
                after: terminalOffsets[terminalID] ?? 0
            )
            terminalStreamTerminalID = terminalID
            terminalObservationTask = Task { [weak self] in
                do {
                    for try await event in stream {
                        guard !Task.isCancelled else { return }
                        self?.receiveTerminal(event, terminalID: terminalID)
                    }
                } catch {
                    guard !Task.isCancelled else { return }
                    if let self {
                        self.attachedTerminalID = nil
                        self.terminalStreamTerminalID = nil
                        self.recordWorkFailure(error)
                    }
                }
            }
        } catch {
            recordWorkFailure(error)
        }
    }

    func detachSelectedTerminal() {
        detachCurrentTerminal()
    }

    func closeSelectedTerminal() async {
        guard workOperation == nil, let terminalID = selectedTerminalID else { return }
        workOperation = "close-terminal"
        workNotice = nil
        workNoticeError = nil
        if terminalStreamTerminalID == terminalID { detachCurrentTerminal() }
        let commandID = terminalCloseCommandIDs[terminalID] ?? UUID()
        terminalCloseCommandIDs[terminalID] = commandID
        do {
            let terminal = try await activeClient().closeTerminal(
                terminalID: terminalID,
                commandID: commandID
            )
            terminalCloseCommandIDs.removeValue(forKey: terminalID)
            if let index = workTerminals.firstIndex(where: { $0.id == terminalID }) {
                workTerminals[index] = terminal
            }
            workNotice = String(localized: "Terminal closed.")
        } catch {
            recordWorkFailure(error)
        }
        workOperation = nil
    }

    func sendTerminalLine() async {
        guard let terminalID = attachedTerminalID, !terminalInput.isEmpty else { return }
        let input = terminalInput + "\n"
        terminalInput = ""
        do {
            try await activeClient().sendTerminalInput(
                terminalID: terminalID,
                bytes: Data(input.utf8)
            )
        } catch {
            terminalInput = String(input.dropLast())
            recordWorkFailure(error)
        }
    }

    func setTerminalGeometry(width: Double, height: Double) {
        let rows = UInt16(clamping: max(1, min(1000, Int(height / 16))))
        let columns = UInt16(clamping: max(1, min(1000, Int(width / 7.2))))
        guard rows != terminalRows || columns != terminalColumns else { return }
        terminalRows = rows
        terminalColumns = columns
        Task { await resizeAttachedTerminal() }
    }

    func resizeAttachedTerminal() async {
        guard let terminalID = attachedTerminalID else { return }
        if let lastTerminalSize,
           lastTerminalSize.terminalID == terminalID,
           lastTerminalSize.rows == terminalRows,
           lastTerminalSize.columns == terminalColumns
        {
            return
        }
        do {
            try await activeClient().resizeTerminal(
                terminalID: terminalID,
                rows: terminalRows,
                columns: terminalColumns
            )
            lastTerminalSize = (terminalID, terminalRows, terminalColumns)
        } catch {
            recordWorkFailure(error)
        }
    }

    func receiveTerminal(_ event: JetTerminalEvent, terminalID: UUID) {
        guard terminalStreamTerminalID == terminalID else { return }
        switch event {
        case .attached:
            attachedTerminalID = terminalID
            Task { await resizeAttachedTerminal() }
        case let .output(offset, bytes):
            terminalOffsets[terminalID] = offset + UInt64(bytes.count)
            var decoder = terminalDecoders[terminalID] ?? TerminalTranscriptDecoder()
            let text = decoder.decode(bytes)
            terminalDecoders[terminalID] = decoder
            terminalOutput[terminalID] = String(
                ((terminalOutput[terminalID] ?? "") + text).suffix(1_048_576)
            )
        case let .gap(firstMissingOffset, missingBytes):
            terminalOffsets[terminalID] = firstMissingOffset + missingBytes
            terminalDecoders[terminalID] = TerminalTranscriptDecoder()
            let text = "\n[\(missingBytes) earlier bytes unavailable]\n"
            terminalOutput[terminalID] = String(
                ((terminalOutput[terminalID] ?? "") + text).suffix(1_048_576)
            )
        case .resized:
            break
        case let .finished(totalBytes):
            terminalOffsets[terminalID] = totalBytes
            if var decoder = terminalDecoders[terminalID] {
                let tail = decoder.decode(Data(), final: true)
                terminalDecoders[terminalID] = decoder
                terminalOutput[terminalID] = String(
                    ((terminalOutput[terminalID] ?? "") + tail).suffix(1_048_576)
                )
            }
            attachedTerminalID = nil
            terminalStreamTerminalID = nil
            workNotice = String(localized: "Terminal session ended.")
        }
    }

    /// Removes a waiting message and puts its text back into an empty composer.
    func withdrawTurn(_ turn: JetTurnQueueEntry) async {
        await removeQueuedMessage(turn)
    }

    /// Sends `withdraw_turn` with a retained Command ID. Returns true once the core
    /// accepted it.
    @discardableResult
    func performWithdrawal(_ turn: JetTurnQueueEntry) async -> Bool {
        guard supervisionOperation == nil,
              turn.withdrawable,
              turn.state == .queued,
              let conversationID = selectedConversationID
        else {
            return false
        }
        supervisionOperation = "withdraw"
        defer { supervisionOperation = nil }
        let commandID = withdrawalCommandIDs[turn.id] ?? UUID()
        withdrawalCommandIDs[turn.id] = commandID
        do {
            _ = try await activeClient().withdrawTurn(
                conversationID: conversationID,
                turnID: turn.id,
                commandID: commandID
            )
            withdrawalCommandIDs.removeValue(forKey: turn.id)
            await loadRunSupervision()
            return true
        } catch {
            composerNotice = failureNotice(for: error)
            return false
        }
    }

    /// Asks for confirmation of Interrupt or Stop Assistant. Neither opens Details.
    func requestRunControl(_ control: JetRunControl) {
        guard (control == .interruptTurn ? canInterruptTurn : canStopRun) else { return }
        runControlConfirmation = control
    }

    func cancelRunControl() {
        runControlConfirmation = nil
        interruptThenReply = false
    }

    func confirmRunControl() async {
        guard supervisionOperation == nil,
              let control = runControlConfirmation,
              let runID = selectedRun?.id
        else {
            return
        }
        supervisionOperation = control.rawValue
        let key = RunControlKey(runID: runID, control: control)
        let commandID = runControlCommandIDs[key] ?? UUID()
        runControlCommandIDs[key] = commandID
        do {
            _ = try await activeClient().controlRun(
                runID: runID,
                control: control,
                commandID: commandID
            )
            runControlCommandIDs.removeValue(forKey: key)
            runControlConfirmation = nil
            if control == .interruptTurn {
                composerNotice = ComposerNotice(kind: .info, text: String(localized: "Interrupting…"))
                if interruptThenReply {
                    composerPlaceholderOverride = selectedAssistantName.map {
                        String(localized: "Tell \($0) what to do instead…")
                    } ?? String(localized: "Tell the assistant what to do instead…")
                }
                interruptThenReply = false
                composerFocusRequest += 1
            } else {
                composerNotice = ComposerNotice(
                    kind: .info,
                    text: selectedAssistantName.map { String(localized: "Stopping \($0)…") }
                        ?? String(localized: "Stopping the assistant…")
                )
            }
            await loadRunSupervision()
        } catch {
            composerNotice = failureNotice(for: error)
        }
        supervisionOperation = nil
    }

    func authorizeApprovalRetry(_ approval: JetApprovalPresentation) async {
        guard supervisionOperation == nil,
              approval.canAuthorizeRetry,
              let runID = approval.runID,
              let reviewID = approval.reviewID
        else {
            return
        }
        supervisionOperation = "approval-retry"
        let commandID = approvalRetryCommandIDs[reviewID] ?? UUID()
        approvalRetryCommandIDs[reviewID] = commandID
        do {
            try await activeClient().authorizeApprovalRetry(
                runID: runID,
                reviewID: reviewID,
                commandID: commandID
            )
            approvalRetryCommandIDs.removeValue(forKey: reviewID)
            composerNotice = ComposerNotice(
                kind: .confirmation,
                text: String(localized: "The safety reviewer will check this request again.")
            )
            if let index = timeline.firstIndex(where: { $0.approval?.reviewID == reviewID }),
               let existing = timeline[index].approval
            {
                timeline[index].approval = JetApprovalPresentation(
                    requestID: existing.requestID,
                    reviewID: existing.reviewID,
                    runID: existing.runID,
                    tool: existing.tool,
                    action: existing.action,
                    target: existing.target,
                    scope: existing.scope,
                    consequence: existing.consequence,
                    rationale: existing.rationale,
                    state: existing.state,
                    canAuthorizeRetry: false
                )
            }
        } catch {
            composerNotice = failureNotice(for: error)
        }
        supervisionOperation = nil
    }

    func requestAddProject() {
        leaveConversation()
        newTaskPlaneRegistryID = localPlaneRegistryID
        sidebarSelection = .project
        isWorkPanelPresented = false
        isProjectImporterPresented = true
        setupNotice = nil
    }

    func showProjects() {
        leaveConversation()
        newTaskPlaneRegistryID = localPlaneRegistryID
        sidebarSelection = .project
        isWorkPanelPresented = false
        setupNotice = nil
    }

    func selectProject(_ projectID: UUID) {
        selectProject(projectID, on: localPlaneRegistryID)
    }

    func selectProject(_ projectID: UUID, on planeRegistryID: UUID) {
        let planeSnapshot = planes.first(where: { $0.id == planeRegistryID })?.snapshot
        let snapshot = planeSnapshot
            ?? (planeRegistryID == localPlaneRegistryID ? setupSnapshot : nil)
        guard snapshot?.projects.projects.contains(where: { $0.id == projectID }) == true
        else {
            return
        }
        leaveConversation()
        newTaskPlaneRegistryID = planeRegistryID
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
            setupNotice = plainMessage(for: presentationError(error))
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
            setupNotice = String(localized: "Added “\(project.name)”.")
            await loadSetup()
            selectedProjectID = project.id
        } catch {
            setupNotice = plainMessage(for: presentationError(error))
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
            setupNotice = plainMessage(for: presentationError(error))
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
                ? String(localized: "Moved the “\(name)” folder to the Trash.")
                : String(localized: "Deleted the “\(name)” folder.")
            await loadSetup()
        } catch {
            let failure = presentationError(error)
            permanentRemovalAllowed = failure.code == "project.trash_unavailable"
            setupNotice = plainMessage(for: failure)
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
            setupNotice = String(localized: "\(binding.label) is connected.")
            await loadSetup()
        } catch {
            setupNotice = plainMessage(for: presentationError(error))
        }
        setupOperation = nil
    }

    func skipRemotePairing() {
        remotePairingSkipped = true
        setupNotice = String(localized: "You can connect another computer later in Settings.")
    }

    func chooseNewTaskPlane(_ planeRegistryID: UUID) {
        guard let plane = planes.first(where: { $0.id == planeRegistryID }) else { return }
        let changed = newTaskPlaneRegistryID != planeRegistryID
        newTaskPlaneRegistryID = planeRegistryID
        let project = plane.snapshot?.projects.projects.first
        selectedProjectID = project?.id
        composerNotice = changed ? project.map {
            ComposerNotice(
                kind: .info,
                text: String(localized: "Project changed to \($0.name) on \(plane.name).")
            )
        } : nil
    }

    func refreshPlanes() async {
        await loadSetup()
        await loadConversations()
    }

    func setPairingGate(on planeRegistryID: UUID, open: Bool) async {
        guard remotePairingOperation == nil else { return }
        remotePairingOperation = "gate"
        remotePairingNotice = nil
        let intent = PairingGateIntent(planeRegistryID: planeRegistryID, open: open)
        let commandID = pairingGateCommandIDs[intent] ?? UUID()
        pairingGateCommandIDs[intent] = commandID
        do {
            _ = try await client(for: planeRegistryID).setPairingGate(
                open ? "open" : "closed",
                commandID: commandID
            )
            pairingGateCommandIDs.removeValue(forKey: intent)
            if !open {
                openedPairing = nil
                openedPairingPlaneID = nil
            }
            await loadPlaneSnapshot(planeRegistryID)
            remotePairingNotice = open
                ? String(localized: "This computer now accepts one new connection.")
                : String(localized: "This computer no longer accepts new connections.")
        } catch {
            remotePairingNotice = plainMessage(for: presentationError(error))
        }
        remotePairingOperation = nil
    }

    func openManualPairing(on planeRegistryID: UUID) async {
        guard remotePairingOperation == nil else { return }
        remotePairingOperation = "open"
        remotePairingNotice = nil
        do {
            let client = try await client(for: planeRegistryID)
            if planes.first(where: { $0.id == planeRegistryID })?.snapshot?.pairing.gate != "open" {
                let gateIntent = PairingGateIntent(
                    planeRegistryID: planeRegistryID,
                    open: true
                )
                let gateCommandID = pairingGateCommandIDs[gateIntent] ?? UUID()
                pairingGateCommandIDs[gateIntent] = gateCommandID
                _ = try await client.setPairingGate("open", commandID: gateCommandID)
                pairingGateCommandIDs.removeValue(forKey: gateIntent)
            }
            let commandID = openPairingCommandIDs[planeRegistryID] ?? UUID()
            openPairingCommandIDs[planeRegistryID] = commandID
            openedPairing = try await client.openManualPairing(commandID: commandID)
            openedPairingPlaneID = planeRegistryID
            openPairingCommandIDs.removeValue(forKey: planeRegistryID)
            await loadPlaneSnapshot(planeRegistryID)
        } catch {
            remotePairingNotice = plainMessage(for: presentationError(error))
        }
        remotePairingOperation = nil
    }

    func confirmPendingPairing(on planeRegistryID: UUID) async {
        guard remotePairingOperation == nil,
              let pending = planes.first(where: { $0.id == planeRegistryID })?
                .snapshot?.pairing.pending,
              case let .awaitingConfirmation(_, authenticationString) = pending.progress
        else { return }
        remotePairingOperation = "confirm"
        remotePairingNotice = nil
        let intent = PairingConfirmationIntent(
            planeRegistryID: planeRegistryID,
            offerID: pending.id
        )
        let commandID = confirmPairingCommandIDs[intent] ?? UUID()
        confirmPairingCommandIDs[intent] = commandID
        do {
            _ = try await client(for: planeRegistryID).confirmPairing(
                offerID: pending.id,
                authenticationString: authenticationString,
                commandID: commandID
            )
            confirmPairingCommandIDs.removeValue(forKey: intent)
            await loadPlaneSnapshot(planeRegistryID)
            remotePairingNotice = String(localized: "The other computer confirmed the matching words.")
        } catch {
            remotePairingNotice = plainMessage(for: presentationError(error))
        }
        remotePairingOperation = nil
    }

    func setPairedClientAccess(
        _ pairedClient: JetPairedClientSummary,
        on planeRegistryID: UUID,
        enabled: Bool
    ) async {
        guard remotePairingOperation == nil else { return }
        remotePairingOperation = "access-\(pairedClient.id.uuidString)"
        remotePairingNotice = nil
        let access: JetPairedClientAccess = enabled ? .enabled : .disabled
        let intent = PairedClientAccessIntent(
            planeRegistryID: planeRegistryID,
            clientID: pairedClient.id,
            access: access
        )
        let commandID = pairedClientAccessCommandIDs[intent] ?? UUID()
        pairedClientAccessCommandIDs[intent] = commandID
        do {
            _ = try await client(for: planeRegistryID).setPairedClientAccess(
                clientID: pairedClient.id,
                access: access,
                commandID: commandID
            )
            pairedClientAccessCommandIDs.removeValue(forKey: intent)
            await loadPlaneSnapshot(planeRegistryID)
            remotePairingNotice = enabled
                ? String(localized: "That Jet app can connect again.")
                : String(localized: "That Jet app is turned off. It stays paired.")
        } catch {
            remotePairingNotice = plainMessage(for: presentationError(error))
        }
        remotePairingOperation = nil
    }

    func requestPairedClientRevocation(
        _ client: JetPairedClientSummary,
        on planeRegistryID: UUID
    ) {
        pairedClientPendingRevocation = client
        pairedClientPendingRevocationPlaneID = planeRegistryID
    }

    func cancelPairedClientRevocation() {
        pairedClientPendingRevocation = nil
        pairedClientPendingRevocationPlaneID = nil
    }

    func confirmPairedClientRevocation() async {
        guard remotePairingOperation == nil,
              let pairedClientPendingRevocation,
              let planeRegistryID = pairedClientPendingRevocationPlaneID
        else { return }
        remotePairingOperation = "revoke-\(pairedClientPendingRevocation.id.uuidString)"
        remotePairingNotice = nil
        let intent = PairedClientIntent(
            planeRegistryID: planeRegistryID,
            clientID: pairedClientPendingRevocation.id
        )
        let commandID = pairedClientRevokeCommandIDs[intent] ?? UUID()
        pairedClientRevokeCommandIDs[intent] = commandID
        do {
            _ = try await client(for: planeRegistryID).revokePairedClient(
                clientID: pairedClientPendingRevocation.id,
                commandID: commandID
            )
            pairedClientRevokeCommandIDs.removeValue(forKey: intent)
            cancelPairedClientRevocation()
            await loadPlaneSnapshot(planeRegistryID)
            remotePairingNotice = String(localized: "That Jet app can no longer connect.")
        } catch {
            remotePairingNotice = plainMessage(for: presentationError(error))
        }
        remotePairingOperation = nil
    }

    func claimRemotePlane() async {
        guard remotePairingOperation == nil, let claimRemotePairing else { return }
        remotePairingOperation = "remote-claim"
        remotePairingNotice = nil
        do {
            let endpoint = try JetSSHEndpoint.validated(
                remoteSSHEndpoint.trimmingCharacters(in: .whitespacesAndNewlines)
            )
            remotePairingClaim = try await claimRemotePairing(
                endpoint,
                remotePairingSecret,
                remotePairingClaimCommandID
            )
            // The one-time secret is no longer needed after a successful claim.
            remotePairingSecret = ""
            remotePairingNotice = String(localized: "Check that both computers show the same words, then confirm on the other computer.")
        } catch {
            remotePairingNotice = plainMessage(for: presentationError(error))
        }
        remotePairingOperation = nil
    }

    func completeRemotePlanePairing() async {
        guard remotePairingOperation == nil,
              let claim = remotePairingClaim,
              let completeRemotePairing
        else { return }
        remotePairingOperation = "remote-complete"
        remotePairingNotice = nil
        do {
            _ = try await completeRemotePairing(claim, remotePairingCompleteCommandID)
            remotePairingClaimCommandID = UUID()
            remotePairingCompleteCommandID = UUID()
            let name = try JetRemotePlaneName.validated(
                remotePlaneName,
                fallback: claim.endpoint
            )
            let existing = remoteProfiles.firstIndex { $0.endpoint == claim.endpoint }
            let profileID: UUID
            if let existing {
                profileID = remoteProfiles[existing].id
                remoteProfiles[existing].name = name
                updatePlane(profileID) { $0.name = name }
            } else {
                let profile = JetRemotePlaneProfile(
                    id: UUID(),
                    name: name,
                    endpoint: claim.endpoint,
                    planeID: nil
                )
                profileID = profile.id
                remoteProfiles.append(profile)
                planes.append(JetPlanePresentation(
                    id: profile.id,
                    name: profile.name,
                    endpoint: profile.endpoint,
                    isLocal: false,
                    planeID: nil,
                    connection: .disconnected,
                    snapshot: nil,
                    failure: nil,
                    conversationCursor: nil
                ))
            }
            saveRemoteProfiles(remoteProfiles)
            remotePairingClaim = nil
            remotePairingSecret = ""
            // Pairing never switches the computer New Task uses.
            await loadPlaneSnapshot(profileID)
            remotePairingNotice = String(localized: "Connected. New tasks still start on This Mac.")
        } catch {
            remotePairingNotice = plainMessage(for: presentationError(error))
        }
        remotePairingOperation = nil
    }

    func remotePairingInputDidChange() {
        remotePairingClaim = nil
        remotePairingClaimCommandID = UUID()
        remotePairingCompleteCommandID = UUID()
        remotePairingNotice = nil
    }

    func restartRemotePlanePairing() {
        remotePairingSecret = ""
        remotePairingInputDidChange()
    }

    func forgetRemotePlane(_ planeRegistryID: UUID) async {
        guard planeRegistryID != localPlaneRegistryID else { return }
        let removedSelectedConversation = selectedConversationID.flatMap {
            conversationPlaneRegistryIDs[$0]
        } == planeRegistryID
        if let client = remoteClients.removeValue(forKey: planeRegistryID) {
            await client.disconnect()
        }
        eventObservationTasks.removeValue(forKey: planeRegistryID)?.cancel()
        planeConnectionObservationTasks.removeValue(forKey: planeRegistryID)?.cancel()
        remoteProfiles.removeAll { $0.id == planeRegistryID }
        planes.removeAll { $0.id == planeRegistryID }
        planeConversations.removeValue(forKey: planeRegistryID)
        planeConversationCursors.removeValue(forKey: planeRegistryID)
        planeNextConversationPages.removeValue(forKey: planeRegistryID)
        conversationPlaneRegistryIDs = conversationPlaneRegistryIDs.filter { $0.value != planeRegistryID }
        if newTaskPlaneRegistryID == planeRegistryID {
            newTaskPlaneRegistryID = localPlaneRegistryID
        }
        if removedSelectedConversation {
            selectedConversationID = nil
            conversationSnapshot = nil
            timeline = []
            turnQueue = nil
            runExecution = nil
            resetWorkPanel()
            sidebarSelection = .newTask
            isWorkPanelPresented = false
        }
        saveRemoteProfiles(remoteProfiles)
        rebuildConversationAggregation()
        remotePairingNotice = String(localized: "Removed from this Mac. Its tasks stay on that computer.")
    }

    /// Restores the scene. Retired destinations (search, needs attention,
    /// schedules and computers) reopen the last task instead.
    func restore(selection: String, workPanel: String, panelPresented: Bool) {
        if let selection = SidebarDestination(rawValue: selection) {
            sidebarSelection = selection.restoresAsConversation ? .conversation : selection
        }
        if let workPanel = WorkPanelTab(rawValue: workPanel) {
            selectedWorkPanel = workPanel
        }
        isWorkPanelPresented = panelPresented
        applySidebarSelection()
    }

    /// Opens New Task without moving focus, so arrowing onto its row keeps the
    /// sidebar's keyboard focus. ⌘N, the toolbar button and Return on the row
    /// bump `composerFocusRequest` themselves.
    func beginNewTask() {
        leaveConversation()
        sidebarSelection = .newTask
        isWorkPanelPresented = false
        composerNotice = nil
    }

    func selectSearch() {
        sidebarSelection = .search
        isWorkPanelPresented = false
        composerNotice = nil
    }

    func applySidebarSelection() {
        composerNotice = nil
        switch sidebarSelection {
        case .newTask:
            leaveConversation()
            isWorkPanelPresented = false
        case .search:
            isWorkPanelPresented = false
        case .needsAttention:
            if usesLivePlane {
                // Open the first task that needs you, otherwise New Task.
                if let conversationID = needsYouConversationIDs.first {
                    selectConversation(conversationID)
                } else {
                    beginNewTask()
                }
            } else {
                showScenario(.approval, fallback: .recovery)
                isWorkPanelPresented = true
            }
        case .project:
            showScenario(.ready)
            isWorkPanelPresented = false
        case .conversation:
            break
        case .schedules:
            break
        case .planes:
            isWorkPanelPresented = false
        case .trash:
            leaveConversation()
            isWorkPanelPresented = false
        }
    }

    /// Renames the open task against the revision the person saw.
    func renameTask(_ name: String, revision: UInt64, commandID: UUID) async -> Bool {
        guard let conversation = selectedConversation,
              planeIsConnected, userOperation == nil else { return false }
        userOperation = .renaming
        defer { userOperation = nil }
        do {
            let client = try await activeClient()
            let updated = try await client.renameConversation(id: conversation.id, revision: revision, name: name, commandID: commandID)
            mergeConversation(updated)
            await loadSelectedConversation()
            return true
        } catch {
            let failure = presentationError(error)
            if failure.category == .conflict {
                await loadSelectedConversation()
                composerNotice = ComposerNotice(
                    kind: .warning,
                    text: String(localized: "This task changed while you were renaming it. Your new name was kept; save it again.")
                )
            } else {
                composerNotice = failureNotice(for: failure)
            }
            return false
        }
    }

    /// Sends the draft. A task that already has a Run continues with
    /// `submit_turn`, so the core resumes the same assistant; `start_run` starts
    /// only a task's first Run. A new task's draft moves to that task once it
    /// exists and clears only after the core accepted the message.
    func submitDraft() async {
        guard userOperation == nil else { return }
        if let blocker = sendBlocker {
            if let message = blocker.message {
                composerNotice = ComposerNotice(kind: .warning, text: message, action: blocker.action)
            }
            return
        }
        guard let route = sendRoute else { return }
        let prompt = draft
        let craft = selectedCraftID
        let operation: UserOperation = route == .startRun ? .starting : .sending
        userOperation = operation
        // Released once, and only while it is still this call's: a rename or send
        // started during the refresh below keeps its own operation.
        var holdsOperation = true
        func releaseOperation() {
            if holdsOperation, userOperation == operation { userOperation = nil }
            holdsOperation = false
        }
        defer { releaseOperation() }
        composerNotice = nil
        actionError = nil
        do {
            var conversationID = selectedConversationID
            if conversationID == nil {
                guard let selectedProjectID else { return }
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
                // This client created the task, so it sees its whole history.
                transcripts.markObservedStart(conversation.id)
                moveNewTaskDraft(to: conversation.id)
                switchConversation(to: conversation.id)
                sidebarSelection = .conversation
                conversationID = conversation.id
                conversationSnapshot = try await activeClient().conversation(conversation.id)
            }
            guard let conversationID else { return }

            switch route {
            case .submitTurn:
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
            case .startRun:
                guard let craft else {
                    throw JetPresentationError.invalidInput(
                        code: "craft.unavailable",
                        message: String(localized: "Install Claude Code or Codex to start.")
                    )
                }
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
                memory.recordAssistant(craft, for: conversationID)
            }
            clearSentDraft(prompt, for: conversationID)
            if selectedConversationID == conversationID { composerPlaceholderOverride = nil }
            releaseOperation()
            await loadSelectedConversation()
        } catch {
            let failure = presentationError(error)
            actionError = failure
            composerNotice = failureNotice(for: failure)
            releaseOperation()
            await loadConversations()
        }
    }

    /// A task created from New Task takes the New Task draft with it.
    func moveNewTaskDraft(to conversationID: UUID) {
        guard let text = drafts.removeValue(forKey: .newTask) else { return }
        drafts[.conversation(conversationID)] = text
    }

    /// Clears a task's draft after the core accepted it, unless it was edited since.
    func clearSentDraft(_ prompt: String, for conversationID: UUID) {
        let key = DraftKey.conversation(conversationID)
        if drafts[key] == prompt { drafts.removeValue(forKey: key) }
    }

    func showFixture(_ state: DesktopFixtureState) {
        composerNotice = nil
        showScenario(state)
    }

    func conversationPlaneName(_ conversationID: UUID) -> String {
        guard let planeRegistryID = conversationPlaneRegistryIDs[conversationID] else {
            return String(localized: "Computer")
        }
        return planeName(planeRegistryID)
    }

    func retryFixtureLoad() {
        didLoadFixtures = false
        contentState = .loading
        Task { await loadFoundationFixture() }
    }

    func resetWorkPanel() {
        workRequest += 1
        workFileRequest += 1
        workDiff = nil
        workFiles = []
        workNextPage = nil
        workPatch = ""
        workTarget = nil
        workTerminals = []
        workOperation = nil
        workError = nil
        workNotice = nil
        workNoticeError = nil
        selectedWorkFilePath = nil
        editableFile = nil
        fileDraft = ""
        selectedTerminalID = nil
        terminalInput = ""
        terminalOutput = [:]
        workScrollAnchors = [:]
        gitDeliveries = []
        gitDeliveryOperation = nil
        gitDeliveryNotice = nil
        gitDeliveryError = nil
        gitDeliveryConfirmation = nil
        gitDeliveryAcknowledgementConfirmation = nil
        gitDeliveryAdmissionUncertain = nil
        pendingGitDelivery = nil
        gitDeliveryAcknowledgementCommandIDs = [:]
        gitDeliveryObservationTask?.cancel()
        gitDeliveryObservationTask = nil
        terminalOffsets = [:]
        terminalDecoders = [:]
        lastTerminalSize = nil
        workArtifactBytes = Data()
        pendingWorkEdit = nil
        pendingReview = nil
        terminalOpenCommandID = UUID()
        terminalCloseCommandIDs = [:]
        detachCurrentTerminal()
    }

    func detachCurrentTerminal() {
        terminalObservationTask?.cancel()
        terminalObservationTask = nil
        let terminalID = terminalStreamTerminalID
        terminalStreamTerminalID = nil
        attachedTerminalID = nil
        let selectedClient = selectedPlaneRegistryID == localPlaneRegistryID
            ? client
            : remoteClients[selectedPlaneRegistryID]
        if let terminalID, let selectedClient {
            Task { await selectedClient.detachTerminal(terminalID: terminalID) }
        }
    }

    func recordWorkFailure(_ error: Error) {
        let failure = presentationError(error)
        workNotice = plainMessage(for: failure)
        workNoticeError = failure
        applyRevisionConflict(failure)
    }

    func applyRevisionConflict(_ error: JetPresentationError) {
        guard let conflict = error.revisionConflict else { return }
        switch conflict.safeState {
        case let .conversation(id, revision):
            func updated(_ conversation: JetConversationSummary) -> JetConversationSummary {
                JetConversationSummary(
                    id: conversation.id,
                    revision: revision,
                    title: conversation.title,
                    createdAtUnixMilliseconds: conversation.createdAtUnixMilliseconds,
                    projectID: conversation.projectID
                )
            }
            if let planeRegistryID = conversationPlaneRegistryIDs[id],
               let index = planeConversations[planeRegistryID, default: []]
                .firstIndex(where: { $0.id == id })
            {
                planeConversations[planeRegistryID]?[index] = updated(
                    planeConversations[planeRegistryID, default: []][index]
                )
                rebuildConversationAggregation()
            }
            if let snapshot = conversationSnapshot, snapshot.conversation.id == id {
                conversationSnapshot = JetConversationSnapshot(
                    cursor: snapshot.cursor,
                    conversation: updated(snapshot.conversation),
                    workspaceID: snapshot.workspaceID,
                    workspaceRoot: snapshot.workspaceRoot,
                    runs: snapshot.runs
                )
            }
        case let .run(safeRun):
            if let snapshot = conversationSnapshot {
                conversationSnapshot = JetConversationSnapshot(
                    cursor: snapshot.cursor,
                    conversation: snapshot.conversation,
                    workspaceID: snapshot.workspaceID,
                    workspaceRoot: snapshot.workspaceRoot,
                    runs: snapshot.runs.map { $0.id == safeRun.id ? safeRun : $0 }
                )
            }
            if let execution = runExecution, execution.run.id == safeRun.id {
                runExecution = JetRunExecution(
                    cursor: execution.cursor,
                    run: safeRun,
                    activity: execution.activity,
                    needsAttention: execution.needsAttention,
                    termination: execution.termination
                )
            }
        }
    }

    func mergeConversation(_ conversation: JetConversationSummary) {
        let planeRegistryID = selectedPlaneRegistryID
        conversationPlaneRegistryIDs[conversation.id] = planeRegistryID
        if let index = planeConversations[planeRegistryID, default: []]
            .firstIndex(where: { $0.id == conversation.id })
        {
            planeConversations[planeRegistryID]?[index] = conversation
        } else {
            planeConversations[planeRegistryID, default: []].insert(conversation, at: 0)
        }
        rebuildConversationAggregation()
    }

    func pendingStartFor(
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

    func pendingTurnFor(
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

    func observeEvents(for planeRegistryID: UUID, after initialCursor: UInt64) {
        guard !isPreviewSession, eventObservationTasks[planeRegistryID] == nil else { return }
        eventObservationTasks[planeRegistryID] = Task { [weak self] in
            guard let self else { return }
            var cursor = initialCursor
            while !Task.isCancelled {
                do {
                    let client = try await client(for: planeRegistryID)
                    let events = await client.eventStream(after: cursor)
                    for try await event in events {
                        guard !Task.isCancelled else { return }
                        cursor = event.sequence
                        planeConversationCursors[planeRegistryID] = cursor
                        updatePlane(planeRegistryID) { plane in
                            plane.conversationCursor = cursor
                            plane.failure = nil
                        }
                        await receive(event, from: planeRegistryID)
                    }
                } catch {
                    let failure = presentationError(error)
                    updatePlane(planeRegistryID) { $0.failure = failure }
                    if failure.restart?.requiresEventSnapshot == true {
                        // Events were missed, so cached status and transcripts for this
                        // computer can no longer be trusted to be continuous.
                        let affected = conversationIDs(on: planeRegistryID)
                        statusStore.reset(conversationIDs: affected)
                        transcripts.reset(conversationIDs: affected)
                        if selectedPlaneRegistryID == planeRegistryID, selectedConversationID != nil {
                            timeline = []
                            composerNotice = ComposerNotice(
                                kind: .info,
                                text: String(localized: "Jet refreshed this task.")
                            )
                        }
                        await loadConversations()
                        cursor = planeConversationCursors[planeRegistryID] ?? 0
                        continue
                    }
                    // The banner and the status show that this view is saved, not live.
                    if selectedPlaneRegistryID == planeRegistryID, conversationSnapshot != nil {
                        conversationFreshness = .cached
                    }
                    try? await Task.sleep(for: .seconds(1))
                }
            }
        }
    }

    /// Every Conversation this session knows on one computer.
    func conversationIDs(on planeRegistryID: UUID) -> [UUID] {
        var ids = planeConversations[planeRegistryID, default: []].map(\.id)
        let known = Set(ids)
        ids += conversationPlaneRegistryIDs
            .filter { $0.value == planeRegistryID && !known.contains($0.key) }
            .map(\.key)
        return ids
    }

    func receive(_ event: JetEvent, from planeRegistryID: UUID) async {
        statusStore.record(event, planeRegistryID: planeRegistryID)
        await notificationRouter.handle(event, planeRegistryID: planeRegistryID)
        let projections = Self.stamped(event.timelineProjections(), from: event)
        if let conversationID = event.conversationID {
            if conversationID == selectedConversationID,
               selectedPlaneRegistryID == planeRegistryID
            {
                var dropped = false
                if projections.isEmpty {
                    dropped = TranscriptStore.groupRaw(sequence: event.sequence, into: &timeline)
                } else {
                    for projection in projections {
                        dropped = TranscriptStore.merge(projection, into: &timeline) || dropped
                    }
                }
                transcripts.save(timeline, for: conversationID)
                if dropped { transcripts.markPartial(conversationID) }
                if event.kind == "run.lifecycle_changed"
                    || event.kind == "run.activity_changed"
                    || event.kind == "turn.changed"
                {
                    await loadSelectedConversation()
                } else if event.kind.hasPrefix("approval.")
                    || event.kind == "run.control_requested"
                    || event.kind == "run.terminated"
                {
                    await loadRunSupervision()
                }
            } else {
                transcripts.ingest(
                    projections: projections,
                    rawSequence: event.sequence,
                    conversationID: conversationID
                )
            }
            switch event.kind {
            case "conversation.created":
                transcripts.markObservedStart(conversationID)
            case "conversation.trashed":
                trashedConversationIDs.insert(conversationID)
                statusStore.remove(conversationID)
                transcripts.remove(conversationID)
            case "conversation.restored":
                trashedConversationIDs.remove(conversationID)
            default:
                break
            }
        }
        if [
            "conversation.created",
            "conversation.name_changed",
            "conversation.trashed",
            "conversation.restored",
        ].contains(event.kind) {
            await loadConversations()
        }
    }

    /// Adds the Event's time, Run and checkpoint Turn to its projections.
    static func stamped(_ projections: [JetTimelineEntry], from event: JetEvent) -> [JetTimelineEntry] {
        guard !projections.isEmpty else { return projections }
        let checkpointTurn = event.kind == "change.checkpoint_recorded"
            ? checkpointTurn(of: event)
            : nil
        return projections.map { projection in
            var entry = projection
            if entry.recordedAtUnixMilliseconds == nil {
                entry.recordedAtUnixMilliseconds = event.recordedAtUnixMilliseconds
            }
            if entry.runID == nil { entry.runID = event.runID }
            if entry.checkpointTurn == nil { entry.checkpointTurn = checkpointTurn }
            return entry
        }
    }

    /// Reads only the bounded `turn` number of a checkpoint Event (ASVS 1.5.2).
    static func checkpointTurn(of event: JetEvent) -> UInt32? {
        guard let data = event.payload.source.data(using: .utf8),
              data.count <= 65_536,
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let turn = object["turn"] as? NSNumber,
              turn.int64Value > 0, turn.int64Value <= Int64(UInt32.max)
        else { return nil }
        return UInt32(turn.int64Value)
    }

    func showScenario(
        _ state: DesktopFixtureState,
        fallback: DesktopFixtureState? = nil
    ) {
        if let scenario = scenarios[state] ?? fallback.flatMap({ scenarios[$0] }) {
            contentState = .ready(scenario)
        }
    }

    func activeClient() async throws -> JetClient {
        try await client(for: selectedPlaneRegistryID)
    }

    func client(for planeRegistryID: UUID) async throws -> JetClient {
        if planeRegistryID == localPlaneRegistryID {
            if let client { return client }
            guard let makeJetClient else {
                throw JetClientFailure.presentation(.offline)
            }
            let client = try await makeJetClient()
            self.client = client
            observeConnection(of: client)
            return client
        }
        if let client = remoteClients[planeRegistryID] { return client }
        guard let profile = remoteProfiles.first(where: { $0.id == planeRegistryID }),
              let makeRemoteClient
        else {
            throw JetClientFailure.presentation(.offline)
        }
        updatePlane(planeRegistryID) { $0.connection = .connecting }
        do {
            let client = try await makeRemoteClient(profile.endpoint)
            remoteClients[planeRegistryID] = client
            observeConnection(of: client, planeRegistryID: planeRegistryID)
            return client
        } catch {
            let failure = presentationError(error)
            updatePlane(planeRegistryID) { plane in
                plane.connection = failure.retryable ? .reconnecting(attempt: 1) : .failed(failure)
                plane.failure = failure
            }
            throw error
        }
    }

    func observeConnection(of client: JetClient) {
        connectionObservationTask?.cancel()
        connectionObservationTask = Task { [weak self] in
            guard let self else { return }
            let states = await client.connectionStates()
            for await state in states {
                guard !Task.isCancelled else { return }
                connectionState = state
                updatePlane(localPlaneRegistryID) { plane in
                    plane.connection = state
                    if case .connected = state { plane.failure = nil }
                }
                if case .connected = state,
                   conversationFreshness == .cached
                {
                    await loadConversations()
                } else if case .reconnecting = state,
                          conversationSnapshot != nil
                {
                    conversationFreshness = .cached
                } else if case .disconnected = state,
                          conversationSnapshot != nil
                {
                    conversationFreshness = .cached
                }
            }
        }
    }

    func observeConnection(of client: JetClient, planeRegistryID: UUID) {
        planeConnectionObservationTasks[planeRegistryID]?.cancel()
        planeConnectionObservationTasks[planeRegistryID] = Task { [weak self] in
            let states = await client.connectionStates()
            for await state in states {
                guard !Task.isCancelled else { return }
                self?.updatePlane(planeRegistryID) { plane in
                    plane.connection = state
                    if case .connected = state { plane.failure = nil }
                }
                if self?.selectedPlaneRegistryID == planeRegistryID {
                    if case .connected = state, self?.conversationFreshness == .cached {
                        await self?.loadConversations()
                    } else if case .reconnecting = state, self?.conversationSnapshot != nil {
                        self?.conversationFreshness = .cached
                    } else if case .disconnected = state, self?.conversationSnapshot != nil {
                        self?.conversationFreshness = .cached
                    }
                }
            }
        }
    }

    func loadRemotePlaneSnapshots() async {
        await withTaskGroup(of: Void.self) { group in
            for profile in remoteProfiles {
                group.addTask { [weak self] in
                    await self?.loadPlaneSnapshot(profile.id)
                }
            }
        }
    }

    func loadPlaneSnapshot(_ planeRegistryID: UUID) async {
        do {
            let snapshot = try await client(for: planeRegistryID).setupSnapshot()
            updatePlane(planeRegistryID) { plane in
                plane.planeID = snapshot.status.planeID
                plane.snapshot = snapshot
                plane.failure = nil
                plane.conversationCursor = snapshot.status.cursor
            }
            if planeRegistryID == localPlaneRegistryID {
                setupState = .ready(snapshot)
            } else if let index = remoteProfiles.firstIndex(where: { $0.id == planeRegistryID }),
                      remoteProfiles[index].planeID != snapshot.status.planeID
            {
                remoteProfiles[index].planeID = snapshot.status.planeID
                saveRemoteProfiles(remoteProfiles)
            }
        } catch {
            let failure = presentationError(error)
            updatePlane(planeRegistryID) { $0.failure = failure }
        }
    }

    func updatePlane(
        _ planeRegistryID: UUID,
        _ update: (inout JetPlanePresentation) -> Void
    ) {
        guard let index = planes.firstIndex(where: { $0.id == planeRegistryID }) else { return }
        update(&planes[index])
    }

    func rebuildConversationAggregation() {
        conversations = planeConversations.values
            .flatMap { $0 }
            .sorted { left, right in
                if left.createdAtUnixMilliseconds != right.createdAtUnixMilliseconds {
                    return left.createdAtUnixMilliseconds > right.createdAtUnixMilliseconds
                }
                let leftPlane = conversationPlaneRegistryIDs[left.id]?.uuidString ?? ""
                let rightPlane = conversationPlaneRegistryIDs[right.id]?.uuidString ?? ""
                return leftPlane < rightPlane
            }
    }

    func planeName(_ planeRegistryID: UUID) -> String {
        planes.first(where: { $0.id == planeRegistryID })?.name ?? String(localized: "Computer")
    }

    func presentationError(_ error: Error) -> JetPresentationError {
        Self.presentationError(error)
    }

    /// Maps any error to a stable presentation error. Command IDs, native error
    /// text and installer command output never reach casual copy.
    static func presentationError(_ error: Error) -> JetPresentationError {
        switch error {
        case let error as JetPresentationError: return error
        case let JetClientFailure.presentation(error): return error
        case JetClientFailure.commandOutcomeUnknown:
            return JetPresentationError(
                category: .outcomeUnknown,
                code: "command.outcome_unknown",
                message: String(localized: "Jet couldn't confirm whether that went through. Check the task before trying again."),
                retryable: false,
                recoveryActions: []
            )
        case is JetIdentityFailure:
            return JetPresentationError(
                category: .unavailable,
                code: "credential.keychain_unavailable",
                message: String(localized: "Jet couldn't read its key from your Keychain."),
                retryable: true
            )
        case is CancellationError:
            return .cancelled
        default:
#if os(macOS)
            if let installer = error as? JetLocalCoreInstaller.InstallerError {
                return JetPresentationError(
                    category: .unavailable,
                    code: installer.code,
                    message: installer.errorDescription ?? String(localized: "Jet couldn't start its helper on this Mac."),
                    retryable: installer.isRetryable
                )
            }
#endif
            return .invalidResponse
        }
    }

    /// Plain copy for a failure. Categories whose wire messages are technical get
    /// a sentence here; specific messages such as invalid input are kept.
    func plainMessage(for failure: JetPresentationError) -> String {
        switch failure.category {
        case .offline:
            String(localized: "Can't reach \(selectedPlaneName) right now.")
        case .outcomeUnknown:
            String(localized: "Jet couldn't confirm whether that went through. Check the task before trying again.")
        case .invalidResponse:
            String(localized: "Jet got a response it couldn't use. Try again, or update Jet.")
        case .incompatible:
            String(localized: "This version of Jet can't work with \(selectedPlaneName). Update Jet.")
        case .overloaded, .rateLimited:
            String(localized: "Jet is busy right now. Try again in a moment.")
        case .cancelled:
            String(localized: "Cancelled.")
        case .internalFailure:
            String(localized: "Something went wrong on \(selectedPlaneName). Try again.")
        case .invalidInput, .unauthorized, .conflict, .unavailable, .notFound:
            failure.message
        }
    }

    /// A composer notice for a failed request. The error itself stays in `actionError`.
    func failureNotice(for error: Error) -> ComposerNotice {
        let failure = presentationError(error)
        if failure.category == .offline {
            return ComposerNotice(
                kind: .warning,
                text: plainMessage(for: failure),
                action: .tryAgainConnection
            )
        }
        return ComposerNotice(kind: .error, text: plainMessage(for: failure))
    }
}
