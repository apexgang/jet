import CryptoKit
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

    private struct RunControlKey: Hashable {
        let runID: UUID
        let control: JetRunControl
    }

    private struct PendingWorkEdit {
        let path: String
        let content: String
        let commandID: UUID
    }

    private struct PendingReview {
        let path: String
        let line: UInt32
        let comment: String
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
    var supervisionOperation: String?
    var timeline: [JetTimelineEntry] = []
    var turnQueue: JetTurnQueue?
    var runExecution: JetRunExecution?
    var runControlConfirmation: JetRunControl?
    var searchText = ""
    var searchResult: JetSearchResult?
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
    private var withdrawalCommandIDs: [UUID: UUID] = [:]
    private var runControlCommandIDs: [RunControlKey: UUID] = [:]
    private var approvalRetryCommandIDs: [UUID: UUID] = [:]
    private var eventObservationTask: Task<Void, Never>?
    private var conversationRequest = 0
    private var searchRequest = 0
    private var workRequest = 0
    private var workFileRequest = 0
    private var workArtifactBytes = Data()
    private var pendingWorkEdit: PendingWorkEdit?
    private var pendingReview: PendingReview?
    private var terminalOpenCommandID = UUID()
    private var terminalCloseCommandIDs: [UUID: UUID] = [:]
    private var terminalOffsets: [UUID: UInt64] = [:]
    private var terminalDecoders: [UUID: TerminalTranscriptDecoder] = [:]
    private var lastTerminalSize: (terminalID: UUID, rows: UInt16, columns: UInt16)?
    private var terminalStreamTerminalID: UUID?
    private var terminalObservationTask: Task<Void, Never>?

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
                detachCurrentTerminal()
                timeline = []
                conversationSnapshot = nil
                resetWorkPanel()
            }
            if let restoredSnapshot,
               restoredSnapshot.conversation.id == selectedConversationID
            {
                conversationSnapshot = restoredSnapshot
                await loadRunSupervision()
            } else if selectedConversationID != nil {
                await loadSelectedConversation()
            } else {
                conversationSnapshot = nil
                turnQueue = nil
                runExecution = nil
                resetWorkPanel()
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
            if failure.restart?.requiresPaginationSnapshot == true {
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
            detachCurrentTerminal()
            timeline = []
            conversationSnapshot = nil
            turnQueue = nil
            runExecution = nil
            resetWorkPanel()
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
            await loadRunSupervision()
        } catch {
            guard self.selectedConversationID == selectedConversationID else { return }
            conversationFreshness = conversationSnapshot == nil ? .failed : .cached
            actionNotice = presentationError(error).message
        }
        conversationOperation = nil
    }

    func loadRunSupervision() async {
        guard usesLivePlane, let conversationID = selectedConversationID else {
            turnQueue = nil
            runExecution = nil
            return
        }
        supervisionOperation = "refresh"
        defer { supervisionOperation = nil }
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
        } catch {
            guard selectedConversationID == conversationID else { return }
            actionNotice = presentationError(error).message
        }
        if selectedConversationID == conversationID, selectedRun != nil {
            await loadWorkPanel()
        } else if selectedConversationID == conversationID {
            resetWorkPanel()
        }
    }

    func loadWorkPanel(preserveContinuity: Bool = true) async {
        guard usesLivePlane,
              let conversationID = selectedConversationID,
              let run = selectedRun
        else {
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
        if previousRunID != run.id {
            checkpointKind = run.lifecycle.isLive ? .current : .final
        }
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
                    workNotice = "Changes loaded, but Workspace terminals are unavailable."
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
        if request == workRequest { workOperation = nil }
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
            observeEvents(after: after)
            workNotice = "Activity reconnected from the requested checkpoint."
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
            workNotice = failure.message
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
                workNotice = "The complete patch was verified."
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
            workNotice = "Saved through the selected Workspace."
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
            workNotice = "The review comment was added as one queued Turn."
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
            workNotice = "The Workspace terminal was closed."
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

    private func resizeAttachedTerminal() async {
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

    private func receiveTerminal(_ event: JetTerminalEvent, terminalID: UUID) {
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
            workNotice = "The terminal session finished."
        }
    }

    func withdrawTurn(_ turn: JetTurnQueueEntry) async {
        guard supervisionOperation == nil,
              turn.withdrawable,
              turn.state == .queued,
              let conversationID = selectedConversationID
        else {
            return
        }
        supervisionOperation = "withdraw"
        let commandID = withdrawalCommandIDs[turn.id] ?? UUID()
        withdrawalCommandIDs[turn.id] = commandID
        do {
            _ = try await activeClient().withdrawTurn(
                conversationID: conversationID,
                turnID: turn.id,
                commandID: commandID
            )
            withdrawalCommandIDs.removeValue(forKey: turn.id)
            actionNotice = "The queued Turn was withdrawn."
            await loadRunSupervision()
        } catch {
            actionNotice = presentationError(error).message
        }
        supervisionOperation = nil
    }

    func requestRunControl(_ control: JetRunControl) {
        guard (control == .interruptTurn ? canInterruptTurn : canStopRun) else { return }
        runControlConfirmation = control
        selectedWorkPanel = .run
        isWorkPanelPresented = true
    }

    func cancelRunControl() {
        runControlConfirmation = nil
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
            actionNotice = control == .interruptTurn
                ? "Interrupt requested. Waiting for the active Turn to end."
                : "Stop requested. Waiting for the Run to end."
            await loadRunSupervision()
        } catch {
            actionNotice = presentationError(error).message
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
            actionNotice = "One exact review retry was authorized."
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
            actionNotice = presentationError(error).message
        }
        supervisionOperation = nil
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
        detachCurrentTerminal()
        sidebarSelection = .newTask
        selectedConversationID = nil
        conversationSnapshot = nil
        turnQueue = nil
        runExecution = nil
        timeline = []
        resetWorkPanel()
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
            detachCurrentTerminal()
            selectedConversationID = nil
            conversationSnapshot = nil
            turnQueue = nil
            runExecution = nil
            timeline = []
            resetWorkPanel()
            isWorkPanelPresented = false
            composerFocusRequest += 1
        case .search:
            isWorkPanelPresented = false
        case .needsAttention:
            if usesLivePlane {
                actionNotice = attentionCount == 0
                    ? "No current Run needs attention."
                    : "Showing the selected Run items that need attention."
                selectedWorkPanel = .run
                isWorkPanelPresented = true
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

    private func resetWorkPanel() {
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

    private func detachCurrentTerminal() {
        terminalObservationTask?.cancel()
        terminalObservationTask = nil
        let terminalID = terminalStreamTerminalID
        terminalStreamTerminalID = nil
        attachedTerminalID = nil
        if let terminalID, let client {
            Task { await client.detachTerminal(terminalID: terminalID) }
        }
    }

    private func recordWorkFailure(_ error: Error) {
        let failure = presentationError(error)
        workNotice = failure.message
        workNoticeError = failure
        applyRevisionConflict(failure)
    }

    private func applyRevisionConflict(_ error: JetPresentationError) {
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
            if let index = conversations.firstIndex(where: { $0.id == id }) {
                conversations[index] = updated(conversations[index])
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
                    if failure.restart?.requiresEventSnapshot == true {
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
            } else if event.kind.hasPrefix("approval.")
                || event.kind == "run.control_requested"
                || event.kind == "run.terminated"
            {
                await loadRunSupervision()
            }
        }
        if ["conversation.created", "conversation.name_changed", "conversation.trashed"]
            .contains(event.kind)
        {
            await loadConversations()
        }
    }

    private func mergeTimeline(_ entry: JetTimelineEntry) {
        if entry.kind == .approval,
           let index = timeline.firstIndex(where: { $0.id == entry.id })
        {
            timeline[index] = entry
        } else if entry.kind == .user,
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
