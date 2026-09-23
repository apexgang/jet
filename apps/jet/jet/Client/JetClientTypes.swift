import Foundation

extension SettingKey: @unchecked Sendable {}

struct JetNegotiation: Sendable, Equatable {
    let protocolVersion: UInt32
    let minorVersion: UInt32
    let codec: String
    let frameLimits: JetFrameLimits
}

enum JetConnectionState: Sendable, Equatable {
    case disconnected
    case connecting
    case connected(JetNegotiation)
    case reconnecting(attempt: Int)
    case failed(JetPresentationError)
}

nonisolated struct JetPlaneStatus: Sendable, Equatable {
    let cursor: UInt64?
    let planeID: UUID
    let daemonStarts: UInt64
    let startedAtUnixMilliseconds: Int64
    let coreVersion: String
    let security: JetRawJSON?
    let recovery: JetRawJSON?
}

nonisolated struct JetRawJSON: Sendable, Equatable {
    /// Exact UTF-8 fragment from the schema-validated frame.
    let source: String
}

struct JetEvent: Sendable, Equatable {
    let sequence: UInt64
    let eventID: UUID
    let actor: JetRawJSON
    let origin: JetRawJSON?
    let recordedAtUnixMilliseconds: Int64
    let conversationID: UUID?
    let runID: UUID?
    let kind: String
    let payloadVersion: UInt32
    let payload: JetRawJSON
}

struct JetEventBatch: Sendable, Equatable {
    let cursor: UInt64
    let events: [JetEvent]
}

enum JetCapabilityObservation: Sendable {
    case lastObserved
    case fresh
}

nonisolated enum JetCredentialStoreState: String, Sendable, Equatable {
    case available
    case locked
    case unavailable

    var label: String {
        switch self {
        case .available: "Secure storage ready"
        case .locked: "Secure storage locked"
        case .unavailable: "Secure storage unavailable"
        }
    }
}

struct JetAuthProvider: Sendable, Equatable, Identifiable {
    let provider: String
    let harness: String
    let label: String

    var id: String { provider }
}

nonisolated struct JetCapabilitySummary: Sendable, Equatable {
    let coreVersion: String
    let platform: String
    let externalTools: [JetExternalToolSummary]
    let harnesses: [String]
    let crafts: [JetInstalledCraft]
    let credentialStore: JetCredentialStoreState
    let degraded: [String]

    var gitIsAvailable: Bool {
        externalTools.contains {
            $0.tool == "git" && $0.availability.isPresent
        }
    }

    var authProviders: [JetAuthProvider] {
        var providers: [JetAuthProvider] = []
        for harness in harnesses {
            let normalized = harness.lowercased()
            let candidate: JetAuthProvider?
            if normalized.contains("codex") {
                candidate = JetAuthProvider(
                    provider: "openai",
                    harness: "Codex",
                    label: "Codex login"
                )
            } else if normalized.contains("claude") {
                candidate = JetAuthProvider(
                    provider: "anthropic",
                    harness: "Claude Code",
                    label: "Claude Code login"
                )
            } else {
                candidate = nil
            }
            if let candidate, !providers.contains(where: { $0.provider == candidate.provider }) {
                providers.append(candidate)
            }
        }
        return providers
    }
}

nonisolated enum JetExternalToolAvailability: Sendable, Equatable {
    case present(version: String)
    case missing

    var isPresent: Bool {
        if case .present = self { return true }
        return false
    }
}

nonisolated struct JetExternalToolSummary: Sendable, Equatable, Identifiable {
    let tool: String
    let availability: JetExternalToolAvailability

    var id: String { tool }
}

nonisolated struct JetInstalledCraft: Sendable, Equatable, Identifiable {
    let id: String
    let version: String
    let harnesses: [String]
}

struct JetConversationSummary: Sendable, Equatable, Identifiable {
    let id: UUID
    let revision: UInt64?
    let title: String
    let createdAtUnixMilliseconds: Int64
    let projectID: UUID?
}

struct JetConversationPage: Sendable, Equatable {
    let cursor: UInt64
    let conversations: [JetConversationSummary]
    let nextPage: UUID?
}

nonisolated enum JetRunLifecycle: String, Sendable, Equatable {
    case created
    case starting
    case active
    case stopping
    case completed
    case failed
    case canceled
    case lost

    var isLive: Bool {
        switch self {
        case .created, .starting, .active, .stopping: true
        case .completed, .failed, .canceled, .lost: false
        }
    }
}

nonisolated struct JetRunSummary: Sendable, Equatable, Identifiable {
    let id: UUID
    let conversationID: UUID
    let revision: UInt64
    let lifecycle: JetRunLifecycle
    let title: String
    let createdAtUnixMilliseconds: Int64
    let endedAtUnixMilliseconds: Int64?
}

struct JetConversationSnapshot: Sendable, Equatable {
    let cursor: UInt64
    let conversation: JetConversationSummary
    let workspaceID: UUID?
    let workspaceRoot: String?
    let runs: [JetRunSummary]
}

nonisolated enum JetFileTarget: Sendable, Equatable, Hashable {
    case project(UUID)
    case workspace(UUID)
}

struct JetFileRevision: Sendable, Equatable {
    let object: String
    let mode: String

    var label: String { "\(mode) · \(object)" }
}

enum JetChangeScope: Sendable, Equatable {
    case current
    case final
    case historical(fromTurn: UInt32, toTurn: UInt32)
    case turn(UInt32)

    var label: String {
        switch self {
        case .current: "Current"
        case .final: "Final"
        case let .historical(fromTurn, toTurn): "Turns \(fromTurn)–\(toTurn)"
        case let .turn(turn): "Turn \(turn)"
        }
    }
}

enum JetArtifactAvailability: String, Sendable, Equatable {
    case stored
    case diskPressure = "disk_pressure"
    case runBudgetExceeded = "run_budget_exceeded"
    case artifactSizeExceeded = "artifact_size_exceeded"
}

struct JetChangeArtifact: Sendable, Equatable {
    let sha256: String
    let size: UInt64
    let availability: JetArtifactAvailability
}

struct JetChangedFile: Sendable, Equatable, Identifiable {
    let path: String
    let beforeObject: String?
    let afterObject: String?
    let beforeSize: UInt64?
    let afterSize: UInt64?
    let origin: String

    var id: String { path }

    var status: String {
        if beforeObject?.allSatisfy({ $0 == "0" }) == true { return "added" }
        if afterObject?.allSatisfy({ $0 == "0" }) == true { return "deleted" }
        return "modified"
    }

    var contentAvailable: Bool { beforeObject != nil && afterObject != nil }
}

struct JetChangeDiff: Sendable, Equatable {
    let cursor: UInt64
    let runID: UUID
    let workspaceID: UUID?
    let scope: JetChangeScope
    let latestTurn: UInt32
    let totalFiles: UInt32
    let files: [JetChangedFile]
    let nextPage: UUID?
    let patch: String
    let patchTruncated: Bool
    let contentComplete: Bool
    let artifact: JetChangeArtifact
}

struct JetChangeArtifactChunk: Sendable, Equatable {
    let artifact: JetChangeArtifact
    let offset: UInt64
    let bytes: Data
}

struct JetGitCheckpoint: Sendable, Equatable {
    let runID: UUID
    let turn: UInt32

    var label: String { "Turn \(turn)" }
}

enum JetGitOperation: Sendable, Equatable, Hashable {
    case branch(name: String)
    case commit
    case push(remote: String)
    case draftPullRequest(remote: String, base: String?)

    var title: String {
        switch self {
        case .branch: "Create Branch"
        case .commit: "Commit"
        case .push: "Push"
        case .draftPullRequest: "GitHub Draft Pull Request"
        }
    }

    var destinationLabel: String {
        switch self {
        case let .branch(name): name
        case .commit: "Current Conversation branch"
        case let .push(remote): remote
        case let .draftPullRequest(remote, base):
            base.map { "\(remote) into \($0)" } ?? "\(remote) default branch"
        }
    }
}

enum JetGitDeliveryChoice: String, CaseIterable, Sendable, Hashable {
    case branch
    case commit
    case push
    case draftPullRequest

    var title: String {
        switch self {
        case .branch: "Branch"
        case .commit: "Commit"
        case .push: "Push"
        case .draftPullRequest: "GitHub Draft PR"
        }
    }
}

struct JetGitDeliveryRequest: Sendable, Equatable {
    let conversationID: UUID
    let checkpoint: JetGitCheckpoint?
    let operation: JetGitOperation

    var reviewSummary: String {
        var parts = [operation.destinationLabel]
        if let checkpoint { parts.append(checkpoint.label) }
        return parts.joined(separator: " · ")
    }
}

struct JetGitMessage: Sendable, Equatable {
    let title: String
    let body: String
    let fallbackReason: String?
}

struct JetGitDeliveryPolicy: Sendable, Equatable {
    let automatic: Bool
    let branch: Bool
    let commit: Bool
    let push: Bool
    let draftPullRequest: Bool
    let branchPrefix: String
}

enum JetGitDeliveryOutcome: Sendable, Equatable {
    case pending
    case completed(head: String, branch: String?, pullRequest: String?)
    case failed(code: String)
    case outcomeUnknown

    var title: String {
        switch self {
        case .pending: "Pending"
        case .completed: "Completed"
        case .failed: "Failed"
        case .outcomeUnknown: "Outcome unknown"
        }
    }
}

struct JetGitDelivery: Sendable, Equatable, Identifiable {
    let id: UUID
    let conversationID: UUID
    let checkpoint: JetGitCheckpoint?
    let operation: JetGitOperation
    let policy: JetGitDeliveryPolicy
    let utilityJobID: UUID?
    let message: JetGitMessage?
    let acknowledgedBy: UUID?
    let outcome: JetGitDeliveryOutcome

    var canRetry: Bool {
        if case .failed = outcome { return true }
        return false
    }

    var needsAcknowledgement: Bool {
        outcome == .outcomeUnknown && acknowledgedBy == nil
    }
}

struct JetGitDeliveryQueued: Sendable, Equatable {
    let deliveryID: UUID
}

struct JetEditableFile: Sendable, Equatable {
    let cursor: UInt64
    let target: JetFileTarget
    let path: String
    let content: String?
    let revision: JetFileRevision
}

enum JetTerminalState: String, Sendable, Equatable {
    case opening
    case open
    case closing
    case closed
    case unavailable
}

struct JetWorkspaceTerminal: Sendable, Equatable, Identifiable {
    let id: UUID
    let workspaceID: UUID
    let state: JetTerminalState
}

enum JetTerminalEvent: Sendable, Equatable {
    case attached
    case output(offset: UInt64, bytes: Data)
    case gap(firstMissingOffset: UInt64, missingBytes: UInt64)
    case resized
    case finished(totalBytes: UInt64)
}

nonisolated enum JetSearchField: String, Sendable, Equatable {
    case name
    case path
    case branch
}

nonisolated struct JetSearchHit: Sendable, Equatable, Identifiable {
    let conversationID: UUID
    let sequence: UInt64
    let field: JetSearchField
    let excerpt: String

    var id: String { "\(conversationID.uuidString)-\(sequence)" }
}

nonisolated struct JetSearchResult: Sendable, Equatable {
    let cursor: UInt64
    let indexedThrough: UInt64
    let hits: [JetSearchHit]
}

nonisolated struct JetFederatedSearchHit: Sendable, Equatable, Identifiable {
    let planeRegistryID: UUID
    let planeName: String
    let hit: JetSearchHit

    var id: String { "\(planeRegistryID.uuidString)-\(hit.id)" }
}

nonisolated struct JetFederatedSearchResult: Sendable, Equatable {
    let hits: [JetFederatedSearchHit]
    let cursors: [UUID: UInt64]
    let indexedThrough: [UUID: UInt64]
    let failures: [UUID: JetPresentationError]
}

nonisolated enum JetTurnSource: String, Sendable, Equatable {
    case user
    case schedule
    case autoContinue = "auto_continue"
}

nonisolated enum JetTurnState: String, Sendable, Equatable {
    case queued
    case active
    case completed
    case superseded
    case canceled
    case withdrawn
    case failed
    case outcomeUnknown = "outcome_unknown"
}

nonisolated struct JetTurnSummary: Sendable, Equatable, Identifiable {
    let id: UUID
    let sequence: UInt64
    let state: JetTurnState
}

nonisolated struct JetTurnQueueEntry: Sendable, Equatable, Identifiable {
    let id: UUID
    let sequence: UInt64
    let position: Int
    let source: JetTurnSource
    let state: JetTurnState
    let runID: UUID?
    let withdrawable: Bool

    var targetLabel: String { runID == nil ? "Next Run" : "Current Run" }
}

nonisolated struct JetTurnQueue: Sendable, Equatable {
    static let maximumEntries = 128
    static let maximumPromptBytes = 65_536

    let cursor: UInt64
    let turns: [JetTurnQueueEntry]
}

enum JetRunActivity: String, Sendable, Equatable {
    case working
    case waitingForUser = "waiting_for_user"
    case waitingForApproval = "waiting_for_approval"
    case waitingForAuth = "waiting_for_auth"
    case waitingForQuota = "waiting_for_quota"
    case reconnecting
}

enum JetRunControl: String, Sendable, Equatable, Hashable {
    case interruptTurn = "interrupt_turn"
    case stopRun = "stop_run"
}

enum JetTerminationStage: String, Sendable, Equatable {
    case nativeCancellation = "native_cancellation"
    case interrupt
    case terminate
    case kill
    case unobserved
}

struct JetRunTermination: Sendable, Equatable {
    let control: JetRunControl
    let stage: JetTerminationStage

    var summary: String {
        switch (control, stage) {
        case (.interruptTurn, .nativeCancellation):
            "The active Turn was interrupted. The Run can accept the next Turn."
        case (.interruptTurn, _):
            "Interrupting the Turn required ending the Run process."
        case (.stopRun, .unobserved):
            "Jet sent every stop signal but could not observe the Run ending."
        case (.stopRun, _):
            "The Run stopped and kept its recorded work."
        }
    }
}

struct JetRunExecution: Sendable, Equatable {
    let cursor: UInt64
    let run: JetRunSummary
    let activity: JetRunActivity?
    let needsAttention: Bool
    let termination: JetRunTermination?
}

struct JetRunControlAccepted: Sendable, Equatable {
    let run: JetRunSummary
    let control: JetRunControl
}

enum JetApprovalState: String, Sendable, Equatable {
    case requested
    case allowed
    case denied
    case unavailable
}

struct JetApprovalPresentation: Sendable, Equatable {
    let requestID: String
    let reviewID: UUID?
    let runID: UUID?
    let tool: String
    let action: String
    let target: String
    let scope: String
    let consequence: String
    let rationale: String?
    let state: JetApprovalState
    let canAuthorizeRetry: Bool
}

enum JetConversationFreshness: Sendable, Equatable {
    case loading
    case live
    case cached
    case failed
}

enum JetTimelineKind: String, Sendable, Equatable {
    case user
    case agent
    case activity
    case approval
    case result
}

struct JetTimelineEntry: Sendable, Equatable, Identifiable {
    let id: String
    var kind: JetTimelineKind
    var text: String
    var sequence: UInt64?
    var rawCount: Int
    var approval: JetApprovalPresentation? = nil
}

nonisolated struct JetProjectSummary: Sendable, Equatable, Identifiable {
    let id: UUID
    let root: String

    var name: String {
        URL(fileURLWithPath: root).lastPathComponent.isEmpty
            ? "Project"
            : URL(fileURLWithPath: root).lastPathComponent
    }
}

nonisolated struct JetProjectList: Sendable, Equatable {
    let cursor: UInt64
    let projects: [JetProjectSummary]
}

nonisolated enum JetProjectRegistrability: Sendable, Equatable {
    case registrable(detail: String)
    case unavailable(verdict: String, detail: String)
}

nonisolated struct JetProjectPreview: Sendable, Equatable {
    let root: String
    let registrability: JetProjectRegistrability

    var canRegister: Bool {
        if case .registrable = registrability { return true }
        return false
    }
}

nonisolated struct JetProjectRemovalPreview: Sendable, Equatable, Identifiable {
    let id = UUID()
    let projectID: UUID
    let root: String
    let diskUseBytes: UInt64
    let liveRuns: UInt64
    let schedules: UInt64
    let dirtyFiles: UInt64
    let unpushedCommits: UInt64
    let workspaceCount: Int
    let obstacles: [String]
    let permanentWarning: String
    let binding: JetRawJSON

    var name: String {
        URL(fileURLWithPath: root).lastPathComponent.isEmpty
            ? "Project"
            : URL(fileURLWithPath: root).lastPathComponent
    }
}

enum JetProjectDisposal: Sendable, Equatable {
    case systemTrash
    case permanent(acknowledgedWarning: String)
}

struct JetProjectRemoved: Sendable, Equatable {
    let projectID: UUID
    let root: String
    let disposition: String
}

nonisolated struct JetAccountBindingSummary: Sendable, Equatable, Identifiable {
    let id: UUID
    let provider: String
    let label: String
    let state: String
    let stateLabel: String
}

nonisolated struct JetAccountBindingList: Sendable, Equatable {
    let cursor: UInt64
    let bindings: [JetAccountBindingSummary]
}

nonisolated enum JetPairedClientAccess: String, Sendable, Equatable, Hashable {
    case enabled
    case disabled

    var label: String {
        switch self {
        case .enabled: "Enabled"
        case .disabled: "Disabled"
        }
    }
}

nonisolated struct JetPairedClientSummary: Sendable, Equatable, Identifiable {
    let id: UUID
    let access: JetPairedClientAccess
    let pairedAtUnixMilliseconds: Int64
    let pairingProtocol: String
    let publicKey: String
}

nonisolated enum JetPairingProgress: Sendable, Equatable {
    case offered
    case awaitingConfirmation(clientID: UUID, authenticationString: String)
    case confirmed(clientID: UUID, authenticationString: String)
    case ended(reason: String)

    var authenticationString: String? {
        switch self {
        case let .awaitingConfirmation(_, value), let .confirmed(_, value): value
        case .offered, .ended: nil
        }
    }

    var clientID: UUID? {
        switch self {
        case let .awaitingConfirmation(clientID, _), let .confirmed(clientID, _): clientID
        case .offered, .ended: nil
        }
    }
}

nonisolated struct JetPendingPairing: Sendable, Equatable, Identifiable {
    let id: UUID
    let method: String
    let progress: JetPairingProgress
    let attemptsRemaining: UInt32
    let openedAtUnixMilliseconds: Int64
    let expiresAtUnixMilliseconds: Int64
}

nonisolated struct JetPairingSummary: Sendable, Equatable {
    let cursor: UInt64
    let gate: String
    let pairedClients: Int
    let hasPendingOffer: Bool
    let clients: [JetPairedClientSummary]
    let pending: JetPendingPairing?

    init(
        cursor: UInt64,
        gate: String,
        clients: [JetPairedClientSummary],
        pending: JetPendingPairing?
    ) {
        self.cursor = cursor
        self.gate = gate
        pairedClients = clients.count
        hasPendingOffer = pending != nil
        self.clients = clients
        self.pending = pending
    }

    init(
        cursor: UInt64,
        gate: String,
        pairedClients: Int,
        hasPendingOffer: Bool
    ) {
        self.cursor = cursor
        self.gate = gate
        self.pairedClients = pairedClients
        self.hasPendingOffer = hasPendingOffer
        clients = []
        pending = nil
    }
}

nonisolated enum JetPairingDisclosure: Sendable, Equatable {
    case manualCode(String)
    case qrPayload(String)
    case alreadyDisclosed
}

nonisolated struct JetOpenedPairing: Sendable, Equatable {
    let disclosure: JetPairingDisclosure
    let pending: JetPendingPairing
}

nonisolated struct JetRemotePairingClaim: Sendable, Equatable {
    let endpoint: String
    let offerID: UUID
    let authenticationString: String
    let signingBytes: Data
}

nonisolated struct JetRemotePlaneProfile: Codable, Sendable, Equatable, Identifiable {
    let id: UUID
    var name: String
    let endpoint: String
    var planeID: UUID?
}

struct JetPlanePresentation: Sendable, Equatable, Identifiable {
    let id: UUID
    var name: String
    let endpoint: String?
    let isLocal: Bool
    var planeID: UUID?
    var connection: JetConnectionState
    var snapshot: JetSetupSnapshot?
    var failure: JetPresentationError?
    var conversationCursor: UInt64?

    var capabilityLimitations: [String] {
        guard let capabilities = snapshot?.capabilities else { return [] }
        var limitations = capabilities.degraded
        if capabilities.credentialStore != .available {
            limitations.append(capabilities.credentialStore.label)
        }
        if capabilities.crafts.isEmpty { limitations.append("No Crafts installed") }
        if capabilities.harnesses.isEmpty { limitations.append("No supported Harnesses") }
        for tool in capabilities.externalTools where !tool.availability.isPresent {
            limitations.append("\(tool.tool) unavailable")
        }
        return Array(Set(limitations)).sorted()
    }
}

struct JetPlaneProject: Sendable, Equatable, Identifiable {
    let planeRegistryID: UUID
    let planeName: String
    let project: JetProjectSummary

    var id: String { "\(planeRegistryID.uuidString)-\(project.id.uuidString)" }
}

nonisolated enum JetSetupSection: String, Sendable, Equatable {
    case capabilities
    case projects
    case accounts
    case pairing

    var title: String {
        switch self {
        case .capabilities: "Plane capabilities"
        case .projects: "Projects"
        case .accounts: "Harness access"
        case .pairing: "Remote pairing"
        }
    }
}

nonisolated struct JetSetupIssue: Sendable, Equatable, Identifiable {
    let section: JetSetupSection
    let error: JetPresentationError

    var id: JetSetupSection { section }
}

nonisolated struct JetSetupSnapshot: Sendable, Equatable {
    let status: JetPlaneStatus
    let capabilities: JetCapabilitySummary
    let projects: JetProjectList
    let accounts: JetAccountBindingList
    let pairing: JetPairingSummary
    let issues: [JetSetupIssue]

    init(
        status: JetPlaneStatus,
        capabilities: JetCapabilitySummary,
        projects: JetProjectList,
        accounts: JetAccountBindingList,
        pairing: JetPairingSummary,
        issues: [JetSetupIssue] = []
    ) {
        self.status = status
        self.capabilities = capabilities
        self.projects = projects
        self.accounts = accounts
        self.pairing = pairing
        self.issues = issues
    }

    func issue(for section: JetSetupSection) -> JetSetupIssue? {
        issues.first { $0.section == section }
    }
}

nonisolated enum JetSettingScope: Sendable, Equatable {
    case plane
    case project(UUID)
    case conversation(UUID)
}

nonisolated struct JetSettingCleared: Sendable, Equatable {
    let key: SettingKey
    let scope: JetSettingScope
}

nonisolated enum JetPresentationErrorCategory: String, Sendable {
    case offline
    case invalidInput = "invalid_input"
    case unauthorized
    case conflict
    case unavailable
    case incompatible
    case rateLimited = "rate_limited"
    case notFound = "not_found"
    case outcomeUnknown = "outcome_unknown"
    case internalFailure = "internal"
    case invalidResponse = "invalid_response"
    case overloaded
    case cancelled
}

nonisolated enum JetRecoveryAction: Sendable, Equatable, Identifiable {
    case refreshFile
    case refreshConversation(UUID)
    case refreshRun(UUID)
    case resumeEvents(after: UInt64)

    var id: String {
        switch self {
        case .refreshFile: "refresh-file"
        case let .refreshConversation(id): "refresh-conversation-\(id)"
        case let .refreshRun(id): "refresh-run-\(id)"
        case let .resumeEvents(after): "resume-events-\(after)"
        }
    }

    var label: String {
        switch self {
        case .refreshFile: "Reload File"
        case .refreshConversation: "Refresh Task"
        case .refreshRun: "Refresh Run"
        case .resumeEvents: "Reconnect Activity"
        }
    }
}

nonisolated enum JetRestartMetadata: Sendable, Equatable {
    case cursorExpired(minimumAvailable: UInt64, snapshotRevision: UInt64)
    case cursorAhead(snapshotRevision: UInt64)
    case paginationStale(snapshotRevision: UInt64)

    var requiresEventSnapshot: Bool {
        switch self {
        case .cursorExpired, .cursorAhead: true
        case .paginationStale: false
        }
    }

    var requiresPaginationSnapshot: Bool {
        if case .paginationStale = self { return true }
        return false
    }
}

nonisolated enum JetConflictSafeState: Sendable, Equatable {
    case conversation(id: UUID, revision: UInt64?)
    case run(JetRunSummary)
}

nonisolated struct JetRevisionConflict: Sendable, Equatable {
    let currentRevision: UInt64
    let safeState: JetConflictSafeState
}

nonisolated struct JetPresentationError: Error, Sendable, Equatable {
    let category: JetPresentationErrorCategory
    let code: String
    let message: String
    let retryable: Bool
    let recoveryActions: [JetRecoveryAction]
    let restart: JetRestartMetadata?
    let revisionConflict: JetRevisionConflict?

    init(
        category: JetPresentationErrorCategory,
        code: String,
        message: String,
        retryable: Bool,
        recoveryActions: [JetRecoveryAction] = [],
        restart: JetRestartMetadata? = nil,
        revisionConflict: JetRevisionConflict? = nil
    ) {
        self.category = category
        self.code = code
        self.message = message
        self.retryable = retryable
        self.recoveryActions = recoveryActions
        self.restart = restart
        self.revisionConflict = revisionConflict
    }

    static let offline = JetPresentationError(
        category: .offline,
        code: "transport.offline",
        message: "Jet could not reach this Plane.",
        retryable: true,
        recoveryActions: []
    )

    static let invalidResponse = JetPresentationError(
        category: .invalidResponse,
        code: "protocol.invalid_response",
        message: "The Plane returned a response this version of Jet cannot use.",
        retryable: false,
        recoveryActions: []
    )

    static let overloaded = JetPresentationError(
        category: .overloaded,
        code: "transport.too_many_requests",
        message: "Too many Plane requests are already in progress.",
        retryable: true,
        recoveryActions: []
    )

    static let cancelled = JetPresentationError(
        category: .cancelled,
        code: "request.cancelled",
        message: "The request was cancelled.",
        retryable: false,
        recoveryActions: []
    )

    static func invalidInput(code: String, message: String) -> JetPresentationError {
        JetPresentationError(
            category: .invalidInput,
            code: code,
            message: message,
            retryable: false,
            recoveryActions: []
        )
    }
}

nonisolated enum JetClientFailure: Error, Sendable, Equatable {
    case presentation(JetPresentationError)
    case commandOutcomeUnknown(commandID: UUID)
}

nonisolated struct JetClientConfiguration: Sendable {
    static let protocolVersion: UInt32 = 1
    static let protocolMinor: UInt32 = 43
    static let codec = "json-v1"

    let clientID: UUID
    let reconnectDelays: [Duration]
    let eventPollDelay: Duration
    let connectionTimeout: Duration

    init(
        clientID: UUID,
        reconnectDelays: [Duration] = [.milliseconds(100), .milliseconds(500), .seconds(1)],
        eventPollDelay: Duration = .milliseconds(500),
        connectionTimeout: Duration = .seconds(3)
    ) {
        self.clientID = clientID
        self.reconnectDelays = reconnectDelays
        self.eventPollDelay = eventPollDelay
        self.connectionTimeout = connectionTimeout
    }
}
