import Foundation

/// Presentation-only states used by previews and tests.
///
/// These values never authorize work or replace a Plane snapshot. Production
/// state must still come from the validated Jet protocol.
nonisolated struct DesktopFixtureCorpus: Sendable {
    static let currentFormatVersion = 1

    let protocolVersion: FixtureProtocolVersion
    let scenarios: [DesktopFixtureScenario]

    init(data: Data) throws {
        let unchecked: UncheckedCorpus

        do {
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            // ASVS 1.5.2: decode only the allowlisted fixture value types below.
            unchecked = try decoder.decode(UncheckedCorpus.self, from: data)
        } catch {
            throw DesktopFixtureCorpusError.invalidJSON(String(describing: error))
        }

        guard unchecked.formatVersion == Self.currentFormatVersion else {
            throw DesktopFixtureCorpusError.unsupportedFormatVersion(
                unchecked.formatVersion
            )
        }
        guard unchecked.protocolVersion.major == 1 else {
            throw DesktopFixtureCorpusError.unsupportedProtocolMajor(
                unchecked.protocolVersion.major
            )
        }
        guard unchecked.protocolVersion.minor > 0 else {
            throw DesktopFixtureCorpusError.invalidProtocolMinor(
                unchecked.protocolVersion.minor
            )
        }

        let identifiers = unchecked.scenarios.map(\.id)
        guard Set(identifiers).count == identifiers.count else {
            throw DesktopFixtureCorpusError.duplicateScenarioIdentifier
        }

        let states = Set(unchecked.scenarios.map(\.state))
        let requiredStates = Set(DesktopFixtureState.allCases)
        guard states == requiredStates,
              unchecked.scenarios.count == requiredStates.count
        else {
            throw DesktopFixtureCorpusError.incompleteStateCoverage(
                missing: requiredStates.subtracting(states).map(\.rawValue).sorted(),
                unexpected: states.subtracting(requiredStates).map(\.rawValue).sorted()
            )
        }

        // ASVS 2.2.1 and 2.2.3: enforce fixture limits and related-state rules
        // before any preview or test consumes the corpus.
        for scenario in unchecked.scenarios {
            try Self.validate(scenario)
        }

        protocolVersion = unchecked.protocolVersion
        scenarios = unchecked.scenarios
    }

    func scenario(for state: DesktopFixtureState) -> DesktopFixtureScenario? {
        scenarios.first { $0.state == state }
    }

    private static func validate(_ scenario: DesktopFixtureScenario) throws {
        guard !scenario.id.isEmpty, !scenario.summary.isEmpty else {
            throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
        }
        guard isCanonicalDecimal(scenario.plane.cursor) else {
            throw DesktopFixtureCorpusError.invalidCursor(scenario.id)
        }
        guard UUID(uuidString: scenario.plane.id) != nil,
              scenario.project.map({ UUID(uuidString: $0.id) != nil }) ?? true,
              scenario.conversation.map({ UUID(uuidString: $0.id) != nil }) ?? true,
              scenario.run.map({ UUID(uuidString: $0.id) != nil }) ?? true,
              scenario.queue.allSatisfy({ UUID(uuidString: $0.id) != nil })
        else {
            throw DesktopFixtureCorpusError.invalidIdentifier(scenario.id)
        }

        let queries = scenario.contract.queries
        let commands = scenario.contract.commands
        guard Set(queries).count == queries.count,
              Set(commands).count == commands.count,
              Set(scenario.contract.errorCategories).count
                == scenario.contract.errorCategories.count,
              Set(scenario.contract.backendDependencies).count
                == scenario.contract.backendDependencies.count,
              scenario.contract.backendDependencies.allSatisfy({ !$0.isEmpty })
        else {
            throw DesktopFixtureCorpusError.duplicateContractReference(scenario.id)
        }

        guard scenario.queue.count <= 128 else {
            throw DesktopFixtureCorpusError.queueLimitExceeded(scenario.id)
        }
        let expectedPositions = scenario.queue.indices.map { $0 + 1 }
        guard scenario.queue.map(\.position) == expectedPositions else {
            throw DesktopFixtureCorpusError.invalidQueuePositions(scenario.id)
        }
        guard scenario.queue.allSatisfy({ !$0.summary.isEmpty }),
              Set(scenario.timeline.map(\.id)).count == scenario.timeline.count,
              scenario.timeline.allSatisfy({ !$0.id.isEmpty && !$0.text.isEmpty }),
              Set(scenario.capabilities.harnesses).count
                == scenario.capabilities.harnesses.count,
              scenario.capabilities.harnesses.allSatisfy({ !$0.isEmpty }),
              Set(scenario.capabilities.externalTools.map(\.tool)).count
                == scenario.capabilities.externalTools.count,
              Set(scenario.capabilities.missingFeatures).count
                == scenario.capabilities.missingFeatures.count,
              scenario.capabilities.missingFeatures.allSatisfy({ !$0.isEmpty })
        else {
            throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
        }

        if let action = scenario.primaryAction {
            guard !action.id.isEmpty, !action.label.isEmpty else {
                throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
            }
            if action.availability == .disabled {
                guard action.reason?.isEmpty == false else {
                    throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
                }
            }
        }

        if let run = scenario.run {
            let activeLifecycles: Set<DesktopRunLifecycle> = [
                .created, .starting, .active, .stopping,
            ]
            if activeLifecycles.contains(run.lifecycle), run.activity == nil {
                throw DesktopFixtureCorpusError.missingRunActivity(scenario.id)
            }
            if !activeLifecycles.contains(run.lifecycle), run.activity != nil {
                throw DesktopFixtureCorpusError.terminalRunHasActivity(scenario.id)
            }
        }

        switch scenario.state {
        case .firstLaunch:
            guard scenario.project == nil,
                  scenario.conversation == nil,
                  scenario.run == nil
            else {
                throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
            }
        case .ready:
            guard scenario.plane.connection == .online,
                  scenario.project != nil,
                  scenario.conversation == nil,
                  scenario.run == nil
            else {
                throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
            }
        case .active:
            try requireConversationAndRun(scenario, lifecycle: .active)
        case .queued:
            try requireConversationAndRun(scenario, lifecycle: .active)
            guard !scenario.queue.isEmpty else {
                throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
            }
        case .approval:
            try requireConversationAndRun(scenario, lifecycle: .active)
            guard scenario.run?.activity == .waitingForApproval,
                  scenario.timeline.contains(where: { $0.kind == .approval })
            else {
                throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
            }
        case .completed:
            try requireConversationAndRun(scenario, lifecycle: .completed)
        case .offline:
            try requireConversationAndRun(scenario, lifecycle: .active)
            guard scenario.plane.connection == .offline else {
                throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
            }
        case .staleCursor:
            try requireConversationAndRun(scenario, lifecycle: .active)
            guard scenario.plane.connection == .recovering,
                  scenario.contract.restart == .cursorExpired,
                  scenario.notice?.action == .refreshSnapshot
            else {
                throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
            }
        case .denied:
            try requireConversationAndRun(scenario, lifecycle: .active)
            guard scenario.contract.errorCategories.contains(.unauthorized) else {
                throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
            }
        case .unsupported:
            guard !scenario.capabilities.missingFeatures.isEmpty,
                  scenario.primaryAction?.availability == .disabled
            else {
                throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
            }
        case .recovery:
            try requireConversationAndRun(scenario, lifecycle: .lost)
            guard scenario.notice?.action == .inspectRecovery else {
                throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
            }
        }
    }

    private static func requireConversationAndRun(
        _ scenario: DesktopFixtureScenario,
        lifecycle: DesktopRunLifecycle
    ) throws {
        guard scenario.conversation != nil,
              scenario.run?.lifecycle == lifecycle
        else {
            throw DesktopFixtureCorpusError.invalidScenario(scenario.id)
        }
    }

    private static func isCanonicalDecimal(_ value: String) -> Bool {
        guard !value.isEmpty else { return false }
        if value == "0" { return true }
        return value.first != "0"
            && value.utf8.allSatisfy { (48...57).contains($0) }
    }
}

private nonisolated struct UncheckedCorpus: Decodable {
    let formatVersion: Int
    let protocolVersion: FixtureProtocolVersion
    let scenarios: [DesktopFixtureScenario]
}

nonisolated struct FixtureProtocolVersion: Decodable, Equatable, Sendable {
    let major: Int
    let minor: Int
}

nonisolated struct DesktopFixtureScenario: Decodable, Equatable, Sendable {
    let id: String
    let state: DesktopFixtureState
    let summary: String
    let plane: DesktopPlaneFixture
    let project: DesktopProjectFixture?
    let conversation: DesktopConversationFixture?
    let run: DesktopRunFixture?
    let queue: [DesktopTurnFixture]
    let timeline: [DesktopTimelineEntryFixture]
    let capabilities: DesktopCapabilityFixture
    let notice: DesktopNoticeFixture?
    let primaryAction: DesktopActionFixture?
    let contract: DesktopContractFixture
}

nonisolated enum DesktopFixtureState: String, CaseIterable, Decodable, Hashable, Sendable {
    case firstLaunch = "first_launch"
    case ready
    case active
    case queued
    case approval
    case completed
    case offline
    case staleCursor = "stale_cursor"
    case denied
    case unsupported
    case recovery
}

nonisolated struct DesktopPlaneFixture: Decodable, Equatable, Sendable {
    let id: String
    let name: String
    let connection: DesktopConnectionState
    let cursor: String
}

nonisolated enum DesktopConnectionState: String, Decodable, Equatable, Sendable {
    case connecting
    case online
    case offline
    case recovering
}

nonisolated struct DesktopProjectFixture: Decodable, Equatable, Sendable {
    let id: String
    let name: String
}

nonisolated struct DesktopConversationFixture: Decodable, Equatable, Sendable {
    let id: String
    let title: String
}

nonisolated struct DesktopRunFixture: Decodable, Equatable, Sendable {
    let id: String
    let lifecycle: DesktopRunLifecycle
    let activity: DesktopRunActivity?
}

nonisolated enum DesktopRunLifecycle: String, Decodable, Hashable, Sendable {
    case created
    case starting
    case active
    case stopping
    case completed
    case failed
    case canceled
    case lost
}

nonisolated enum DesktopRunActivity: String, Decodable, Equatable, Sendable {
    case working
    case waitingForUser = "waiting_for_user"
    case waitingForApproval = "waiting_for_approval"
    case waitingForAuth = "waiting_for_auth"
    case waitingForQuota = "waiting_for_quota"
    case reconnecting
}

nonisolated struct DesktopTurnFixture: Decodable, Equatable, Sendable {
    let id: String
    let position: Int
    let state: DesktopTurnState
    let summary: String
}

nonisolated enum DesktopTurnState: String, Decodable, Equatable, Sendable {
    case queued
    case active
    case completed
    case superseded
    case canceled
    case withdrawn
    case failed
    case outcomeUnknown = "outcome_unknown"
}

nonisolated struct DesktopTimelineEntryFixture: Decodable, Equatable, Sendable {
    let id: String
    let kind: DesktopTimelineEntryKind
    let text: String
}

nonisolated enum DesktopTimelineEntryKind: String, Decodable, Equatable, Sendable {
    case user
    case agent
    case activity
    case approval
    case result
    case recovery
}

nonisolated struct DesktopCapabilityFixture: Decodable, Equatable, Sendable {
    let harnesses: [String]
    let externalTools: [DesktopExternalToolFixture]
    let missingFeatures: [String]
}

nonisolated struct DesktopExternalToolFixture: Decodable, Equatable, Sendable {
    let tool: DesktopExternalTool
    let availability: DesktopToolAvailability
}

nonisolated enum DesktopExternalTool: String, Decodable, Hashable, Sendable {
    case git
    case gitLFS = "git-lfs"
    case ssh
    case tailscale
}

nonisolated enum DesktopToolAvailability: String, Decodable, Equatable, Sendable {
    case present
    case missing
}

nonisolated struct DesktopNoticeFixture: Decodable, Equatable, Sendable {
    let tone: DesktopNoticeTone
    let title: String
    let message: String
    let action: DesktopFixtureAction?
}

nonisolated enum DesktopNoticeTone: String, Decodable, Equatable, Sendable {
    case informational
    case warning
    case critical
}

nonisolated enum DesktopFixtureAction: String, Decodable, Equatable, Sendable {
    case reconnect
    case refreshSnapshot = "refresh_snapshot"
    case openSettings = "open_settings"
    case reviewApproval = "review_approval"
    case inspectRecovery = "inspect_recovery"
}

nonisolated struct DesktopActionFixture: Decodable, Equatable, Sendable {
    let id: String
    let label: String
    let availability: DesktopActionAvailability
    let reason: String?
}

nonisolated enum DesktopActionAvailability: String, Decodable, Equatable, Sendable {
    case enabled
    case disabled
}

nonisolated struct DesktopContractFixture: Decodable, Equatable, Sendable {
    let queries: [DesktopFixtureQuery]
    let commands: [DesktopFixtureCommand]
    let errorCategories: [DesktopFixtureErrorCategory]
    let restart: DesktopFixtureRestart?
    let backendDependencies: [String]
}

nonisolated enum DesktopFixtureQuery: String, Decodable, Hashable, Sendable {
    case status
    case capabilities
    case projects
    case accountBindings = "account_bindings"
    case conversations
    case conversation
    case events
    case runExecution = "run_execution"
    case turnQueue = "turn_queue"
    case changeDiff = "change_diff"
    case gitDeliveries = "git_deliveries"
    case orphanedExecutions = "orphaned_executions"
}

nonisolated enum DesktopFixtureCommand: String, Decodable, Hashable, Sendable {
    case createConversation = "create_conversation"
    case startRun = "start_run"
    case submitTurn = "submit_turn"
    case withdrawTurn = "withdraw_turn"
    case interruptTurn = "interrupt_turn"
    case stopRun = "stop_run"
    case authorizeApprovalRetry = "authorize_approval_retry"
    case deliverGit = "deliver_git"
    case resolveExecution = "resolve_execution"
}

nonisolated enum DesktopFixtureErrorCategory: String, Decodable, Hashable, Sendable {
    case invalidInput = "invalid_input"
    case unauthorized
    case conflict
    case unavailable
    case incompatible
    case rateLimited = "rate_limited"
    case notFound = "not_found"
    case outcomeUnknown = "outcome_unknown"
    case `internal`
}

nonisolated enum DesktopFixtureRestart: String, Decodable, Equatable, Sendable {
    case cursorExpired = "cursor_expired"
    case cursorAhead = "cursor_ahead"
    case paginationStale = "pagination_stale"
}

nonisolated enum DesktopFixtureCorpusError: Error, Equatable, LocalizedError {
    case invalidJSON(String)
    case unsupportedFormatVersion(Int)
    case unsupportedProtocolMajor(Int)
    case invalidProtocolMinor(Int)
    case duplicateScenarioIdentifier
    case incompleteStateCoverage(missing: [String], unexpected: [String])
    case invalidScenario(String)
    case invalidIdentifier(String)
    case invalidCursor(String)
    case duplicateContractReference(String)
    case queueLimitExceeded(String)
    case invalidQueuePositions(String)
    case missingRunActivity(String)
    case terminalRunHasActivity(String)

    var errorDescription: String? {
        switch self {
        case let .invalidJSON(detail):
            "The fixture corpus is not valid JSON: \(detail)"
        case let .unsupportedFormatVersion(version):
            "Unsupported fixture format version \(version)."
        case let .unsupportedProtocolMajor(major):
            "Unsupported fixture protocol major \(major)."
        case let .invalidProtocolMinor(minor):
            "Invalid fixture protocol minor \(minor)."
        case .duplicateScenarioIdentifier:
            "Fixture scenario identifiers must be unique."
        case let .incompleteStateCoverage(missing, unexpected):
            "Fixture state coverage differs. Missing: \(missing); unexpected: \(unexpected)."
        case let .invalidScenario(identifier):
            "Fixture scenario \(identifier) has inconsistent state."
        case let .invalidIdentifier(identifier):
            "Fixture scenario \(identifier) has an invalid protocol identifier."
        case let .invalidCursor(identifier):
            "Fixture scenario \(identifier) has a non-canonical cursor."
        case let .duplicateContractReference(identifier):
            "Fixture scenario \(identifier) repeats a contract reference."
        case let .queueLimitExceeded(identifier):
            "Fixture scenario \(identifier) exceeds the 128-turn queue limit."
        case let .invalidQueuePositions(identifier):
            "Fixture scenario \(identifier) has non-contiguous queue positions."
        case let .missingRunActivity(identifier):
            "Fixture scenario \(identifier) has a live Run without activity."
        case let .terminalRunHasActivity(identifier):
            "Fixture scenario \(identifier) gives a terminal Run live activity."
        }
    }
}
