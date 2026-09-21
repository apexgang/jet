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

    init(
        clientID: UUID,
        reconnectDelays: [Duration] = [.milliseconds(100), .milliseconds(500), .seconds(1)],
        eventPollDelay: Duration = .milliseconds(500)
    ) {
        self.clientID = clientID
        self.reconnectDelays = reconnectDelays
        self.eventPollDelay = eventPollDelay
    }
}
