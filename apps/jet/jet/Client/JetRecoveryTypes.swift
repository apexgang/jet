import Foundation

nonisolated struct JetTrashEntry: Sendable, Equatable, Identifiable {
    let conversationID: UUID
    let reason: String
    let trashedAt: Date
    let expiresAt: Date

    var id: UUID { conversationID }
    var canRestore: Bool { reason != "plane_transfer" }
}

nonisolated struct JetTrashSnapshot: Sendable, Equatable {
    let cursor: UInt64
    let entries: [JetTrashEntry]
}

nonisolated struct JetRetentionPreview: Sendable, Equatable {
    let conversationID: UUID
    let protections: [String]
    let auditRecords: UInt64
    let trash: JetTrashEntry?

    var blocksForget: Bool {
        protections.contains("active_run") || protections.contains("pending_turn")
    }
}

nonisolated enum JetAutodeleteState: Sendable, Equatable {
    case compiling
    case refused(String)
    case draft(days: UInt32)
    case approved(days: UInt32, at: Date)

    var days: UInt32? {
        switch self {
        case let .draft(days), let .approved(days, _): days
        case .compiling, .refused: nil
        }
    }
}

nonisolated struct JetAutodeleteCandidate: Sendable, Equatable, Identifiable {
    let conversationID: UUID
    let lastActiveAt: Date
    let protections: [String]

    var id: UUID { conversationID }
}

nonisolated struct JetAutodeleteRule: Sendable, Equatable, Identifiable {
    let id: UUID
    let prompt: String
    let scope: String
    let state: JetAutodeleteState
    let candidates: [JetAutodeleteCandidate]
}

nonisolated struct JetAutodeleteSnapshot: Sendable, Equatable {
    let cursor: UInt64
    let rules: [JetAutodeleteRule]
}

nonisolated struct JetRecoverySnapshot: Sendable, Equatable, Identifiable {
    let name: String
    let reason: String
    let takenAt: Date
    let bytes: UInt64

    var id: String { name }
}

nonisolated enum JetAuditIntegrity: Sendable, Equatable {
    case trusted
    case degraded(String)
    case unavailable
}

nonisolated struct JetSystemHealth: Sendable, Equatable {
    let planeID: UUID
    let daemonVersion: String
    let daemonStarts: UInt64
    let daemonStartedAt: Date
    let platform: String
    let capabilitiesAvailable: Bool
    let externalTools: [JetExternalToolSummary]
    let crafts: [JetInstalledCraft]
    let degradedCapabilities: [String]
    let credentialStore: JetCredentialStoreState
    let recoveryState: String?
    let recoveryReason: String?
    let snapshots: [JetRecoverySnapshot]
    let deletionLedger: String?
    let auditIntegrity: JetAuditIntegrity
}

nonisolated struct JetAuditEntry: Sendable, Equatable, Identifiable {
    let id: UUID
    let sequence: UInt64
    let epoch: UInt64
    let recordedAt: Date
    let planeID: UUID
    let actor: String
    let decision: String
    let targetKind: String
    let targetReference: String
    let risk: String
    let outcome: String
}

nonisolated struct JetAuditPage: Sendable, Equatable {
    let cursor: UInt64
    let entries: [JetAuditEntry]
}

nonisolated enum JetRetentionAction: String, Sendable {
    case forget = "forget_conversation"
    case deleteEverywhere = "delete_conversation_everywhere"
}
