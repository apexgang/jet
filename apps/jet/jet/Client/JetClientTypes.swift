import Foundation

extension SettingKey: Sendable {}

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

struct JetPlaneStatus: Sendable, Equatable {
    let cursor: UInt64?
    let planeID: UUID
    let daemonStarts: UInt64
    let startedAtUnixMilliseconds: Int64
    let coreVersion: String
    let security: JetRawJSON?
    let recovery: JetRawJSON?
}

struct JetRawJSON: Sendable, Equatable {
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

enum JetCredentialStoreState: String, Sendable, Equatable {
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

struct JetCapabilitySummary: Sendable, Equatable {
    let coreVersion: String
    let platform: String
    let harnesses: [String]
    let crafts: [JetInstalledCraft]
    let credentialStore: JetCredentialStoreState
    let degraded: [String]

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

struct JetInstalledCraft: Sendable, Equatable, Identifiable {
    let id: String
    let version: String
    let harnesses: [String]
}

struct JetConversationSummary: Sendable, Equatable, Identifiable {
    let id: UUID
    let title: String
    let createdAtUnixMilliseconds: Int64
    let projectID: UUID?
}

struct JetConversationPage: Sendable, Equatable {
    let cursor: UInt64
    let conversations: [JetConversationSummary]
    let nextPage: UUID?
}

enum JetRunLifecycle: String, Sendable, Equatable {
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

struct JetRunSummary: Sendable, Equatable, Identifiable {
    let id: UUID
    let lifecycle: JetRunLifecycle
    let title: String
    let createdAtUnixMilliseconds: Int64
    let endedAtUnixMilliseconds: Int64?
}

struct JetConversationSnapshot: Sendable, Equatable {
    let cursor: UInt64
    let conversation: JetConversationSummary
    let workspaceRoot: String?
    let runs: [JetRunSummary]
}

enum JetSearchField: String, Sendable, Equatable {
    case name
    case path
    case branch
}

struct JetSearchHit: Sendable, Equatable, Identifiable {
    let conversationID: UUID
    let sequence: UInt64
    let field: JetSearchField
    let excerpt: String

    var id: String { "\(conversationID.uuidString)-\(sequence)" }
}

struct JetSearchResult: Sendable, Equatable {
    let cursor: UInt64
    let indexedThrough: UInt64
    let hits: [JetSearchHit]
}

struct JetTurnSummary: Sendable, Equatable {
    let id: UUID
    let sequence: UInt64
    let state: String
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
    case result
}

struct JetTimelineEntry: Sendable, Equatable, Identifiable {
    let id: String
    var kind: JetTimelineKind
    var text: String
    var sequence: UInt64?
    var rawCount: Int
}

struct JetProjectSummary: Sendable, Equatable, Identifiable {
    let id: UUID
    let root: String

    var name: String {
        URL(fileURLWithPath: root).lastPathComponent.isEmpty
            ? "Project"
            : URL(fileURLWithPath: root).lastPathComponent
    }
}

struct JetProjectList: Sendable, Equatable {
    let cursor: UInt64
    let projects: [JetProjectSummary]
}

enum JetProjectRegistrability: Sendable, Equatable {
    case registrable(detail: String)
    case unavailable(verdict: String, detail: String)
}

struct JetProjectPreview: Sendable, Equatable {
    let root: String
    let registrability: JetProjectRegistrability

    var canRegister: Bool {
        if case .registrable = registrability { return true }
        return false
    }
}

struct JetProjectRemovalPreview: Sendable, Equatable, Identifiable {
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

struct JetAccountBindingSummary: Sendable, Equatable, Identifiable {
    let id: UUID
    let provider: String
    let label: String
    let state: String
    let stateLabel: String
}

struct JetAccountBindingList: Sendable, Equatable {
    let cursor: UInt64
    let bindings: [JetAccountBindingSummary]
}

struct JetPairingSummary: Sendable, Equatable {
    let cursor: UInt64
    let gate: String
    let pairedClients: Int
    let hasPendingOffer: Bool
}

enum JetSetupSection: String, Sendable, Equatable {
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

struct JetSetupIssue: Sendable, Equatable, Identifiable {
    let section: JetSetupSection
    let error: JetPresentationError

    var id: JetSetupSection { section }
}

struct JetSetupSnapshot: Sendable, Equatable {
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

enum JetSettingScope: Sendable, Equatable {
    case plane
    case project(UUID)
    case conversation(UUID)
}

struct JetSettingCleared: Sendable, Equatable {
    let key: SettingKey
    let scope: JetSettingScope
}

enum JetPresentationErrorCategory: String, Sendable {
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

struct JetPresentationError: Error, Sendable, Equatable {
    let category: JetPresentationErrorCategory
    let code: String
    let message: String
    let retryable: Bool
    let recoveryActions: [JetRawJSON]

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

enum JetClientFailure: Error, Sendable, Equatable {
    case presentation(JetPresentationError)
    case commandOutcomeUnknown(commandID: UUID)
}

struct JetClientConfiguration: Sendable {
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
