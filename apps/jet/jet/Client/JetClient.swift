import Foundation

typealias JetTransportFactory = @Sendable () -> any JetByteTransport

actor JetClient {
    private struct PendingReply {
        var continuation: CheckedContinuation<Data, Error>?
        var sent: Bool
        let cancellationFailure: JetClientFailure
    }

    private struct TerminalReply {
        let terminalID: UUID
        var nextOffset: UInt64
        var continuation: AsyncThrowingStream<JetTerminalEvent, Error>.Continuation?
        let attachRequestID: UInt64
        var resizeRequestIDs: Set<UInt64>
        var attached: Bool
        var active: Bool
    }

    private static let maximumInFlightRequests = 256
    private static let maximumTerminalStreams = 16
    private static let maximumTerminalCredit: UInt64 = 16 * 1_024 * 1_024
    private static let protocolVersion = JetClientConfiguration.protocolVersion
    private static let protocolMinor = JetClientConfiguration.protocolMinor
    private static let codec = JetClientConfiguration.codec

    private let configuration: JetClientConfiguration
    private let schema: JetWireSchema
    private let makeTransport: JetTransportFactory
    private let remoteSigner: (any JetConnectionSigning)?

    private var transport: (any JetByteTransport)?
    private var negotiation: JetNegotiation?
    private var state: JetConnectionState = .disconnected
    private var connectionTask: Task<Void, Error>?
    private var readerTask: Task<Void, Never>?
    private var connectionObservers: [UUID: AsyncStream<JetConnectionState>.Continuation] = [:]
    private var generation: UInt64 = 0
    private var pending: [UInt32: PendingReply] = [:]
    private var nextRequestID: UInt64 = 1
    private var nextStreamID: UInt32 = 1
    private var terminalReplies: [UInt32: TerminalReply] = [:]

    init(
        configuration: JetClientConfiguration,
        schema: JetWireSchema,
        makeTransport: @escaping JetTransportFactory,
        remoteSigner: (any JetConnectionSigning)? = nil
    ) {
        self.configuration = configuration
        self.schema = schema
        self.makeTransport = makeTransport
        self.remoteSigner = remoteSigner
    }

#if os(macOS)
    static func connectLocal(
        socketURL: URL,
        configuration: JetClientConfiguration
    ) async throws -> JetClient {
        let schema = try JetWireSchema.bundled()
        let client = JetClient(
            configuration: configuration,
            schema: schema,
            makeTransport: { JetUnixSocketTransport(socketURL: socketURL) }
        )
        try await client.connect()
        return client
    }

    static func connectRemote(
        endpoint: String,
        configuration: JetClientConfiguration,
        signer: any JetConnectionSigning
    ) async throws -> JetClient {
        let schema = try JetWireSchema.bundled()
        let validatedEndpoint = try JetSSHEndpoint.validated(endpoint)
        let client = JetClient(
            configuration: configuration,
            schema: schema,
            makeTransport: { JetSSHTransport(validatedEndpoint: validatedEndpoint) },
            remoteSigner: signer
        )
        try await client.connect()
        return client
    }

    static func defaultLocalSocketURL(homeDirectory: URL) -> URL {
        homeDirectory
            .appending(path: ".jet/runtime/jetd.sock", directoryHint: .notDirectory)
    }
#endif

    func currentState() -> JetConnectionState {
        state
    }

    func connectionStates() -> AsyncStream<JetConnectionState> {
        let observerID = UUID()
        let (stream, continuation) = AsyncStream.makeStream(
            of: JetConnectionState.self,
            bufferingPolicy: .bufferingNewest(1)
        )
        connectionObservers[observerID] = continuation
        continuation.yield(state)
        continuation.onTermination = { _ in
            Task { await self.removeConnectionObserver(observerID) }
        }
        return stream
    }

    func connect() async throws {
        try await ensureConnected()
    }

    func disconnect() async {
        await closeConnection(
            failure: .presentation(.offline),
            publish: .disconnected
        )
    }

    func status() async throws -> JetPlaneStatus {
        var attempt = 0
        while true {
            do {
                try await ensureConnected()
                let requestID = takeRequestID()
                let payload = try encodeClientMessage([
                    "kind": "query",
                    "id": NSNumber(value: requestID),
                    "query": ["type": "status"],
                ])
                let response = try await exchange(
                    payload,
                    cancellationFailure: .presentation(.cancelled)
                )
                return try decodeStatus(response, requestID: requestID)
            } catch {
                guard try await prepareReadRetry(error, attempt: attempt) else {
                    throw normalized(error)
                }
                attempt += 1
            }
        }
    }

    func setupSnapshot() async throws -> JetSetupSnapshot {
        let status = try await status()
        var issues: [JetSetupIssue] = []

        let capabilities: JetCapabilitySummary
        do {
            capabilities = try await self.capabilities(.fresh)
        } catch {
            issues.append(setupIssue(.capabilities, error: error))
            capabilities = JetCapabilitySummary(
                coreVersion: status.coreVersion,
                platform: "Platform unavailable",
                externalTools: [],
                harnesses: [],
                crafts: [],
                credentialStore: .unavailable,
                degraded: []
            )
        }

        let projects: JetProjectList
        do {
            projects = try await self.projects()
        } catch {
            issues.append(setupIssue(.projects, error: error))
            projects = JetProjectList(cursor: status.cursor ?? 0, projects: [])
        }

        let accounts: JetAccountBindingList
        do {
            accounts = try await accountBindings(.lastObserved)
        } catch {
            issues.append(setupIssue(.accounts, error: error))
            accounts = JetAccountBindingList(cursor: status.cursor ?? 0, bindings: [])
        }

        let pairing: JetPairingSummary
        do {
            pairing = try await self.pairing()
        } catch {
            issues.append(setupIssue(.pairing, error: error))
            pairing = JetPairingSummary(
                cursor: status.cursor ?? 0,
                gate: "closed",
                pairedClients: 0,
                hasPendingOffer: false
            )
        }
        return JetSetupSnapshot(
            status: status,
            capabilities: capabilities,
            projects: projects,
            accounts: accounts,
            pairing: pairing,
            issues: issues
        )
    }

    func capabilities(
        _ observation: JetCapabilityObservation
    ) async throws -> JetCapabilitySummary {
        let (data, requestID) = try await sendQuery([
            "type": "capabilities",
            "observation": wireObservation(observation),
        ])
        return try decodeCapabilities(data, requestID: requestID)
    }

    func projects() async throws -> JetProjectList {
        let (data, requestID) = try await sendQuery(["type": "projects"])
        return try decodeProjects(data, requestID: requestID)
    }

    func conversations() async throws -> JetConversationPage {
        let (data, requestID) = try await sendQuery(["type": "conversations"])
        return try decodeConversations(data, requestID: requestID)
    }

    func nextConversations(_ cursor: UUID) async throws -> JetConversationPage {
        let (data, requestID) = try await sendQuery([
            "type": "next_conversations",
            "cursor": cursor.uuidString.lowercased(),
        ])
        return try decodeConversations(data, requestID: requestID)
    }

    func conversation(_ conversationID: UUID) async throws -> JetConversationSnapshot {
        let (data, requestID) = try await sendQuery([
            "type": "conversation",
            "conversation_id": conversationID.uuidString.lowercased(),
        ])
        return try decodeConversation(data, requestID: requestID)
    }

    func searchConversations(_ text: String) async throws -> JetSearchResult {
        let terms = text.split(whereSeparator: { $0.isWhitespace })
        guard !terms.isEmpty, terms.count <= 16, text.utf8.count <= 256 else {
            throw JetClientFailure.presentation(
                .invalidInput(
                    code: "search.invalid_text",
                    message: "Search for 1 to 16 terms using at most 256 UTF-8 bytes."
                )
            )
        }
        let (data, requestID) = try await sendQuery([
            "type": "search",
            "text": text,
        ])
        return try decodeSearch(data, requestID: requestID)
    }

    func createConversation(
        projectID: UUID,
        commandID: UUID = UUID()
    ) async throws -> JetConversationSummary {
        // ASVS 2.2.2 and 8.3.1: the typed adapter exposes only Jet's
        // managed-Workspace choice. The Plane revalidates Project authority.
        let (data, requestID) = try await sendCommand(
            [
                "type": "create_conversation",
                "retention": "retain",
                "working_tree": [
                    "kind": "workspace",
                    "project_id": projectID.uuidString.lowercased(),
                ],
            ],
            commandID: commandID
        )
        return try decodeCreatedConversation(data, requestID: requestID)
    }

    func startRun(
        conversationID: UUID,
        craft: String,
        prompt: String,
        commandID: UUID = UUID()
    ) async throws -> JetRunSummary {
        try validatePrompt(prompt)
        guard isSafeToken(craft, maximumBytes: 128) else {
            throw JetClientFailure.presentation(
                .invalidInput(
                    code: "craft.identifier_invalid",
                    message: "Choose an installed Craft."
                )
            )
        }
        let capabilities = try await capabilities(.fresh)
        guard capabilities.crafts.contains(where: { $0.id == craft }) else {
            throw JetClientFailure.presentation(
                .invalidInput(
                    code: "craft.unavailable",
                    message: "Choose a Craft that is installed on this Plane."
                )
            )
        }
        let (data, requestID) = try await sendCommand(
            [
                "type": "start_run",
                "conversation_id": conversationID.uuidString.lowercased(),
                "craft": craft,
                "prompt": prompt,
            ],
            commandID: commandID
        )
        return try decodeCreatedRun(data, requestID: requestID)
    }

    func submitTurn(
        conversationID: UUID,
        prompt: String,
        commandID: UUID = UUID()
    ) async throws -> JetTurnSummary {
        try validatePrompt(prompt)
        let (data, requestID) = try await sendCommand(
            [
                "type": "submit_turn",
                "conversation_id": conversationID.uuidString.lowercased(),
                "source": "user",
                "prompt": prompt,
            ],
            commandID: commandID
        )
        return try decodeAdmittedTurn(data, requestID: requestID)
    }

    func turnQueue(conversationID: UUID) async throws -> JetTurnQueue {
        let (data, requestID) = try await sendQuery([
            "type": "turn_queue",
            "conversation_id": conversationID.uuidString.lowercased(),
        ])
        return try decodeTurnQueue(data, requestID: requestID)
    }

    func withdrawTurn(
        conversationID: UUID,
        turnID: UUID,
        commandID: UUID
    ) async throws -> JetTurnSummary {
        let (data, requestID) = try await sendCommand(
            [
                "type": "withdraw_turn",
                "conversation_id": conversationID.uuidString.lowercased(),
                "turn_id": turnID.uuidString.lowercased(),
            ],
            commandID: commandID
        )
        return try decodeWithdrawnTurn(data, requestID: requestID)
    }

    func runExecution(runID: UUID) async throws -> JetRunExecution {
        let (data, requestID) = try await sendQuery([
            "type": "run_execution",
            "run_id": runID.uuidString.lowercased(),
        ])
        return try decodeRunExecution(data, requestID: requestID)
    }

    func controlRun(
        runID: UUID,
        control: JetRunControl,
        commandID: UUID
    ) async throws -> JetRunControlAccepted {
        let (data, requestID) = try await sendCommand(
            ["type": control.rawValue, "run_id": runID.uuidString.lowercased()],
            commandID: commandID
        )
        return try decodeRunControlAccepted(
            data,
            requestID: requestID,
            expectedControl: control
        )
    }

    func authorizeApprovalRetry(
        runID: UUID,
        reviewID: UUID,
        commandID: UUID
    ) async throws {
        let (data, requestID) = try await sendCommand(
            [
                "type": "authorize_approval_retry",
                "run_id": runID.uuidString.lowercased(),
                "review_id": reviewID.uuidString.lowercased(),
            ],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "approval_retry_authorized"
        )
        guard uuid(result["review_id"]) == reviewID else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    func previewProject(path: String) async throws -> JetProjectPreview {
        guard !path.isEmpty,
              path.utf8.count <= 4_096,
              path.first == "/",
              !path.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
        else {
            throw JetClientFailure.presentation(
                .invalidInput(
                    code: "project.path_invalid",
                    message: "Choose an absolute folder path without control characters."
                )
            )
        }
        let (data, requestID) = try await sendQuery([
            "type": "preview_project",
            "path": path,
            "observation": wireObservation(.fresh),
        ])
        return try decodeProjectPreview(data, requestID: requestID)
    }

    func registerProject(
        preview: JetProjectPreview,
        commandID: UUID = UUID()
    ) async throws -> JetProjectSummary {
        guard preview.canRegister else {
            throw JetClientFailure.presentation(
                .invalidInput(
                    code: "project.preview_not_registrable",
                    message: "Review a registrable Project folder before adding it."
                )
            )
        }
        let (data, requestID) = try await sendCommand(
            ["type": "register_project", "path": preview.root],
            commandID: commandID
        )
        return try decodeRegisteredProject(data, requestID: requestID)
    }

    func accountBindings(
        _ observation: JetCapabilityObservation
    ) async throws -> JetAccountBindingList {
        let (data, requestID) = try await sendQuery([
            "type": "account_bindings",
            "observation": wireObservation(observation),
        ])
        return try decodeAccountBindings(data, requestID: requestID)
    }

    func unbindHarnessAccount(
        _ bindingID: UUID,
        commandID: UUID = UUID()
    ) async throws {
        try await requireProtocolMinor(4, feature: "Harness accounts")
        let (data, requestID) = try await sendCommand(
            [
                "type": "unbind_account",
                "binding_id": bindingID.uuidString.lowercased(),
            ],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "account_unbound"
        )
        guard uuid(result["binding_id"]) == bindingID else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    func bindHarnessAccount(
        _ option: JetAuthProvider,
        commandID: UUID = UUID()
    ) async throws -> JetAccountBindingSummary {
        guard ["openai", "anthropic"].contains(option.provider) else {
            throw JetClientFailure.presentation(
                .invalidInput(
                    code: "account.provider_unavailable",
                    message: "That Harness is not available on this Plane."
                )
            )
        }
        // ASVS 13.3.1 and 14.3.3: the request carries no credential.
        // Authentication stays in the Harness environment.
        let (data, requestID) = try await sendCommand(
            [
                "type": "bind_account",
                "provider": option.provider,
                "label": option.label,
                "credential_source": ["source": "harness_native"],
            ],
            commandID: commandID
        )
        return try decodeBoundAccount(data, requestID: requestID)
    }

    func usage() async throws -> JetUsageSnapshot {
        try await requireProtocolMinor(29, feature: "Usage")
        let (data, requestID) = try await sendQuery([
            "type": "usage",
            "selection": ["scope": "plane"],
        ])
        return try decodeUsage(data, requestID: requestID)
    }

    func usageHistory(
        fromUnixMilliseconds: Int64,
        untilUnixMilliseconds: Int64
    ) async throws -> JetUsageHistorySnapshot {
        guard fromUnixMilliseconds < untilUnixMilliseconds else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "usage.range_inverted",
                message: "Choose a Usage history range with an end after its start."
            ))
        }
        try await requireProtocolMinor(43, feature: "Usage history")
        let (data, requestID) = try await sendQuery([
            "type": "usage_history",
            "selection": ["scope": "plane"],
            "range": [
                "from_unix_ms": fromUnixMilliseconds,
                "until_unix_ms": untilUnixMilliseconds,
            ],
            "resolution": "day",
        ])
        return try decodeUsageHistory(data, requestID: requestID)
    }

    func extensionCatalog(craftID: String) async throws -> JetExtensionCatalogSummary {
        try validateExtensionToken(craftID, field: "Craft")
        try await requireProtocolMinor(28, feature: "Harness extensions")
        let (data, requestID) = try await sendQuery([
            "type": "extension_catalog",
            "craft_id": craftID,
        ])
        return try decodeExtensionCatalog(
            data,
            requestID: requestID,
            expectedCraftID: craftID
        )
    }

    func inspectExtension(
        craftID: String,
        extensionID: String,
        action: JetExtensionAction
    ) async throws -> JetExtensionProposal {
        try validateExtensionToken(craftID, field: "Craft")
        try validateExtensionToken(extensionID, field: "Extension")
        try await requireProtocolMinor(28, feature: "Harness extensions")
        let (data, requestID) = try await sendQuery([
            "type": "inspect_extension",
            "craft_id": craftID,
            "extension_id": extensionID,
        ])
        return JetExtensionProposal(
            catalog: try decodeExtensionCatalog(
                data,
                requestID: requestID,
                expectedCraftID: craftID
            ),
            extensionID: extensionID,
            action: action
        )
    }

    func changeExtension(
        _ proposal: JetExtensionProposal,
        commandID: UUID = UUID()
    ) async throws -> UUID {
        try validateExtensionToken(proposal.catalog.craftID, field: "Craft")
        try validateExtensionToken(proposal.extensionID, field: "Extension")
        try await requireProtocolMinor(28, feature: "Harness extensions")
        // ASVS 2.2.2 and 8.3.1: this is the exact inspected catalog and
        // closed action vocabulary. jetd revalidates both before mutation.
        let (data, requestID) = try await sendCommand(
            [
                "type": "change_extension",
                "confirmation": [
                    "catalog": [
                        "craft_id": proposal.catalog.craftID,
                        "harness": proposal.catalog.harness,
                        "native_metadata": proposal.catalog.nativeMetadata,
                    ],
                    "extension_id": proposal.extensionID,
                    "action": proposal.action.rawValue,
                    "scope": "user",
                    "trust": "same_user_executable",
                ],
            ],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "extension_change_queued"
        )
        guard let changeID = uuid(result["change_id"]) else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return changeID
    }

    func extensionChange(_ changeID: UUID) async throws -> JetExtensionChangeSummary {
        try await requireProtocolMinor(28, feature: "Harness extensions")
        let (data, requestID) = try await sendQuery([
            "type": "extension_change",
            "change_id": changeID.uuidString.lowercased(),
        ])
        return try decodeExtensionChange(
            data,
            requestID: requestID,
            expectedChangeID: changeID
        )
    }

    func scheduledTasks(conversationID: UUID) async throws -> JetScheduledTaskSnapshot {
        try await requireProtocolMinor(21, feature: "Schedules")
        let (data, requestID) = try await sendQuery([
            "type": "scheduled_tasks",
            "conversation_id": conversationID.uuidString.lowercased(),
        ])
        return try decodeScheduledTasks(
            data,
            requestID: requestID,
            expectedConversationID: conversationID
        )
    }

    func createSchedule(
        conversationID: UUID,
        timeZone: String,
        localTime: String,
        prompt: String,
        commandID: UUID = UUID()
    ) async throws -> JetScheduledTask {
        try validateSchedule(timeZone: timeZone, localTime: localTime, prompt: prompt)
        try await requireProtocolMinor(21, feature: "Schedules")
        let (data, requestID) = try await sendCommand(
            [
                "type": "create_schedule",
                "conversation_id": conversationID.uuidString.lowercased(),
                "time_zone": timeZone,
                "local_time": localTime,
                "prompt": prompt,
            ],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "schedule_created"
        )
        guard let task = result["task"] as? [String: Any] else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return try decodeScheduledTask(task, expectedConversationID: conversationID)
    }

    func cancelSchedule(
        _ scheduleID: UUID,
        commandID: UUID = UUID()
    ) async throws {
        try await requireProtocolMinor(21, feature: "Schedules")
        let (data, requestID) = try await sendCommand(
            [
                "type": "cancel_schedule",
                "schedule_id": scheduleID.uuidString.lowercased(),
            ],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "schedule_canceled"
        )
        guard uuid(result["schedule_id"]) == scheduleID else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    func pairing() async throws -> JetPairingSummary {
        let (data, requestID) = try await sendQuery(["type": "pairing"])
        return try decodePairing(data, requestID: requestID)
    }

    func setPairingGate(
        _ gate: String,
        commandID: UUID
    ) async throws -> String {
        guard ["open", "closed"].contains(gate) else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "pairing.gate_invalid",
                message: "Choose whether this Plane accepts new pairings."
            ))
        }
        try await requireProtocolMinor(6, feature: "remote pairing")
        let (data, requestID) = try await sendCommand(
            ["type": "set_pairing_gate", "gate": gate],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "pairing_gate_set"
        )
        guard let accepted = result["gate"] as? String,
              ["open", "closed"].contains(accepted)
        else { throw JetClientFailure.presentation(.invalidResponse) }
        return accepted
    }

    func openManualPairing(commandID: UUID) async throws -> JetOpenedPairing {
        try await requireProtocolMinor(6, feature: "remote pairing")
        let (data, requestID) = try await sendCommand(
            [
                "type": "open_pairing",
                "method": ["method": "manual_code"],
            ],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "pairing_opened"
        )
        guard let disclosure = result["disclosure"] as? [String: Any],
              let disclosureKind = disclosure["disclosure"] as? String,
              let pendingValue = result["pending"] as? [String: Any],
              let pending = JetPairingWire.pending(pendingValue)
        else { throw JetClientFailure.presentation(.invalidResponse) }
        let decoded: JetPairingDisclosure
        switch disclosureKind {
        case "manual_code":
            guard let code = disclosure["code"] as? String else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            decoded = .manualCode(code)
        case "qr_payload":
            guard let payload = disclosure["payload"] as? String else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            decoded = .qrPayload(payload)
        case "already_disclosed":
            decoded = .alreadyDisclosed
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetOpenedPairing(disclosure: decoded, pending: pending)
    }

    func confirmPairing(
        offerID: UUID,
        authenticationString: String,
        commandID: UUID
    ) async throws -> JetPendingPairing {
        try await requireProtocolMinor(6, feature: "remote pairing")
        let (data, requestID) = try await sendCommand(
            [
                "type": "confirm_pairing",
                "offer_id": offerID.uuidString.lowercased(),
                "authentication_string": authenticationString,
            ],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "pairing_confirmed"
        )
        guard let value = result["pending"] as? [String: Any],
              let pending = JetPairingWire.pending(value)
        else { throw JetClientFailure.presentation(.invalidResponse) }
        return pending
    }

    func setPairedClientAccess(
        clientID: UUID,
        access: JetPairedClientAccess,
        commandID: UUID
    ) async throws -> JetPairedClientSummary {
        try await requireProtocolMinor(6, feature: "paired-client access")
        let (data, requestID) = try await sendCommand(
            [
                "type": "set_paired_client_access",
                "client_id": clientID.uuidString.lowercased(),
                "access": access.rawValue,
            ],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "paired_client_access_set"
        )
        guard let value = result["client"] as? [String: Any],
              let client = JetPairingWire.client(value),
              client.id == clientID,
              client.access == access
        else { throw JetClientFailure.presentation(.invalidResponse) }
        return client
    }

    func revokePairedClient(
        clientID: UUID,
        commandID: UUID
    ) async throws -> UUID {
        try await requireProtocolMinor(6, feature: "paired-client revocation")
        let (data, requestID) = try await sendCommand(
            [
                "type": "revoke_paired_client",
                "client_id": clientID.uuidString.lowercased(),
            ],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "paired_client_revoked"
        )
        guard let revoked = uuid(result["client_id"]), revoked == clientID else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return revoked
    }

    func previewProjectRemoval(
        projectID: UUID
    ) async throws -> JetProjectRemovalPreview {
        let (data, requestID) = try await sendQuery([
            "type": "preview_project_removal",
            "project_id": projectID.uuidString.lowercased(),
        ])
        return try decodeProjectRemovalPreview(data, requestID: requestID)
    }

    func removeProject(
        preview: JetProjectRemovalPreview,
        typedName: String,
        disposal: JetProjectDisposal,
        commandID: UUID = UUID()
    ) async throws -> JetProjectRemoved {
        guard typedName == preview.name else {
            throw JetClientFailure.presentation(
                .invalidInput(
                    code: "project.name_mismatch",
                    message: "Type the Project folder name exactly as shown."
                )
            )
        }
        guard preview.obstacles.isEmpty else {
            throw JetClientFailure.presentation(
                .invalidInput(
                    code: "project.live_work",
                    message: "Resolve the listed blockers before removing this Project."
                )
            )
        }
        let binding = try JSONSerialization.jsonObject(
            with: Data(preview.binding.source.utf8)
        )
        let wireDisposal: [String: Any] = switch disposal {
        case .systemTrash:
            ["kind": "system_trash"]
        case let .permanent(acknowledgedWarning):
            [
                "kind": "permanent",
                "acknowledged_warning": acknowledgedWarning,
            ]
        }
        // ASVS 2.3.1 and 8.3.1: the command carries the exact validated
        // Plane binding shown by the preview and only the intended choices.
        let (data, requestID) = try await sendCommand(
            [
                "type": "remove_project",
                "binding": binding,
                "typed_name": typedName,
                "disposal": wireDisposal,
            ],
            commandID: commandID
        )
        return try decodeRemovedProject(data, requestID: requestID)
    }

    func changeDiff(runID: UUID, scope: JetChangeScope) async throws -> JetChangeDiff {
        let wireScope: [String: Any] = switch scope {
        case .current:
            ["kind": "current"]
        case .final:
            ["kind": "final"]
        case let .historical(fromTurn, toTurn):
            [
                "kind": "historical",
                "from_turn": NSNumber(value: fromTurn),
                "to_turn": NSNumber(value: toTurn),
            ]
        case let .turn(turn):
            ["kind": "turn", "turn": NSNumber(value: turn)]
        }
        let (data, requestID) = try await sendQuery([
            "type": "change_diff",
            "run_id": runID.uuidString.lowercased(),
            "scope": wireScope,
        ])
        return try decodeChangeDiff(data, requestID: requestID)
    }

    func nextChangeDiff(_ cursor: UUID) async throws -> JetChangeDiff {
        let (data, requestID) = try await sendQuery([
            "type": "next_change_diff",
            "cursor": cursor.uuidString.lowercased(),
        ])
        return try decodeChangeDiff(data, requestID: requestID)
    }

    func changeArtifact(sha256: String, offset: UInt64) async throws -> JetChangeArtifactChunk {
        guard sha256.count == 64,
              sha256.utf8.allSatisfy({ (48 ... 57).contains($0) || (97 ... 102).contains($0) })
        else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "artifact.identity_invalid",
                message: "The patch identity is not valid."
            ))
        }
        let (data, requestID) = try await sendQuery([
            "type": "change_artifact",
            "sha256": sha256,
            "offset": String(offset),
        ])
        return try decodeChangeArtifactChunk(data, requestID: requestID)
    }

    func gitDeliveries(conversationID: UUID) async throws -> [JetGitDelivery] {
        try await requireProtocolMinor(35, feature: "Git delivery")
        let (data, requestID) = try await sendQuery([
            "type": "git_deliveries",
            "conversation_id": conversationID.uuidString.lowercased(),
        ])
        let deliveries = try decodeGitDeliveries(data, requestID: requestID)
        guard deliveries.allSatisfy({ $0.conversationID == conversationID }) else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return deliveries
    }

    func deliverGit(
        _ request: JetGitDeliveryRequest,
        commandID: UUID
    ) async throws -> JetGitDeliveryQueued {
        try await requireProtocolMinor(35, feature: "Git delivery")
        try validateGitDelivery(request)
        let (data, requestID) = try await sendCommand(
            [
                "type": "deliver_git",
                "conversation_id": request.conversationID.uuidString.lowercased(),
                "checkpoint": request.checkpoint.map(wireGitCheckpoint) ?? NSNull(),
                "operation": wireGitOperation(request.operation),
            ],
            commandID: commandID
        )
        return try decodeGitDeliveryQueued(data, requestID: requestID)
    }

    func acknowledgeGitDelivery(
        deliveryID: UUID,
        commandID: UUID
    ) async throws -> UUID {
        try await requireProtocolMinor(35, feature: "Git delivery")
        let (data, requestID) = try await sendCommand(
            [
                "type": "acknowledge_git_delivery",
                "delivery_id": deliveryID.uuidString.lowercased(),
            ],
            commandID: commandID
        )
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "git_delivery_acknowledged"
        )
        guard let acknowledged = uuid(result["delivery_id"]), acknowledged == deliveryID else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return acknowledged
    }

    func editableFile(target: JetFileTarget, path: String) async throws -> JetEditableFile {
        try validateRelativePath(path)
        let (data, requestID) = try await sendQuery([
            "type": "editable_file",
            "target": wireFileTarget(target),
            "path": path,
        ])
        return try decodeEditableFile(data, requestID: requestID)
    }

    func applyUserEdit(
        target: JetFileTarget,
        path: String,
        expectedRevision: JetFileRevision,
        content: String,
        commandID: UUID
    ) async throws -> JetFileRevision {
        try validateRelativePath(path)
        guard content.utf8.count <= 128 * 1_024 else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "user_edit.too_large",
                message: "A file edit can contain at most 128 KiB of UTF-8 text."
            ))
        }
        // ASVS 2.2.1, 5.1.4, and 8.3.1: the client binds a validated relative
        // path to the exact registered target and revision it read.
        let (data, requestID) = try await sendCommand(
            [
                "type": "apply_user_edit",
                "target": wireFileTarget(target),
                "path": path,
                "expected_revision": wireFileRevision(expectedRevision),
                "content": content,
            ],
            commandID: commandID
        )
        return try decodeAppliedUserEdit(
            data,
            requestID: requestID,
            expectedTarget: target,
            expectedPath: path
        )
    }

    func submitReview(
        conversationID: UUID,
        path: String,
        line: UInt32,
        comment: String,
        commandID: UUID
    ) async throws -> JetTurnSummary {
        try validateRelativePath(path)
        guard line > 0,
              !comment.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              comment.utf8.count <= 8_192
        else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "review.comment_invalid",
                message: "Enter a line and a comment between 1 and 8,192 UTF-8 bytes."
            ))
        }
        let (data, requestID) = try await sendCommand(
            [
                "type": "submit_review",
                "conversation_id": conversationID.uuidString.lowercased(),
                "comments": [[
                    "path": path,
                    "line": NSNumber(value: line),
                    "comment": comment,
                ]],
            ],
            commandID: commandID
        )
        return try decodeAdmittedTurn(data, requestID: requestID)
    }

    func workspaceTerminals(workspaceID: UUID) async throws -> [JetWorkspaceTerminal] {
        let (data, requestID) = try await sendQuery([
            "type": "workspace_terminals",
            "workspace_id": workspaceID.uuidString.lowercased(),
        ])
        return try decodeWorkspaceTerminals(data, requestID: requestID)
    }

    func openTerminal(
        workspaceID: UUID,
        rows: UInt16 = 24,
        columns: UInt16 = 80,
        commandID: UUID
    ) async throws -> JetWorkspaceTerminal {
        try validateTerminalDimensions(rows: rows, columns: columns)
        let (data, requestID) = try await sendCommand(
            [
                "type": "open_terminal",
                "workspace_id": workspaceID.uuidString.lowercased(),
                "rows": NSNumber(value: rows),
                "columns": NSNumber(value: columns),
            ],
            commandID: commandID
        )
        return try decodeTerminalCommand(data, requestID: requestID)
    }

    func closeTerminal(
        terminalID: UUID,
        commandID: UUID
    ) async throws -> JetWorkspaceTerminal {
        let (data, requestID) = try await sendCommand(
            [
                "type": "close_terminal",
                "terminal_id": terminalID.uuidString.lowercased(),
            ],
            commandID: commandID
        )
        return try decodeTerminalCommand(data, requestID: requestID)
    }

    func attachTerminal(
        terminalID: UUID,
        after: UInt64,
        credit: UInt64 = 65_536
    ) async throws -> AsyncThrowingStream<JetTerminalEvent, Error> {
        try await ensureConnected()
        guard let negotiation, negotiation.minorVersion >= 18 else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "protocol.feature_unavailable",
                message: "This Plane does not support Workspace terminals."
            ))
        }
        guard credit > 0, credit <= Self.maximumTerminalCredit,
              terminalReplies.count < Self.maximumTerminalStreams
        else {
            throw JetClientFailure.presentation(.overloaded)
        }
        let streamID = takeStreamID()
        guard pending[streamID] == nil, terminalReplies[streamID] == nil else {
            throw JetClientFailure.presentation(.overloaded)
        }
        let requestID = takeRequestID()
        let (stream, continuation) = AsyncThrowingStream.makeStream(
            of: JetTerminalEvent.self,
            bufferingPolicy: .bufferingNewest(64)
        )
        terminalReplies[streamID] = TerminalReply(
            terminalID: terminalID,
            nextOffset: after,
            continuation: continuation,
            attachRequestID: requestID,
            resizeRequestIDs: [],
            attached: false,
            active: true
        )
        continuation.onTermination = { _ in
            Task { await self.detachTerminal(terminalID: terminalID) }
        }
        do {
            let payload = try encode([
                "kind": "attach_terminal",
                "id": NSNumber(value: requestID),
                "terminal_id": terminalID.uuidString.lowercased(),
                "after": String(after),
                "credit": String(credit),
            ], definition: "ClientMessage")
            try await writeFrame(.control, streamID: streamID, payload: payload)
            return stream
        } catch {
            terminalReplies.removeValue(forKey: streamID)?.continuation?.finish(
                throwing: normalized(error)
            )
            throw normalized(error)
        }
    }

    func sendTerminalInput(terminalID: UUID, bytes: Data) async throws {
        guard !bytes.isEmpty, bytes.count <= 65_536,
              let streamID = activeTerminalStream(for: terminalID)
        else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "terminal.input_invalid",
                message: "Attach the terminal before sending up to 65,536 bytes of input."
            ))
        }
        try await writeFrame(.data, streamID: streamID, payload: bytes)
    }

    func resizeTerminal(terminalID: UUID, rows: UInt16, columns: UInt16) async throws {
        try validateTerminalDimensions(rows: rows, columns: columns)
        guard let streamID = activeTerminalStream(for: terminalID),
              var reply = terminalReplies[streamID]
        else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "terminal.not_attached",
                message: "Attach this terminal before resizing it."
            ))
        }
        let requestID = takeRequestID()
        reply.resizeRequestIDs.insert(requestID)
        terminalReplies[streamID] = reply
        let payload = try encode([
            "kind": "resize_terminal",
            "id": NSNumber(value: requestID),
            "rows": NSNumber(value: rows),
            "columns": NSNumber(value: columns),
        ], definition: "ClientMessage")
        try await writeFrame(.control, streamID: streamID, payload: payload)
    }

    func detachTerminal(terminalID: UUID) {
        guard let streamID = terminalReplies.first(where: { _, reply in
            reply.terminalID == terminalID && reply.active
        })?.key,
              var reply = terminalReplies[streamID]
        else { return }
        reply.active = false
        reply.continuation?.finish()
        reply.continuation = nil
        terminalReplies[streamID] = reply
    }

    func events(after cursor: UInt64) async throws -> JetEventBatch {
        var attempt = 0
        while true {
            do {
                try await ensureConnected()
                let requestID = takeRequestID()
                let payload = try encodeClientMessage([
                    "kind": "query",
                    "id": NSNumber(value: requestID),
                    "query": [
                        "type": "events",
                        "after": String(cursor),
                    ],
                ])
                let response = try await exchange(
                    payload,
                    cancellationFailure: .presentation(.cancelled)
                )
                return try decodeEvents(
                    response,
                    requestID: requestID,
                    after: cursor
                )
            } catch {
                guard try await prepareReadRetry(error, attempt: attempt) else {
                    throw normalized(error)
                }
                attempt += 1
            }
        }
    }

    /// Returns Plane events in sequence order. A reconnect resumes strictly
    /// after the last event yielded; cancellation stops only this subscription.
    func eventStream(
        after initialCursor: UInt64
    ) -> AsyncThrowingStream<JetEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                var cursor = initialCursor
                do {
                    while !Task.isCancelled {
                        let batch = try await self.events(after: cursor)
                        for event in batch.events {
                            guard !Task.isCancelled else { throw CancellationError() }
                            continuation.yield(event)
                            cursor = event.sequence
                        }
                        if batch.events.isEmpty || cursor == batch.cursor {
                            try await ContinuousClock().sleep(
                                for: self.configuration.eventPollDelay
                            )
                        }
                    }
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: self.normalized(error))
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    func settings(scope: JetSettingScope) async throws -> JetSettingSnapshot {
        try await requireProtocolMinor(3, feature: "Settings")
        let (data, requestID) = try await sendQuery([
            "type": "settings",
            "scope": wireScope(scope),
            "selection": ["type": "all"],
        ])
        return try decodeSettings(
            data,
            requestID: requestID,
            expectedScope: scope
        )
    }

    func setSetting(
        _ key: SettingKey,
        value: JetSettingValue,
        scope: JetSettingScope,
        commandID: UUID = UUID()
    ) async throws -> JetSettingSet {
        try await requireProtocolMinor(3, feature: "Settings")
        try validateSetting(key: key, value: value, scope: scope)
        let (data, requestID) = try await sendCommand(
            [
                "type": "set_setting",
                "key": key.rawValue,
                "scope": wireScope(scope),
                "value": wireSettingValue(value),
            ],
            commandID: commandID
        )
        return try decodeSettingSet(
            data,
            requestID: requestID,
            expectedKey: key,
            expectedValue: value,
            expectedScope: scope
        )
    }

    func clearSetting(
        _ key: SettingKey,
        scope: JetSettingScope,
        commandID: UUID = UUID()
    ) async throws -> JetSettingCleared {
        try await requireProtocolMinor(3, feature: "Settings")
        let (data, requestID) = try await sendCommand(
            [
                "type": "clear_setting",
                "key": key.rawValue,
                "scope": wireScope(scope),
            ],
            commandID: commandID
        )
        return try decodeSettingCleared(
            data,
            requestID: requestID,
            expectedKey: key,
            expectedScope: scope
        )
    }

    private func ensureConnected() async throws {
        if transport != nil, negotiation != nil { return }
        if let connectionTask {
            try await connectionTask.value
            return
        }

        let task = Task { try await self.openConnection() }
        connectionTask = task
        do {
            try await task.value
            connectionTask = nil
        } catch {
            connectionTask = nil
            let failure = normalized(error)
            if case let .presentation(presentation) = failure {
                publishState(.failed(presentation))
            }
            throw failure
        }
    }

    private func sendQuery(
        _ query: [String: Any]
    ) async throws -> (Data, UInt64) {
        var attempt = 0
        while true {
            do {
                try await ensureConnected()
                let requestID = takeRequestID()
                let payload = try encodeClientMessage([
                    "kind": "query",
                    "id": NSNumber(value: requestID),
                    "query": query,
                ])
                let response = try await exchange(
                    payload,
                    cancellationFailure: .presentation(.cancelled)
                )
                return (response, requestID)
            } catch {
                guard try await prepareReadRetry(error, attempt: attempt) else {
                    throw normalized(error)
                }
                attempt += 1
            }
        }
    }

    private func sendCommand(
        _ command: [String: Any],
        commandID: UUID
    ) async throws -> (Data, UInt64) {
        var attempt = 0
        while true {
            do {
                try await ensureConnected()
                let requestID = takeRequestID()
                let payload = try encodeClientMessage([
                    "kind": "command",
                    "id": NSNumber(value: requestID),
                    "command_id": commandID.uuidString.lowercased(),
                    "command": command,
                ])
                let response = try await exchange(
                    payload,
                    cancellationFailure: .commandOutcomeUnknown(commandID: commandID)
                )
                return (response, requestID)
            } catch {
                if isOffline(error), attempt < configuration.reconnectDelays.count {
                    try await waitBeforeReconnect(attempt: attempt)
                    attempt += 1
                    continue
                }
                if isOffline(error) {
                    throw JetClientFailure.commandOutcomeUnknown(commandID: commandID)
                }
                throw normalized(error)
            }
        }
    }

    private func openConnection() async throws {
        publishState(.connecting)
        let candidate = makeTransport()
        do {
            try await withThrowingTaskGroup(of: Void.self) { group in
                group.addTask {
                    try await candidate.connect()
                }
                group.addTask { [connectionTimeout = configuration.connectionTimeout] in
                    try await Task.sleep(for: connectionTimeout)
                    throw JetClientFailure.presentation(.offline)
                }
                defer { group.cancelAll() }
                guard try await group.next() != nil else {
                    throw JetClientFailure.presentation(.offline)
                }
            }
            let hello = try encodeClientHello()
            try await candidate.write(JetHandshakeCodec.preface)
            try await candidate.write(
                JetFrameCodec.encode(
                    JetFrame(kind: .control, streamID: 0, payload: hello),
                    multiplexed: false,
                    limits: .protocolMaximum
                )
            )
            let frame = try await readHandshakeFrame(
                from: candidate,
                timeout: configuration.connectionTimeout
            )
            guard frame.kind == .control, frame.streamID == 0 else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            let accepted: JetNegotiation
            if try serverHelloIsChallenge(frame.payload) {
                guard let remoteSigner else {
                    throw JetClientFailure.presentation(
                        JetPresentationError(
                            category: .incompatible,
                            code: "protocol.unexpected_remote_challenge",
                            message: "The local Plane requested remote authentication.",
                            retryable: false
                        )
                    )
                }
                let nonce = try JetHandshakeCodec.challenge(frame.payload, schema: schema)
                var transcript = Data("jet.connection.v1\0ed25519\0".utf8)
                transcript.append(hello)
                transcript.append(nonce)
                let signature = try await remoteSigner.sign(transcript)
                guard signature.count == 64 else {
                    throw JetIdentityFailure.invalidSignature
                }
                let proof = try JetHandshakeCodec.encode(
                    ["signature": signature.hexadecimal],
                    definition: "ConnectionProof",
                    schema: schema
                )
                try await candidate.write(
                    JetFrameCodec.encode(
                        JetFrame(kind: .control, streamID: 0, payload: proof),
                        multiplexed: false,
                        limits: .protocolMaximum
                    )
                )
                let welcome = try await readHandshakeFrame(
                    from: candidate,
                    timeout: configuration.connectionTimeout
                )
                guard welcome.kind == .control, welcome.streamID == 0 else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                accepted = try decodeServerHello(welcome.payload)
                guard accepted.minorVersion >= 7 else {
                    throw JetClientFailure.presentation(
                        JetPresentationError(
                            category: .incompatible,
                            code: "protocol.remote_auth_downgrade",
                            message: "The remote Plane did not keep authenticated transport enabled.",
                            retryable: false
                        )
                    )
                }
            } else {
                accepted = try decodeServerHello(frame.payload)
            }

            generation &+= 1
            let acceptedGeneration = generation
            transport = candidate
            negotiation = accepted
            publishState(.connected(accepted))
            readerTask = Task {
                await self.readReplies(
                    from: candidate,
                    negotiation: accepted,
                    generation: acceptedGeneration
                )
            }
        } catch {
            await candidate.close()
            throw normalized(error)
        }
    }

    private func encodeClientHello() throws -> Data {
        try JetHandshakeCodec.clientHello(
            clientID: configuration.clientID,
            schema: schema
        )
    }

    private func readHandshakeFrame(
        from transport: any JetByteTransport,
        timeout: Duration
    ) async throws -> JetFrame {
        return try await withThrowingTaskGroup(of: JetFrame.self) { group in
            group.addTask {
                try await JetFrameCodec.read(from: transport, multiplexed: false)
            }
            group.addTask {
                try await Task.sleep(for: timeout)
                await transport.close()
                throw JetClientFailure.presentation(.offline)
            }
            defer { group.cancelAll() }
            guard let frame = try await group.next() else {
                throw JetClientFailure.presentation(.offline)
            }
            return frame
        }
    }

    private func serverHelloIsChallenge(_ data: Data) throws -> Bool {
        let document = try schema.validate(data, definition: "ServerHello")
        guard let object = document.value as? [String: Any],
              let kind = object["kind"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return kind == "challenge"
    }

    private func decodeServerHello(_ data: Data) throws -> JetNegotiation {
        let document = try schema.validate(data, definition: "ServerHello")
        guard let object = document.value as? [String: Any],
              let kind = object["kind"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }

        switch kind {
        case "welcome":
            guard let protocolVersion = unsigned32(object["protocol"]),
                  let minor = unsigned32(object["minor"]),
                  let codec = object["codec"] as? String,
                  let maximumControl = unsigned32(object["max_control_frame"]),
                  let maximumData = unsigned32(object["max_data_frame"]),
                  protocolVersion == Self.protocolVersion,
                  minor <= Self.protocolMinor,
                  codec == Self.codec
            else {
                throw JetClientFailure.presentation(
                    JetPresentationError(
                        category: .incompatible,
                        code: "protocol.incompatible",
                        message: "This Plane uses an incompatible Jet protocol.",
                        retryable: false,
                        recoveryActions: []
                    )
                )
            }
            let peer = JetFrameLimits(
                control: Int(maximumControl),
                data: Int(maximumData)
            )
            return JetNegotiation(
                protocolVersion: protocolVersion,
                minorVersion: minor,
                codec: codec,
                frameLimits: .protocolMaximum.negotiated(with: peer)
            )
        case "rejected":
            guard let errorNode = document.root.member("error") else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            throw JetClientFailure.presentation(
                try decodeRemoteError(document, node: errorNode)
            )
        case "challenge":
            throw JetClientFailure.presentation(
                JetPresentationError(
                    category: .incompatible,
                    code: "protocol.unexpected_remote_challenge",
                    message: "The local Plane requested remote authentication.",
                    retryable: false,
                    recoveryActions: []
                )
            )
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    private func readReplies(
        from source: any JetByteTransport,
        negotiation: JetNegotiation,
        generation expectedGeneration: UInt64
    ) async {
        do {
            while !Task.isCancelled {
                let frame = try await JetFrameCodec.read(
                    from: source,
                    multiplexed: negotiation.minorVersion >= 2,
                    limits: negotiation.frameLimits
                )
                if terminalReplies[frame.streamID] != nil {
                    try await routeTerminalFrame(frame)
                    continue
                }
                guard frame.kind == .control else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }

                // ASVS 1.5.2 and 2.2.2: validate the complete untrusted server
                // envelope before request code observes any field or RawJSON.
                let document = try schema.validate(
                    frame.payload,
                    definition: "ServerMessage"
                )
                guard let object = document.value as? [String: Any],
                      let kind = object["kind"] as? String
                else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }

                if frame.streamID == 0,
                   kind == "error",
                   object["id"] == nil || object["id"] is NSNull {
                    guard let errorNode = document.root.member("error") else {
                        throw JetClientFailure.presentation(.invalidResponse)
                    }
                    throw JetClientFailure.presentation(
                        try decodeRemoteError(document, node: errorNode)
                    )
                }

                guard var reply = pending.removeValue(forKey: frame.streamID) else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                reply.continuation?.resume(returning: frame.payload)
                reply.continuation = nil
            }
        } catch is CancellationError {
            return
        } catch {
            await connectionFailed(normalized(error), generation: expectedGeneration)
        }
    }

    private func routeTerminalFrame(_ frame: JetFrame) async throws {
        guard var reply = terminalReplies[frame.streamID] else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        switch frame.kind {
        case .data:
            let addition = reply.nextOffset.addingReportingOverflow(
                UInt64(frame.payload.count)
            )
            guard reply.attached, !frame.payload.isEmpty, !addition.overflow else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            let offset = reply.nextOffset
            reply.nextOffset = addition.partialValue
            terminalReplies[frame.streamID] = reply
            if try yieldTerminal(
                .output(offset: offset, bytes: frame.payload),
                to: reply
            ) {
                try await sendTerminalCredit(
                    streamID: frame.streamID,
                    bytes: UInt64(frame.payload.count)
                )
            }
        case .control:
            if let document = try? schema.validate(frame.payload, definition: "ServerMessage"),
               let object = document.value as? [String: Any],
               let kind = object["kind"] as? String
            {
                switch kind {
                case "terminal_attached":
                    guard !reply.attached,
                          unsigned64(object["id"]) == reply.attachRequestID
                    else {
                        throw JetClientFailure.presentation(.invalidResponse)
                    }
                    reply.attached = true
                    terminalReplies[frame.streamID] = reply
                    _ = try yieldTerminal(.attached, to: reply)
                case "terminal_resized":
                    guard let id = unsigned64(object["id"]),
                          reply.resizeRequestIDs.remove(id) != nil
                    else {
                        throw JetClientFailure.presentation(.invalidResponse)
                    }
                    terminalReplies[frame.streamID] = reply
                    _ = try yieldTerminal(.resized, to: reply)
                case "error":
                    guard let errorNode = document.root.member("error") else {
                        throw JetClientFailure.presentation(.invalidResponse)
                    }
                    let failure = try decodeRemoteError(document, node: errorNode)
                    terminalReplies.removeValue(forKey: frame.streamID)?
                        .continuation?.finish(throwing: JetClientFailure.presentation(failure))
                default:
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                return
            }

            let document = try schema.validate(frame.payload, definition: "StreamControl")
            guard let object = document.value as? [String: Any],
                  let type = object["type"] as? String
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            switch type {
            case "terminal_gap":
                guard let first = object["first_missing_offset"] as? String,
                      let first = UInt64(first),
                      let missing = object["missing_bytes"] as? String,
                      let missing = UInt64(missing),
                      first == reply.nextOffset,
                      missing > 0,
                      !first.addingReportingOverflow(missing).overflow
                else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                reply.nextOffset = first + missing
                terminalReplies[frame.streamID] = reply
                _ = try yieldTerminal(
                    .gap(firstMissingOffset: first, missingBytes: missing),
                    to: reply
                )
            case "terminal_finished":
                guard let total = object["total_bytes"] as? String,
                      let total = UInt64(total),
                      total == reply.nextOffset
                else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                _ = try yieldTerminal(.finished(totalBytes: total), to: reply)
                terminalReplies.removeValue(forKey: frame.streamID)?.continuation?.finish()
            default:
                throw JetClientFailure.presentation(.invalidResponse)
            }
        }
    }

    private func yieldTerminal(
        _ event: JetTerminalEvent,
        to reply: TerminalReply
    ) throws -> Bool {
        guard reply.active, let continuation = reply.continuation else { return false }
        switch continuation.yield(event) {
        case .enqueued:
            return true
        case .terminated:
            return false
        case .dropped:
            throw JetClientFailure.presentation(.overloaded)
        @unknown default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    private func sendTerminalCredit(streamID: UInt32, bytes: UInt64) async throws {
        guard bytes > 0, bytes <= Self.maximumTerminalCredit else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let payload = try encode([
            "type": "credit",
            "bytes": NSNumber(value: bytes),
        ], definition: "StreamControl")
        try await writeFrame(.control, streamID: streamID, payload: payload)
    }

    private func writeFrame(
        _ kind: JetFrameKind,
        streamID: UInt32,
        payload: Data
    ) async throws {
        guard let transport, let negotiation else {
            throw JetClientFailure.presentation(.offline)
        }
        let data = try JetFrameCodec.encode(
            JetFrame(kind: kind, streamID: streamID, payload: payload),
            multiplexed: negotiation.minorVersion >= 2,
            limits: negotiation.frameLimits
        )
        let expectedGeneration = generation
        do {
            try await transport.write(data)
        } catch {
            await connectionFailed(.presentation(.offline), generation: expectedGeneration)
            throw JetClientFailure.presentation(.offline)
        }
    }

    private func activeTerminalStream(for terminalID: UUID) -> UInt32? {
        terminalReplies.first { _, reply in
            reply.terminalID == terminalID && reply.active && reply.attached
        }?.key
    }

    private func exchange(
        _ payload: Data,
        cancellationFailure: JetClientFailure
    ) async throws -> Data {
        try Task.checkCancellation()
        guard let transport, let negotiation else {
            throw JetClientFailure.presentation(.offline)
        }
        guard pending.count < Self.maximumInFlightRequests else {
            throw JetClientFailure.presentation(.overloaded)
        }

        let streamID = negotiation.minorVersion >= 2 ? takeStreamID() : 0
        guard pending[streamID] == nil else {
            throw JetClientFailure.presentation(.overloaded)
        }
        let frame = try JetFrameCodec.encode(
            JetFrame(kind: .control, streamID: streamID, payload: payload),
            multiplexed: negotiation.minorVersion >= 2,
            limits: negotiation.frameLimits
        )
        let expectedGeneration = generation

        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                pending[streamID] = PendingReply(
                    continuation: continuation,
                    sent: false,
                    cancellationFailure: cancellationFailure
                )
                Task {
                    await self.sendReserved(
                        frame,
                        streamID: streamID,
                        transport: transport,
                        generation: expectedGeneration
                    )
                }
            }
        } onCancel: {
            Task { await self.cancelPending(streamID: streamID) }
        }
    }

    private func sendReserved(
        _ frame: Data,
        streamID: UInt32,
        transport: any JetByteTransport,
        generation expectedGeneration: UInt64
    ) async {
        guard var reply = pending[streamID] else { return }
        reply.sent = true
        pending[streamID] = reply
        do {
            try await transport.write(frame)
        } catch {
            await connectionFailed(
                .presentation(.offline),
                generation: expectedGeneration
            )
        }
    }

    private func cancelPending(streamID: UInt32) {
        guard var reply = pending[streamID],
              let continuation = reply.continuation
        else {
            return
        }
        continuation.resume(throwing: reply.cancellationFailure)
        reply.continuation = nil
        if reply.sent {
            pending[streamID] = reply
        } else {
            pending.removeValue(forKey: streamID)
        }
    }

    private func connectionFailed(
        _ failure: JetClientFailure,
        generation failedGeneration: UInt64
    ) async {
        guard failedGeneration == generation else { return }
        await closeConnection(
            failure: failure,
            publish: stateForFailure(failure)
        )
    }

    private func closeConnection(
        failure: JetClientFailure,
        publish newState: JetConnectionState
    ) async {
        generation &+= 1
        let oldTransport = transport
        transport = nil
        negotiation = nil
        readerTask?.cancel()
        readerTask = nil
        connectionTask?.cancel()
        connectionTask = nil
        publishState(newState)

        let continuations = pending.values.compactMap(\.continuation)
        pending.removeAll(keepingCapacity: true)
        continuations.forEach { $0.resume(throwing: failure) }
        let terminalContinuations = terminalReplies.values.compactMap(\.continuation)
        terminalReplies.removeAll(keepingCapacity: true)
        terminalContinuations.forEach { $0.finish(throwing: failure) }
        await oldTransport?.close()
    }

    private func decodeStatus(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetPlaneStatus {
        let document = try responseDocument(data, requestID: requestID)
        guard let root = document.value as? [String: Any],
              root["kind"] as? String == "query_result",
              let result = root["result"] as? [String: Any],
              result["type"] as? String == "status",
              let resultNode = document.root.member("result"),
              let planeID = uuid(result["plane_id"]),
              let daemonStarts = unsigned64(result["daemon_starts"]),
              let startedAt = signed64(result["started_at_unix_ms"]),
              let coreVersion = result["core_version"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }

        let cursor: UInt64?
        if let cursorText = result["cursor"] as? String {
            guard let parsed = UInt64(cursorText) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            cursor = parsed
        } else {
            cursor = nil
        }

        return JetPlaneStatus(
            cursor: cursor,
            planeID: planeID,
            daemonStarts: daemonStarts,
            startedAtUnixMilliseconds: startedAt,
            coreVersion: coreVersion,
            security: try optionalRawJSON(
                document,
                node: resultNode.member("security"),
                value: result["security"]
            ),
            recovery: try optionalRawJSON(
                document,
                node: resultNode.member("recovery"),
                value: result["recovery"]
            )
        )
    }

    private func decodeCapabilities(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetCapabilitySummary {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "capabilities"
        )
        guard let coreVersion = result["core_version"] as? String,
              let platform = result["platform"] as? [String: Any],
              let operatingSystem = platform["operating_system"] as? String,
              let architecture = platform["architecture"] as? String,
              let externalTools = result["external_tools"] as? [[String: Any]],
              let harnesses = result["harnesses"] as? [String],
              let crafts = result["crafts"] as? [[String: Any]],
              let credentialStore = result["credential_store"] as? [String: Any],
              let credentialStatus = credentialStore["status"] as? String,
              let storeState = JetCredentialStoreState(rawValue: credentialStatus),
              let degraded = result["degraded"] as? [[String: Any]]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetCapabilitySummary(
            coreVersion: coreVersion,
            platform: "\(operatingSystem) · \(architecture)",
            externalTools: try externalTools.map(decodeExternalTool),
            harnesses: harnesses,
            crafts: try crafts.map { craft in
                guard let id = craft["craft_id"] as? String,
                      let version = craft["version"] as? String,
                      let harnesses = craft["harnesses"] as? [String],
                      !id.isEmpty,
                      id.utf8.count <= 128
                else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                return JetInstalledCraft(id: id, version: version, harnesses: harnesses)
            },
            credentialStore: storeState,
            degraded: degraded.compactMap(degradedLabel)
        )
    }

    private func decodeProjects(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetProjectList {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "projects"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let values = result["projects"] as? [[String: Any]]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetProjectList(
            cursor: cursor,
            projects: try values.map(decodeProject)
        )
    }

    private func decodeConversations(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetConversationPage {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "conversations"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let values = result["conversations"] as? [[String: Any]]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let nextPage: UUID?
        if let value = result["next_page"], !(value is NSNull) {
            guard let parsed = uuid(value) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            nextPage = parsed
        } else {
            nextPage = nil
        }
        return JetConversationPage(
            cursor: cursor,
            conversations: try values.map(decodeConversationSummary),
            nextPage: nextPage
        )
    }

    private func decodeConversation(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetConversationSnapshot {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "conversation"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let conversation = result["conversation"] as? [String: Any],
              let runs = result["runs"] as? [[String: Any]]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let workspaceID: UUID?
        let workspaceRoot: String?
        if let workspace = result["workspace"] as? [String: Any] {
            guard let id = uuid(workspace["workspace_id"]),
                  let root = workspace["root"] as? String
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            workspaceID = id
            workspaceRoot = safeDisplayText(root, maximumBytes: 4_096, fallback: "Workspace")
        } else {
            workspaceID = nil
            workspaceRoot = nil
        }
        return JetConversationSnapshot(
            cursor: cursor,
            conversation: try decodeConversationSummary(conversation),
            workspaceID: workspaceID,
            workspaceRoot: workspaceRoot,
            runs: try runs.map(decodeRun)
        )
    }

    private func decodeChangeDiff(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetChangeDiff {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "change_diff"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let runID = uuid(result["run_id"]),
              let scopeValue = result["scope"] as? [String: Any],
              let scope = try? decodeChangeScope(scopeValue),
              let latestTurn = unsigned32(result["latest_turn"]),
              let totalFiles = unsigned32(result["total_files"]),
              let before = result["before"] as? [String: Any],
              let beforeComplete = before["content_complete"] as? Bool,
              let after = result["after"] as? [String: Any],
              let afterComplete = after["content_complete"] as? Bool,
              let values = result["files"] as? [[String: Any]],
              let patch = result["patch"] as? String,
              patch.utf8.count <= 1_048_576,
              let patchTruncated = result["patch_truncated"] as? Bool,
              let artifactValue = result["artifact"] as? [String: Any]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let workspaceID = try optionalUUID(result["workspace_id"])
        let nextPage = try optionalUUID(result["next_page"])
        return JetChangeDiff(
            cursor: cursor,
            runID: runID,
            workspaceID: workspaceID,
            scope: scope,
            latestTurn: latestTurn,
            totalFiles: totalFiles,
            files: try values.map(decodeChangedFile),
            nextPage: nextPage,
            patch: patch,
            patchTruncated: patchTruncated,
            contentComplete: beforeComplete && afterComplete,
            artifact: try decodeChangeArtifact(artifactValue)
        )
    }

    private func decodeChangeScope(_ value: [String: Any]) throws -> JetChangeScope {
        guard let kind = value["kind"] as? String else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        switch kind {
        case "current":
            return .current
        case "final":
            return .final
        case "historical":
            guard let fromTurn = unsigned32(value["from_turn"]),
                  let toTurn = unsigned32(value["to_turn"]),
                  fromTurn <= toTurn,
                  toTurn > 0
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .historical(fromTurn: fromTurn, toTurn: toTurn)
        case "turn":
            guard let turn = unsigned32(value["turn"]), turn > 0 else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .turn(turn)
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    private func decodeChangedFile(_ value: [String: Any]) throws -> JetChangedFile {
        guard let path = value["path"] as? String,
              let origin = value["origin"] as? [String: Any],
              let originKind = origin["kind"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        try validateRelativePath(path)
        let originLabel: String = switch originKind {
        case "user_edit": "user edit"
        case "workspace_terminal": "terminal"
        case "harness": "agent"
        case "mixed": "mixed"
        case "external_or_unknown": "external or unknown"
        default: throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetChangedFile(
            path: path,
            beforeObject: try optionalString(value["before_object"]),
            afterObject: try optionalString(value["after_object"]),
            beforeSize: try optionalUnsigned64(value["before_size"]),
            afterSize: try optionalUnsigned64(value["after_size"]),
            origin: originLabel
        )
    }

    private func decodeChangeArtifact(_ value: [String: Any]) throws -> JetChangeArtifact {
        guard let sha256 = value["sha256"] as? String,
              sha256.count == 64,
              sha256.utf8.allSatisfy({ (48 ... 57).contains($0) || (97 ... 102).contains($0) }),
              let size = unsigned64(value["size"]),
              let availability = JetArtifactAvailability(
                rawValue: value["availability"] as? String ?? "stored"
              )
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetChangeArtifact(sha256: sha256, size: size, availability: availability)
    }

    private func decodeChangeArtifactChunk(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetChangeArtifactChunk {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "change_artifact"
        )
        guard let artifact = result["artifact"] as? [String: Any],
              let offsetText = result["offset"] as? String,
              let offset = UInt64(offsetText),
              let bytes = result["bytes"] as? [NSNumber],
              bytes.count <= 65_536
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        var data = Data(capacity: bytes.count)
        for number in bytes {
            let value = number.intValue
            guard (0 ... 255).contains(value) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            data.append(UInt8(value))
        }
        return JetChangeArtifactChunk(
            artifact: try decodeChangeArtifact(artifact),
            offset: offset,
            bytes: data
        )
    }

    private func decodeEditableFile(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetEditableFile {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "editable_file"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let targetValue = result["target"] as? [String: Any],
              let path = result["path"] as? String,
              let revisionValue = result["revision"] as? [String: Any]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        try validateRelativePath(path)
        let content = try optionalString(result["content"])
        guard content?.utf8.count ?? 0 <= 128 * 1_024 else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetEditableFile(
            cursor: cursor,
            target: try decodeFileTarget(targetValue),
            path: path,
            content: content,
            revision: try decodeFileRevision(revisionValue)
        )
    }

    private func decodeAppliedUserEdit(
        _ data: Data,
        requestID: UInt64,
        expectedTarget: JetFileTarget,
        expectedPath: String
    ) throws -> JetFileRevision {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "user_edit_applied"
        )
        guard let target = result["target"] as? [String: Any],
              try decodeFileTarget(target) == expectedTarget,
              result["path"] as? String == expectedPath,
              let revision = result["revision"] as? [String: Any]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return try decodeFileRevision(revision)
    }

    private func decodeWorkspaceTerminals(
        _ data: Data,
        requestID: UInt64
    ) throws -> [JetWorkspaceTerminal] {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "workspace_terminals"
        )
        guard result["cursor"] is String,
              let terminals = result["terminals"] as? [[String: Any]],
              terminals.count <= 64
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return try terminals.map(decodeWorkspaceTerminal)
    }

    private func decodeTerminalCommand(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetWorkspaceTerminal {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "terminal"
        )
        guard let terminal = result["terminal"] as? [String: Any] else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return try decodeWorkspaceTerminal(terminal)
    }

    private func decodeWorkspaceTerminal(
        _ value: [String: Any]
    ) throws -> JetWorkspaceTerminal {
        guard let terminalID = uuid(value["terminal_id"]),
              let workspaceID = uuid(value["workspace_id"]),
              let stateText = value["state"] as? String,
              let state = JetTerminalState(rawValue: stateText)
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetWorkspaceTerminal(id: terminalID, workspaceID: workspaceID, state: state)
    }

    private func decodeFileRevision(_ value: [String: Any]) throws -> JetFileRevision {
        guard let object = value["object"] as? String,
              let mode = value["mode"] as? String,
              !object.isEmpty,
              !mode.isEmpty
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetFileRevision(object: object, mode: mode)
    }

    private func decodeFileTarget(_ value: [String: Any]) throws -> JetFileTarget {
        switch value["kind"] as? String {
        case "project":
            guard let id = uuid(value["project_id"]) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .project(id)
        case "workspace":
            guard let id = uuid(value["workspace_id"]) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .workspace(id)
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    private func decodeSearch(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetSearchResult {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "search"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let indexedText = result["indexed_through"] as? String,
              let indexedThrough = UInt64(indexedText),
              let values = result["hits"] as? [[String: Any]]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let hits = try values.map { value -> JetSearchHit in
            guard let conversationID = uuid(value["conversation_id"]),
                  let sequenceText = value["sequence"] as? String,
                  let sequence = UInt64(sequenceText),
                  let fieldText = value["field"] as? String,
                  let field = JetSearchField(rawValue: fieldText),
                  let excerpt = value["excerpt"] as? String
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return JetSearchHit(
                conversationID: conversationID,
                sequence: sequence,
                field: field,
                excerpt: safeDisplayText(excerpt, maximumBytes: 512, fallback: "Matching task")
            )
        }
        return JetSearchResult(cursor: cursor, indexedThrough: indexedThrough, hits: hits)
    }

    private func decodeCreatedConversation(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetConversationSummary {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "conversation_created"
        )
        return try decodeConversationSummary(result)
    }

    private func decodeCreatedRun(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetRunSummary {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "run_created"
        )
        return try decodeRun(result)
    }

    private func decodeAdmittedTurn(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetTurnSummary {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "turn_admitted"
        )
        guard let turn = result["turn"] as? [String: Any],
              let id = uuid(turn["turn_id"]),
              let sequenceText = turn["sequence"] as? String,
              let sequence = UInt64(sequenceText),
              let stateText = turn["state"] as? String,
              let state = JetTurnState(rawValue: stateText)
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetTurnSummary(id: id, sequence: sequence, state: state)
    }

    private func decodeWithdrawnTurn(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetTurnSummary {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "turn_withdrawn"
        )
        guard let turn = result["turn"] as? [String: Any] else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let decoded = try decodeTurn(turn, position: 0)
        return JetTurnSummary(id: decoded.id, sequence: decoded.sequence, state: decoded.state)
    }

    private func decodeTurnQueue(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetTurnQueue {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "turn_queue"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let turns = result["turns"] as? [[String: Any]],
              turns.count <= JetTurnQueue.maximumEntries
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetTurnQueue(
            cursor: cursor,
            turns: try turns.enumerated().map { index, turn in
                try decodeTurn(turn, position: index + 1)
            }
        )
    }

    private func decodeTurn(
        _ value: [String: Any],
        position: Int
    ) throws -> JetTurnQueueEntry {
        guard let id = uuid(value["turn_id"]),
              let sequenceText = value["sequence"] as? String,
              let sequence = UInt64(sequenceText),
              let clientID = uuid(value["client_id"]),
              let sourceText = value["source"] as? String,
              let source = JetTurnSource(rawValue: sourceText),
              let stateText = value["state"] as? String,
              let state = JetTurnState(rawValue: stateText)
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetTurnQueueEntry(
            id: id,
            sequence: sequence,
            position: position,
            source: source,
            state: state,
            runID: try optionalUUID(value["run_id"]),
            withdrawable: clientID == configuration.clientID
                && source == .user
                && state == .queued
        )
    }

    private func decodeRunExecution(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetRunExecution {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "run_execution"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let run = result["run"] as? [String: Any]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let activity: JetRunActivity?
        if let text = result["activity"] as? String {
            guard let parsed = JetRunActivity(rawValue: text) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            activity = parsed
        } else {
            activity = nil
        }
        let termination: JetRunTermination?
        if let value = result["termination"] as? [String: Any] {
            guard let controlText = value["control"] as? String,
                  let control = JetRunControl(rawValue: controlText),
                  let stageText = value["stage"] as? String,
                  let stage = JetTerminationStage(rawValue: stageText)
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            termination = JetRunTermination(control: control, stage: stage)
        } else {
            termination = nil
        }
        return JetRunExecution(
            cursor: cursor,
            run: try decodeRun(run),
            activity: activity,
            needsAttention: result["needs_attention"] as? Bool ?? false,
            termination: termination
        )
    }

    private func decodeRunControlAccepted(
        _ data: Data,
        requestID: UInt64,
        expectedControl: JetRunControl
    ) throws -> JetRunControlAccepted {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "run_control_accepted"
        )
        guard let run = result["run"] as? [String: Any],
              let controlText = result["control"] as? String,
              let control = JetRunControl(rawValue: controlText),
              control == expectedControl
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetRunControlAccepted(run: try decodeRun(run), control: control)
    }

    private func decodeConversationSummary(
        _ value: [String: Any]
    ) throws -> JetConversationSummary {
        guard let id = uuid(value["conversation_id"]),
              let createdAt = signed64(value["created_at_unix_ms"])
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let title: String
        if let name = value["name"] as? [String: Any], let raw = name["value"] as? String {
            title = safeDisplayText(raw, maximumBytes: 256, fallback: "Untitled task")
        } else {
            title = "Untitled task"
        }
        let projectID: UUID?
        if let workingTree = value["working_tree"] as? [String: Any],
           workingTree["kind"] as? String != "no_project"
        {
            guard let parsed = uuid(workingTree["project_id"]) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            projectID = parsed
        } else {
            projectID = nil
        }
        let revision: UInt64?
        if let revisionText = value["revision"] as? String {
            guard let parsed = UInt64(revisionText) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            revision = parsed
        } else if value["revision"] == nil || value["revision"] is NSNull {
            revision = nil
        } else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetConversationSummary(
            id: id,
            revision: revision,
            title: title,
            createdAtUnixMilliseconds: createdAt,
            projectID: projectID
        )
    }

    private func decodeRun(_ value: [String: Any]) throws -> JetRunSummary {
        guard let id = uuid(value["run_id"]),
              let conversationID = uuid(value["conversation_id"]),
              let revisionText = value["revision"] as? String,
              let revision = UInt64(revisionText),
              let lifecycleText = value["lifecycle"] as? String,
              let lifecycle = JetRunLifecycle(rawValue: lifecycleText),
              let createdAt = signed64(value["created_at_unix_ms"])
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let title: String
        if let name = value["name"] as? [String: Any], let raw = name["value"] as? String {
            title = safeDisplayText(raw, maximumBytes: 256, fallback: "Run")
        } else {
            title = "Run"
        }
        let endedAt: Int64?
        if let value = value["ended_at_unix_ms"], !(value is NSNull) {
            guard let parsed = signed64(value) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            endedAt = parsed
        } else {
            endedAt = nil
        }
        return JetRunSummary(
            id: id,
            conversationID: conversationID,
            revision: revision,
            lifecycle: lifecycle,
            title: title,
            createdAtUnixMilliseconds: createdAt,
            endedAtUnixMilliseconds: endedAt
        )
    }

    private func decodeProjectPreview(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetProjectPreview {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "project_preview"
        )
        guard let root = result["root"] as? String,
              let value = result["registrability"] as? [String: Any],
              let verdict = value["verdict"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let registrability: JetProjectRegistrability
        switch verdict {
        case "registrable":
            guard let repository = value["repository"] as? [String: Any],
                  let worktree = repository["worktree"] as? [String: Any],
                  let worktreeKind = worktree["kind"] as? String,
                  let lfs = repository["lfs"] as? [String: Any],
                  let lfsStatus = lfs["status"] as? String
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            let tree = worktreeKind == "linked" ? "linked working tree" : "main working tree"
            let lfsLabel = lfsStatus == "present" ? "Git LFS available" : "Git LFS not installed"
            registrability = .registrable(detail: "\(tree), \(lfsLabel)")
        case "not_a_repository":
            registrability = .unavailable(
                verdict: verdict,
                detail: "Choose the top folder of a Git working tree."
            )
        case "broken_repository":
            registrability = .unavailable(
                verdict: verdict,
                detail: "Git cannot open this repository."
            )
        case "bare_repository":
            registrability = .unavailable(
                verdict: verdict,
                detail: "Jet needs a working tree, not a bare repository."
            )
        case "inside_git_dir":
            registrability = .unavailable(
                verdict: verdict,
                detail: "Choose the working tree instead of its .git directory."
            )
        case "inside_working_tree":
            guard let topLevel = value["toplevel"] as? String else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            registrability = .unavailable(
                verdict: verdict,
                detail: "Choose the repository root at \(topLevel)."
            )
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetProjectPreview(root: root, registrability: registrability)
    }

    private func decodeRegisteredProject(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetProjectSummary {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "project_registered"
        )
        return try decodeProject(result)
    }

    private func decodeAccountBindings(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetAccountBindingList {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "account_bindings"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let values = result["bindings"] as? [[String: Any]]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let bindings = try values.map { value -> JetAccountBindingSummary in
            guard let binding = value["binding"] as? [String: Any],
                  let credential = value["credential_state"] as? [String: Any],
                  let state = credential["state"] as? String
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return try decodeAccount(binding, state: state)
        }
        return JetAccountBindingList(cursor: cursor, bindings: bindings)
    }

    private func decodeBoundAccount(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetAccountBindingSummary {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "account_bound"
        )
        return try decodeAccount(result, state: "resolved_at_use")
    }

    private func decodePairing(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetPairingSummary {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "pairing"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let gate = result["gate"] as? String,
              ["open", "closed"].contains(gate)
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let clientValues = result["clients"] as? [[String: Any]] ?? []
        let clients = clientValues.compactMap(JetPairingWire.client)
        guard clients.count == clientValues.count else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let pending: JetPendingPairing?
        if let pendingValue = result["pending"] as? [String: Any] {
            guard let decoded = JetPairingWire.pending(pendingValue) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            pending = decoded
        } else {
            pending = nil
        }
        return JetPairingSummary(
            cursor: cursor,
            gate: gate,
            clients: clients,
            pending: pending
        )
    }

    private func decodeProjectRemovalPreview(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetProjectRemovalPreview {
        let (document, result, resultNode) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "project_removal_preview"
        )
        guard let binding = result["binding"] as? [String: Any],
              let bindingNode = resultNode.member("binding"),
              let projectID = uuid(binding["project_id"]),
              let root = binding["root"] as? String,
              let liveRuns = unsigned64(binding["live_runs"]),
              let schedules = unsigned64(binding["schedules"]),
              let dirtyFiles = unsigned64(binding["dirty_files"]),
              let unpushedCommits = unsigned64(binding["unpushed_commits"]),
              let workspaces = binding["workspaces"] as? [String],
              let diskUseBytes = unsigned64(result["disk_use_bytes"]),
              let obstacles = result["obstacles"] as? [String],
              let warning = result["permanent_removal_warning"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetProjectRemovalPreview(
            projectID: projectID,
            root: root,
            diskUseBytes: diskUseBytes,
            liveRuns: liveRuns,
            schedules: schedules,
            dirtyFiles: dirtyFiles,
            unpushedCommits: unpushedCommits,
            workspaceCount: workspaces.count,
            obstacles: obstacles.map(removalObstacleLabel),
            permanentWarning: warning,
            binding: try document.rawJSON(for: bindingNode)
        )
    }

    private func decodeGitDeliveries(
        _ data: Data,
        requestID: UInt64
    ) throws -> [JetGitDelivery] {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "git_deliveries"
        )
        guard let values = result["deliveries"] as? [[String: Any]], values.count <= 100 else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return try values.map(decodeGitDelivery)
    }

    private func decodeGitDelivery(_ value: [String: Any]) throws -> JetGitDelivery {
        guard let deliveryID = uuid(value["delivery_id"]),
              let conversationID = uuid(value["conversation_id"]),
              let operationValue = value["operation"] as? [String: Any],
              let policyValue = value["policy"] as? [String: Any],
              let outcomeValue = value["outcome"] as? [String: Any]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetGitDelivery(
            id: deliveryID,
            conversationID: conversationID,
            checkpoint: try decodeGitCheckpoint(value["checkpoint"]),
            operation: try decodeGitOperation(operationValue),
            policy: try decodeGitDeliveryPolicy(policyValue),
            utilityJobID: try optionalUUID(value["utility_job"]),
            message: try decodeGitMessage(value["message"]),
            acknowledgedBy: try optionalUUID(value["acknowledged_by"]),
            outcome: try decodeGitDeliveryOutcome(outcomeValue)
        )
    }

    private func decodeGitCheckpoint(_ value: Any?) throws -> JetGitCheckpoint? {
        guard let value, !(value is NSNull) else { return nil }
        guard let checkpoint = value as? [String: Any],
              let runID = uuid(checkpoint["run_id"]),
              let turn = unsigned32(checkpoint["turn"]),
              turn > 0
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetGitCheckpoint(runID: runID, turn: turn)
    }

    private func decodeGitOperation(_ value: [String: Any]) throws -> JetGitOperation {
        guard let operation = value["operation"] as? String else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        switch operation {
        case "branch":
            guard let name = validGitInput(value["name"], maximumBytes: 255) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .branch(name: name)
        case "commit":
            return .commit
        case "push":
            guard let remote = validGitInput(value["remote"], maximumBytes: 255) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .push(remote: remote)
        case "draft_pull_request":
            guard let remote = validGitInput(value["remote"], maximumBytes: 255) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            let base: String?
            if value["base"] == nil || value["base"] is NSNull {
                base = nil
            } else {
                guard let decoded = validGitInput(value["base"], maximumBytes: 255) else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                base = decoded
            }
            return .draftPullRequest(remote: remote, base: base)
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    private func decodeGitDeliveryPolicy(
        _ value: [String: Any]
    ) throws -> JetGitDeliveryPolicy {
        guard let automatic = value["automatic"] as? Bool,
              let branch = value["branch"] as? Bool,
              let commit = value["commit"] as? Bool,
              let push = value["push"] as? Bool,
              let draftPullRequest = value["draft_pull_request"] as? Bool,
              let branchPrefix = value["branch_prefix"] as? String,
              branchPrefix.utf8.count <= 255,
              !branchPrefix.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetGitDeliveryPolicy(
            automatic: automatic,
            branch: branch,
            commit: commit,
            push: push,
            draftPullRequest: draftPullRequest,
            branchPrefix: branchPrefix
        )
    }

    private func decodeGitMessage(_ value: Any?) throws -> JetGitMessage? {
        guard let value, !(value is NSNull) else { return nil }
        guard let message = value as? [String: Any],
              let title = message["title"] as? String,
              let body = message["body"] as? String,
              title.utf8.count <= 1_024,
              body.utf8.count <= 32_768
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let fallbackReason: String?
        if message["fallback_reason"] == nil || message["fallback_reason"] is NSNull {
            fallbackReason = nil
        } else {
            guard let reason = message["fallback_reason"] as? String,
                  reason.utf8.count <= 256,
                  !reason.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            fallbackReason = reason
        }
        return JetGitMessage(title: title, body: body, fallbackReason: fallbackReason)
    }

    private func decodeGitDeliveryOutcome(
        _ value: [String: Any]
    ) throws -> JetGitDeliveryOutcome {
        guard let status = value["status"] as? String else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        switch status {
        case "pending":
            return .pending
        case "completed":
            guard let head = value["head"] as? String,
                  (40 ... 64).contains(head.count),
                  head.utf8.allSatisfy({ (48 ... 57).contains($0) || (97 ... 102).contains($0) })
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            let branch = try optionalBoundedText(value["branch"], maximumBytes: 255)
            let pullRequest = try optionalBoundedText(
                value["pull_request"],
                maximumBytes: 2_048
            )
            return .completed(head: head, branch: branch, pullRequest: pullRequest)
        case "failed":
            guard let code = validGitInput(value["code"], maximumBytes: 256) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .failed(code: code)
        case "outcome_unknown":
            return .outcomeUnknown
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    private func decodeGitDeliveryQueued(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetGitDeliveryQueued {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "git_delivery_queued"
        )
        guard let deliveryID = uuid(result["delivery_id"]) else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetGitDeliveryQueued(deliveryID: deliveryID)
    }

    private func decodeRemovedProject(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetProjectRemoved {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "project_removed"
        )
        guard let projectID = uuid(result["project_id"]),
              let root = result["root"] as? String,
              let disposition = result["disposition"] as? String,
              ["trashed", "deleted"].contains(disposition)
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetProjectRemoved(
            projectID: projectID,
            root: root,
            disposition: disposition
        )
    }

    private func responseResult(
        _ data: Data,
        requestID: UInt64,
        kind: String,
        type: String
    ) throws -> (JetJSONDocument, [String: Any], JetJSONNode) {
        let document = try responseDocument(data, requestID: requestID)
        guard let root = document.value as? [String: Any],
              root["kind"] as? String == kind,
              let result = root["result"] as? [String: Any],
              result["type"] as? String == type,
              let resultNode = document.root.member("result")
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return (document, result, resultNode)
    }

    private func decodeProject(_ value: [String: Any]) throws -> JetProjectSummary {
        guard let projectID = uuid(value["project_id"]),
              let root = value["root"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetProjectSummary(id: projectID, root: root)
    }

    private func decodeAccount(
        _ value: [String: Any],
        state: String
    ) throws -> JetAccountBindingSummary {
        guard let bindingID = uuid(value["binding_id"]),
              let provider = value["provider"] as? String,
              let label = value["label"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let stateLabel = switch state {
        case "resolvable": "Ready"
        case "resolved_at_use": "Checked when used"
        case "waiting_for_unlock": "Unlock required"
        case "unavailable": "Unavailable"
        case "invalidated_by_restart": "Reconnect required"
        default: "Unknown"
        }
        guard stateLabel != "Unknown" else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetAccountBindingSummary(
            id: bindingID,
            provider: provider,
            label: label,
            state: state,
            stateLabel: stateLabel
        )
    }

    private func degradedLabel(_ value: [String: Any]) -> String? {
        switch value["condition"] as? String {
        case "missing_external_tool":
            switch value["tool"] as? String {
            case "git": "Git is not available"
            case "git-lfs": "Git LFS is not available"
            case "ssh": "SSH is not available"
            case "tailscale": "Tailscale is not available"
            default: nil
            }
        case "no_harness_available": "No Harness is installed"
        case "credential_store_unavailable": "Secure storage is unavailable"
        case "credential_store_locked": "Secure storage is locked"
        default: nil
        }
    }

    private func decodeExternalTool(
        _ value: [String: Any]
    ) throws -> JetExternalToolSummary {
        guard let tool = value["tool"] as? String,
              ["git", "git-lfs", "ssh", "tailscale"].contains(tool),
              let availability = value["availability"] as? [String: Any],
              let status = availability["status"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let decoded: JetExternalToolAvailability
        switch status {
        case "present":
            guard let version = availability["version"] as? String,
                  !version.isEmpty,
                  version.utf8.count <= 1_024,
                  !version.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            decoded = .present(version: version)
        case "missing":
            decoded = .missing
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetExternalToolSummary(tool: tool, availability: decoded)
    }

    private func removalObstacleLabel(_ value: String) -> String {
        switch value {
        case "live_runs": "Stop active Runs first"
        case "schedules": "Disable scheduled tasks first"
        case "filesystem_root": "A filesystem root cannot be removed"
        case "user_home": "Your home directory cannot be removed"
        case "jet_home": "Jet's data directory cannot be removed"
        case "contains_project": "Remove nested Projects first"
        default: "Refresh the removal preview"
        }
    }

    private func decodeEvents(
        _ data: Data,
        requestID: UInt64,
        after: UInt64
    ) throws -> JetEventBatch {
        let document = try responseDocument(data, requestID: requestID)
        guard let root = document.value as? [String: Any],
              root["kind"] as? String == "query_result",
              let result = root["result"] as? [String: Any],
              result["type"] as? String == "events",
              let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              cursor >= after,
              let values = result["events"] as? [[String: Any]],
              let nodes = document.root
                .member("result")?
                .member("events")?
                .elements,
              values.count == nodes.count
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }

        var events: [JetEvent] = []
        events.reserveCapacity(values.count)
        var previous = after
        for (value, node) in zip(values, nodes) {
            let event = try decodeEvent(document, value: value, node: node)
            guard event.sequence > previous, event.sequence <= cursor else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            events.append(event)
            previous = event.sequence
        }
        guard cursor == after || !events.isEmpty else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetEventBatch(cursor: cursor, events: events)
    }

    private func decodeEvent(
        _ document: JetJSONDocument,
        value: [String: Any],
        node: JetJSONNode
    ) throws -> JetEvent {
        guard let sequenceText = value["sequence"] as? String,
              let sequence = UInt64(sequenceText),
              let eventID = uuid(value["event_id"]),
              let actorNode = node.member("actor"),
              let recordedAt = signed64(value["recorded_at_unix_ms"]),
              let kind = value["kind"] as? String,
              let payloadVersion = unsigned32(value["payload_version"]),
              let payloadNode = node.member("payload")
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetEvent(
            sequence: sequence,
            eventID: eventID,
            actor: try document.rawJSON(for: actorNode),
            origin: try optionalRawJSON(
                document,
                node: node.member("origin"),
                value: value["origin"]
            ),
            recordedAtUnixMilliseconds: recordedAt,
            conversationID: try optionalUUID(value["conversation_id"]),
            runID: try optionalUUID(value["run_id"]),
            kind: kind,
            payloadVersion: payloadVersion,
            payload: try document.rawJSON(for: payloadNode)
        )
    }

    private func decodeSettings(
        _ data: Data,
        requestID: UInt64,
        expectedScope: JetSettingScope
    ) throws -> JetSettingSnapshot {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "settings"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let scope = decodeScope(result["scope"]),
              scope == expectedScope,
              let values = result["settings"] as? [[String: Any]]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        var settings: [JetResolvedSetting] = []
        settings.reserveCapacity(values.count)
        var keys = Set<String>()
        for value in values {
            let setting = try decodeSetting(value)
            guard keys.insert(setting.key.rawValue).inserted else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            settings.append(setting)
        }
        return JetSettingSnapshot(cursor: cursor, scope: scope, settings: settings)
    }

    private func decodeSetting(_ value: [String: Any]) throws -> JetResolvedSetting {
        guard let rawKey = value["key"] as? String,
              let key = SettingKey(rawValue: rawKey),
              let settingValue = decodeSettingValue(value["value"]),
              let sourceValue = value["source"] as? [String: Any],
              let rawSource = sourceValue["source"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let source: JetSettingSource
        switch rawSource {
        case "built_in":
            source = .builtIn
        case "scope":
            guard let scope = decodeScope(sourceValue["scope"]) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            source = .scope(scope)
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetResolvedSetting(key: key, value: settingValue, source: source)
    }

    private func decodeSettingSet(
        _ data: Data,
        requestID: UInt64,
        expectedKey: SettingKey,
        expectedValue: JetSettingValue,
        expectedScope: JetSettingScope
    ) throws -> JetSettingSet {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "command_result",
            type: "setting_set"
        )
        guard result["key"] as? String == expectedKey.rawValue,
              decodeScope(result["scope"]) == expectedScope,
              decodeSettingValue(result["value"]) == expectedValue
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetSettingSet(
            key: expectedKey,
            scope: expectedScope,
            value: expectedValue
        )
    }

    private func decodeSettingValue(_ value: Any?) -> JetSettingValue? {
        guard let value = value as? [String: Any],
              let type = value["type"] as? String
        else { return nil }
        switch type {
        case "flag":
            return (value["value"] as? Bool).map(JetSettingValue.flag)
        case "text":
            return (value["value"] as? String).map(JetSettingValue.text)
        case "count":
            return unsigned32(value["value"]).map(JetSettingValue.count)
        default:
            return nil
        }
    }

    private func decodeUsage(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetUsageSnapshot {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "usage"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let planeID = uuid(result["plane_id"]),
              let consumption = result["consumption"] as? [String: Any],
              let tokens = decodeUsageTokens(consumption["tokens"]),
              let measurements = unsigned64(consumption["measurements"]),
              let estimated = unsigned64(consumption["estimated"]),
              let interim = unsigned64(consumption["interim"]),
              let windows = result["quota_windows"] as? [[String: Any]]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetUsageSnapshot(
            cursor: cursor,
            planeID: planeID,
            tokens: tokens,
            measurements: measurements,
            estimated: estimated,
            interim: interim,
            quotaWindows: try windows.map(decodeQuotaWindow)
        )
    }

    private func decodeQuotaWindow(
        _ value: [String: Any]
    ) throws -> JetQuotaWindowSummary {
        guard let bindingID = uuid(value["binding_id"]),
              let provider = value["provider"] as? String,
              let window = value["window"] as? String,
              let measure = value["measure"] as? [String: Any],
              let unit = measure["unit"] as? String,
              ["tokens", "requests", "credits", "share"].contains(unit),
              let used = unsigned64(measure["used"]),
              let freshnessValue = value["freshness"] as? [String: Any],
              let state = freshnessValue["state"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let freshness: JetUsageFreshness
        switch state {
        case "fresh": freshness = .fresh
        case "stale": freshness = .stale
        case "unreachable":
            guard let reason = freshnessValue["reason"] as? String else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            freshness = .unreachable(reason)
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetQuotaWindowSummary(
            bindingID: bindingID,
            provider: provider,
            window: window,
            unit: unit,
            used: used,
            limit: try optionalUnsigned64(measure["limit"]),
            resetsAtUnixMilliseconds: try optionalSigned64(value["resets_at_unix_ms"]),
            freshness: freshness
        )
    }

    private func decodeUsageHistory(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetUsageHistorySnapshot {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "usage_history"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let planeID = uuid(result["plane_id"]),
              let resolution = result["resolution"] as? String,
              ["hour", "day"].contains(resolution),
              let rawSeries = result["series"] as? [[String: Any]]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let series = try rawSeries.map { value in
            guard let points = value["points"] as? [[String: Any]] else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            var totals = JetUsageTokens(input: 0, cachedInput: 0, output: 0, reasoning: 0)
            var measurements: UInt64 = 0
            for point in points {
                guard let tokens = decodeUsageTokens(point["tokens"]),
                      let count = unsigned64(point["measurements"])
                else {
                    throw JetClientFailure.presentation(.invalidResponse)
                }
                totals = addUsageTokens(totals, tokens)
                measurements = clampedAdd(measurements, count)
            }
            return JetUsageHistorySeries(
                model: try optionalBoundedText(value["model"], maximumBytes: 1_024),
                tokens: totals,
                measurements: measurements
            )
        }
        return JetUsageHistorySnapshot(
            cursor: cursor,
            planeID: planeID,
            resolution: resolution,
            series: series
        )
    }

    private func decodeUsageTokens(_ value: Any?) -> JetUsageTokens? {
        guard let value = value as? [String: Any],
              let input = unsigned64(value["input"]),
              let cachedInput = unsigned64(value["cached_input"]),
              let output = unsigned64(value["output"]),
              let reasoning = unsigned64(value["reasoning"])
        else { return nil }
        return JetUsageTokens(
            input: input,
            cachedInput: cachedInput,
            output: output,
            reasoning: reasoning
        )
    }

    private func decodeExtensionCatalog(
        _ data: Data,
        requestID: UInt64,
        expectedCraftID: String
    ) throws -> JetExtensionCatalogSummary {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "extension_catalog"
        )
        guard let craftID = result["craft_id"] as? String,
              craftID == expectedCraftID,
              let harness = result["harness"] as? String,
              let nativeMetadata = result["native_metadata"] as? String,
              !craftID.isEmpty,
              !harness.isEmpty,
              nativeMetadata.utf8.count <= 65_536
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetExtensionCatalogSummary(
            craftID: craftID,
            harness: harness,
            nativeMetadata: nativeMetadata
        )
    }

    private func decodeExtensionChange(
        _ data: Data,
        requestID: UInt64,
        expectedChangeID: UUID
    ) throws -> JetExtensionChangeSummary {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "extension_change"
        )
        guard let changeID = uuid(result["change_id"]),
              changeID == expectedChangeID,
              let craftID = result["craft_id"] as? String,
              let extensionID = result["extension_id"] as? String,
              let rawAction = result["action"] as? String,
              let action = JetExtensionAction(rawValue: rawAction),
              let state = result["state"] as? String,
              ["staged", "applied", "refused", "outcome_unknown"].contains(state)
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetExtensionChangeSummary(
            id: changeID,
            craftID: craftID,
            extensionID: extensionID,
            action: action,
            state: state
        )
    }

    private func decodeScheduledTasks(
        _ data: Data,
        requestID: UInt64,
        expectedConversationID: UUID
    ) throws -> JetScheduledTaskSnapshot {
        let (_, result, _) = try responseResult(
            data,
            requestID: requestID,
            kind: "query_result",
            type: "scheduled_tasks"
        )
        guard let cursorText = result["cursor"] as? String,
              let cursor = UInt64(cursorText),
              let values = result["tasks"] as? [[String: Any]],
              values.count <= 32
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetScheduledTaskSnapshot(
            cursor: cursor,
            tasks: try values.map {
                try decodeScheduledTask($0, expectedConversationID: expectedConversationID)
            }
        )
    }

    private func decodeScheduledTask(
        _ value: [String: Any],
        expectedConversationID: UUID
    ) throws -> JetScheduledTask {
        guard let scheduleID = uuid(value["schedule_id"]),
              let conversationID = uuid(value["conversation_id"]),
              conversationID == expectedConversationID,
              let timeZone = value["time_zone"] as? String,
              let localTime = value["local_time"] as? String,
              let prompt = value["prompt"] as? String,
              prompt.utf8.count <= 8_192,
              let next = value["next"] as? [String: Any],
              let dueAt = signed64(next["due_at_unix_ms"]),
              let intendedLocal = next["intended_local"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetScheduledTask(
            id: scheduleID,
            conversationID: conversationID,
            timeZone: timeZone,
            localTime: localTime,
            prompt: prompt,
            nextDueAtUnixMilliseconds: dueAt,
            nextIntendedLocal: intendedLocal
        )
    }

    private func decodeSettingCleared(
        _ data: Data,
        requestID: UInt64,
        expectedKey: SettingKey,
        expectedScope: JetSettingScope
    ) throws -> JetSettingCleared {
        let document = try responseDocument(data, requestID: requestID)
        guard let root = document.value as? [String: Any],
              root["kind"] as? String == "command_result",
              let result = root["result"] as? [String: Any],
              result["type"] as? String == "setting_cleared",
              result["key"] as? String == expectedKey.rawValue,
              let scope = decodeScope(result["scope"]),
              scope == expectedScope
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetSettingCleared(key: expectedKey, scope: scope)
    }

    private func responseDocument(
        _ data: Data,
        requestID: UInt64
    ) throws -> JetJSONDocument {
        let document = try schema.validate(data, definition: "ServerMessage")
        guard let root = document.value as? [String: Any],
              unsigned64(root["id"]) == requestID,
              let kind = root["kind"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        if kind == "error" {
            guard let errorNode = document.root.member("error") else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            throw JetClientFailure.presentation(
                try decodeRemoteError(document, node: errorNode)
            )
        }
        return document
    }

    private func decodeRemoteError(
        _ document: JetJSONDocument,
        node: JetJSONNode
    ) throws -> JetPresentationError {
        guard let root = document.value as? [String: Any],
              let error = root["error"] as? [String: Any],
              let categoryText = error["category"] as? String,
              let category = presentationCategory(categoryText),
              let code = error["code"] as? String,
              let message = error["message"] as? String,
              let retryable = error["retryable"] as? Bool
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let actions = try (error["recovery_actions"] as? [[String: Any]] ?? []).map(
            decodeRecoveryAction
        )
        return JetPresentationError(
            category: category,
            code: code,
            message: message,
            retryable: retryable,
            recoveryActions: actions,
            restart: try decodeRestart(error["restart"]),
            revisionConflict: try decodeRevisionConflict(error["revision_conflict"])
        )
    }

    private func decodeRecoveryAction(_ value: [String: Any]) throws -> JetRecoveryAction {
        guard let type = value["type"] as? String else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        switch type {
        case "refresh_file":
            return .refreshFile
        case "refresh_conversation":
            guard let id = uuid(value["conversation_id"]) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .refreshConversation(id)
        case "refresh_run":
            guard let id = uuid(value["run_id"]) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .refreshRun(id)
        case "resume_events":
            guard let after = unsigned64(value["after"]) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .resumeEvents(after: after)
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    private func decodeRestart(_ value: Any?) throws -> JetRestartMetadata? {
        guard let value else { return nil }
        guard let restart = value as? [String: Any],
              let reason = restart["reason"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        switch reason {
        case "cursor_expired":
            guard let minimum = unsigned64(restart["minimum_available_cursor"]),
                  let revision = unsigned64(restart["current_snapshot_revision"])
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .cursorExpired(minimumAvailable: minimum, snapshotRevision: revision)
        case "cursor_ahead":
            guard let revision = unsigned64(restart["current_snapshot_revision"]) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .cursorAhead(snapshotRevision: revision)
        case "pagination_stale":
            guard let revision = unsigned64(restart["current_snapshot_revision"]) else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            return .paginationStale(snapshotRevision: revision)
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    private func decodeRevisionConflict(_ value: Any?) throws -> JetRevisionConflict? {
        guard let value else { return nil }
        guard let conflict = value as? [String: Any],
              let currentRevision = unsigned64(conflict["current_revision"]),
              let safeState = conflict["safe_state"] as? [String: Any],
              let type = safeState["type"] as? String
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let decoded: JetConflictSafeState
        switch type {
        case "conversation":
            guard let conversation = safeState["conversation"] as? [String: Any],
                  let id = uuid(conversation["conversation_id"])
            else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            decoded = .conversation(
                id: id,
                revision: try optionalUnsigned64(conversation["revision"])
            )
        case "run":
            guard let run = safeState["run"] as? [String: Any] else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            decoded = .run(try decodeRun(run))
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetRevisionConflict(currentRevision: currentRevision, safeState: decoded)
    }

    private func encodeClientMessage(_ object: [String: Any]) throws -> Data {
        try encode(object, definition: "ClientMessage")
    }

    private func encode(
        _ object: [String: Any],
        definition: String
    ) throws -> Data {
        guard JSONSerialization.isValidJSONObject(object) else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let data = try JSONSerialization.data(
            withJSONObject: object,
            options: [.sortedKeys, .withoutEscapingSlashes]
        )
        _ = try schema.validate(data, definition: definition)
        return data
    }

    private func prepareReadRetry(_ error: Error, attempt: Int) async throws -> Bool {
        guard isOffline(error), attempt < configuration.reconnectDelays.count else {
            return false
        }
        try await waitBeforeReconnect(attempt: attempt)
        return true
    }

    private func waitBeforeReconnect(attempt: Int) async throws {
        publishState(.reconnecting(attempt: attempt + 1))
        try await ContinuousClock().sleep(for: configuration.reconnectDelays[attempt])
    }

    private func publishState(_ newState: JetConnectionState) {
        state = newState
        for continuation in connectionObservers.values {
            continuation.yield(newState)
        }
    }

    private func removeConnectionObserver(_ observerID: UUID) {
        connectionObservers.removeValue(forKey: observerID)
    }

    private func setupIssue(
        _ section: JetSetupSection,
        error: Error
    ) -> JetSetupIssue {
        let presentation: JetPresentationError = switch normalized(error) {
        case let .presentation(error): error
        case .commandOutcomeUnknown: .invalidResponse
        }
        return JetSetupIssue(section: section, error: presentation)
    }

    private func takeRequestID() -> UInt64 {
        defer {
            nextRequestID &+= 1
            if nextRequestID == 0 { nextRequestID = 1 }
        }
        return nextRequestID
    }

    private func takeStreamID() -> UInt32 {
        defer {
            nextStreamID &+= 1
            if nextStreamID == 0 { nextStreamID = 1 }
        }
        return nextStreamID
    }

    private func wireScope(_ scope: JetSettingScope) -> [String: Any] {
        switch scope {
        case .plane:
            ["type": "plane"]
        case let .project(projectID):
            ["type": "project", "project_id": projectID.uuidString.lowercased()]
        case let .conversation(conversationID):
            [
                "type": "conversation",
                "conversation_id": conversationID.uuidString.lowercased(),
            ]
        }
    }

    private func wireSettingValue(_ value: JetSettingValue) -> [String: Any] {
        switch value {
        case let .flag(flag):
            ["type": "flag", "value": flag]
        case let .text(text):
            ["type": "text", "value": text]
        case let .count(count):
            ["type": "count", "value": NSNumber(value: count)]
        }
    }

    private func wireFileTarget(_ target: JetFileTarget) -> [String: Any] {
        switch target {
        case let .project(projectID):
            [
                "kind": "project",
                "project_id": projectID.uuidString.lowercased(),
            ]
        case let .workspace(workspaceID):
            [
                "kind": "workspace",
                "workspace_id": workspaceID.uuidString.lowercased(),
            ]
        }
    }

    private func wireFileRevision(_ revision: JetFileRevision) -> [String: Any] {
        ["object": revision.object, "mode": revision.mode]
    }

    private func wireGitCheckpoint(_ checkpoint: JetGitCheckpoint) -> [String: Any] {
        [
            "run_id": checkpoint.runID.uuidString.lowercased(),
            "turn": NSNumber(value: checkpoint.turn),
        ]
    }

    private func wireGitOperation(_ operation: JetGitOperation) -> [String: Any] {
        switch operation {
        case let .branch(name):
            ["operation": "branch", "name": name]
        case .commit:
            ["operation": "commit"]
        case let .push(remote):
            ["operation": "push", "remote": remote]
        case let .draftPullRequest(remote, base):
            [
                "operation": "draft_pull_request",
                "remote": remote,
                "base": base ?? NSNull(),
            ]
        }
    }

    private func wireObservation(
        _ observation: JetCapabilityObservation
    ) -> [String: Any] {
        switch observation {
        case .lastObserved: ["type": "last_observed"]
        case .fresh: ["type": "fresh"]
        }
    }

    private func decodeScope(_ value: Any?) -> JetSettingScope? {
        guard let scope = value as? [String: Any],
              let type = scope["type"] as? String
        else {
            return nil
        }
        switch type {
        case "plane": return .plane
        case "project": return uuid(scope["project_id"]).map(JetSettingScope.project)
        case "conversation":
            return uuid(scope["conversation_id"]).map(JetSettingScope.conversation)
        default: return nil
        }
    }

    private func validatePrompt(_ prompt: String) throws {
        guard !prompt.isEmpty, prompt.utf8.count <= 65_536 else {
            throw JetClientFailure.presentation(
                .invalidInput(
                    code: "turn.invalid_prompt",
                    message: "Enter a task between 1 and 65,536 UTF-8 bytes."
                )
            )
        }
    }

    private func validateSetting(
        key: SettingKey,
        value: JetSettingValue,
        scope: JetSettingScope
    ) throws {
        let rawKey = key.rawValue
        let planeOnly: Set<String> = [
            "storage.disposable_mib", "energy.concurrency",
            "energy.low_power_concurrency", "energy.constrained",
            "energy.foreground_override", "artifact.max_mib", "artifact.run_mib",
            "utility.account_binding", "utility.content_consent",
            "utility.autodelete_compilation", "utility.git_text",
            "git.message_instructions", "security.audit_retention_days",
            "retention.trash_grace_days", "craft.developer_mode",
            "review.automatic", "review.account_binding",
            "review.cross_provider_consent",
        ]
        let projectOrConversation: Set<String> = [
            "git.auto_commit", "git.auto_branch", "git.auto_push",
            "git.auto_draft_pull_request", "git.branch_prefix",
        ]
        let scopeAccepted = if planeOnly.contains(rawKey) {
            scope == .plane
        } else if projectOrConversation.contains(rawKey) {
            switch scope {
            case .project, .conversation: true
            case .plane: false
            }
        } else if rawKey == "utility.automatic_naming" {
            true
        } else {
            false
        }
        guard scopeAccepted else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "setting.scope_unsupported",
                message: "This setting is not available at the selected scope."
            ))
        }

        let countKeys: Set<String> = [
            "storage.disposable_mib", "energy.concurrency",
            "energy.low_power_concurrency", "artifact.max_mib", "artifact.run_mib",
            "security.audit_retention_days", "retention.trash_grace_days",
        ]
        let textKeys: Set<String> = [
            "utility.account_binding", "utility.content_consent", "git.branch_prefix",
            "git.message_instructions", "review.account_binding",
            "review.cross_provider_consent",
        ]
        let shapeAccepted = switch value {
        case .count: countKeys.contains(rawKey)
        case .text: textKeys.contains(rawKey)
        case .flag: !countKeys.contains(rawKey) && !textKeys.contains(rawKey)
        }
        guard shapeAccepted else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "setting.value_unsupported",
                message: "This setting does not accept that value."
            ))
        }
        if case let .text(text) = value, text.utf8.count > 2_048 {
            throw JetClientFailure.presentation(.invalidInput(
                code: "setting.value_too_long",
                message: "Setting text is limited to 2,048 UTF-8 bytes."
            ))
        }
        if case let .count(count) = value,
           rawKey == "security.audit_retention_days",
           count < 90
        {
            throw JetClientFailure.presentation(.invalidInput(
                code: "setting.value_below_minimum",
                message: "Keep the Security audit for at least 90 days."
            ))
        }
        if case let .count(count) = value,
           rawKey == "retention.trash_grace_days",
           count < 1
        {
            throw JetClientFailure.presentation(.invalidInput(
                code: "setting.value_below_minimum",
                message: "Keep tasks in Jet Trash for at least one day."
            ))
        }
        let bindingKeys: Set<String> = [
            "utility.account_binding", "utility.content_consent",
            "review.account_binding", "review.cross_provider_consent",
        ]
        if case let .text(text) = value,
           bindingKeys.contains(rawKey),
           !text.isEmpty,
           UUID(uuidString: text) == nil
        {
            throw JetClientFailure.presentation(.invalidInput(
                code: "setting.binding_invalid",
                message: "Choose an Account binding or use the default."
            ))
        }
    }

    private func validateExtensionToken(_ value: String, field: String) throws {
        guard !value.isEmpty,
              value.utf8.count <= 1_024,
              !value.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "extension.identity_invalid",
                message: "\(field) identity must use 1 to 1,024 UTF-8 bytes without control characters."
            ))
        }
    }

    private func validateSchedule(
        timeZone: String,
        localTime: String,
        prompt: String
    ) throws {
        let components = localTime.split(separator: ":", omittingEmptySubsequences: false)
        guard TimeZone(identifier: timeZone) != nil,
              components.count == 3,
              components.allSatisfy({ $0.count == 2 && $0.allSatisfy(\.isNumber) }),
              let hour = Int(components[0]), (0 ... 23).contains(hour),
              let minute = Int(components[1]), (0 ... 59).contains(minute),
              let second = Int(components[2]), (0 ... 59).contains(second)
        else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "schedule.time_invalid",
                message: "Choose a valid local time and IANA time zone."
            ))
        }
        guard !prompt.isEmpty, prompt.utf8.count <= 8_192 else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "schedule.prompt_invalid",
                message: "Enter schedule instructions between 1 and 8,192 UTF-8 bytes."
            ))
        }
    }

    private func addUsageTokens(
        _ left: JetUsageTokens,
        _ right: JetUsageTokens
    ) -> JetUsageTokens {
        JetUsageTokens(
            input: clampedAdd(left.input, right.input),
            cachedInput: clampedAdd(left.cachedInput, right.cachedInput),
            output: clampedAdd(left.output, right.output),
            reasoning: clampedAdd(left.reasoning, right.reasoning)
        )
    }

    private func clampedAdd(_ left: UInt64, _ right: UInt64) -> UInt64 {
        let result = left.addingReportingOverflow(right)
        return result.overflow ? .max : result.partialValue
    }

    private func validateGitDelivery(_ request: JetGitDeliveryRequest) throws {
        if let checkpoint = request.checkpoint, checkpoint.turn == 0 {
            throw JetClientFailure.presentation(.invalidInput(
                code: "git.checkpoint_invalid",
                message: "Choose a retained Turn checkpoint before delivery."
            ))
        }
        // ASVS 2.2.1, 2.2.2, and 8.3.1: the client accepts only the four
        // typed protocol operations and bounds their display inputs. jetd
        // revalidates Git names, capability, policy, and Actor authority.
        let values: [String] = switch request.operation {
        case let .branch(name): [name]
        case .commit: []
        case let .push(remote): [remote]
        case let .draftPullRequest(remote, base): [remote] + (base.map { [$0] } ?? [])
        }
        guard values.allSatisfy({ validGitInput($0, maximumBytes: 255) != nil }) else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "git.destination_invalid",
                message: "Branch, remote, and base names must use 1 to 255 UTF-8 bytes without whitespace or control characters."
            ))
        }
    }

    private func requireProtocolMinor(_ minimum: UInt32, feature: String) async throws {
        try await ensureConnected()
        guard let negotiation, negotiation.minorVersion >= minimum else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "protocol.feature_unavailable",
                message: "This Plane does not support \(feature)."
            ))
        }
    }

    private func validGitInput(_ value: Any?, maximumBytes: Int) -> String? {
        guard let value = value as? String,
              !value.isEmpty,
              value.utf8.count <= maximumBytes,
              !value.unicodeScalars.contains(where: CharacterSet.whitespacesAndNewlines.contains),
              !value.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        else {
            return nil
        }
        return value
    }

    private func optionalBoundedText(
        _ value: Any?,
        maximumBytes: Int
    ) throws -> String? {
        guard let value, !(value is NSNull) else { return nil }
        guard let text = value as? String,
              text.utf8.count <= maximumBytes,
              !text.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return text
    }

    private func validateRelativePath(_ path: String) throws {
        let components = path.split(separator: "/", omittingEmptySubsequences: false)
        guard !path.isEmpty,
              path.utf8.count <= 4_096,
              !path.hasPrefix("/"),
              !components.contains(where: { $0.isEmpty || $0 == "." || $0 == ".." }),
              !path.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "file.path_invalid",
                message: "Choose a file from the selected registered root."
            ))
        }
    }

    private func validateTerminalDimensions(rows: UInt16, columns: UInt16) throws {
        guard (1 ... 1_000).contains(rows), (1 ... 1_000).contains(columns) else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "terminal.dimensions_invalid",
                message: "Terminal rows and columns must be between 1 and 1,000."
            ))
        }
    }

    private func isSafeToken(_ value: String, maximumBytes: Int) -> Bool {
        !value.isEmpty
            && value.utf8.count <= maximumBytes
            && value.utf8.allSatisfy { byte in
                (48 ... 57).contains(byte)
                    || (65 ... 90).contains(byte)
                    || (97 ... 122).contains(byte)
                    || byte == 45
                    || byte == 95
            }
    }

    private func safeDisplayText(
        _ value: String,
        maximumBytes: Int,
        fallback: String
    ) -> String {
        guard !value.isEmpty,
              value.utf8.count <= maximumBytes,
              !value.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
        else {
            return fallback
        }
        return value
    }

    private func optionalRawJSON(
        _ document: JetJSONDocument,
        node: JetJSONNode?,
        value: Any?
    ) throws -> JetRawJSON? {
        guard let node, !(value is NSNull) else { return nil }
        return try document.rawJSON(for: node)
    }

    private func optionalUUID(_ value: Any?) throws -> UUID? {
        guard let value, !(value is NSNull) else { return nil }
        guard let parsed = uuid(value) else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return parsed
    }

    private func optionalString(_ value: Any?) throws -> String? {
        guard let value, !(value is NSNull) else { return nil }
        guard let value = value as? String else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return value
    }

    private func optionalUnsigned64(_ value: Any?) throws -> UInt64? {
        guard let value, !(value is NSNull) else { return nil }
        guard let value = unsigned64(value) else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return value
    }

    private func optionalSigned64(_ value: Any?) throws -> Int64? {
        guard let value, !(value is NSNull) else { return nil }
        guard let value = signed64(value) else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return value
    }

    private func uuid(_ value: Any?) -> UUID? {
        guard let text = value as? String else { return nil }
        return UUID(uuidString: text)
    }

    private func unsigned32(_ value: Any?) -> UInt32? {
        guard let parsed = unsigned64(value), parsed <= UInt32.max else { return nil }
        return UInt32(parsed)
    }

    private func unsigned64(_ value: Any?) -> UInt64? {
        guard let number = value as? NSNumber else { return nil }
        return UInt64(number.stringValue)
    }

    private func signed64(_ value: Any?) -> Int64? {
        guard let number = value as? NSNumber else { return nil }
        return Int64(number.stringValue)
    }

    private func presentationCategory(
        _ value: String
    ) -> JetPresentationErrorCategory? {
        switch value {
        case "invalid_input": .invalidInput
        case "unauthorized": .unauthorized
        case "conflict": .conflict
        case "unavailable": .unavailable
        case "incompatible": .incompatible
        case "rate_limited": .rateLimited
        case "not_found": .notFound
        case "outcome_unknown": .outcomeUnknown
        case "internal": .internalFailure
        default: nil
        }
    }

    private func isOffline(_ error: Error) -> Bool {
        guard case let JetClientFailure.presentation(presentation) = normalized(error) else {
            return false
        }
        return presentation.category == .offline
    }

    private func stateForFailure(_ failure: JetClientFailure) -> JetConnectionState {
        switch failure {
        case let .presentation(error) where error.category == .offline:
            .disconnected
        case let .presentation(error):
            .failed(error)
        case .commandOutcomeUnknown:
            .disconnected
        }
    }

    private func normalized(_ error: Error) -> JetClientFailure {
        if let error = error as? JetClientFailure { return error }
        if error is CancellationError { return .presentation(.cancelled) }
        if error is JetTransportFailure { return .presentation(.offline) }
        if error is JetIdentityFailure {
            return .presentation(JetPresentationError(
                category: .unavailable,
                code: "credential.keychain_unavailable",
                message: "Jet could not use this installation's pairing key in Keychain.",
                retryable: true
            ))
        }
        if error is JetFrameFailure || error is JetWireValidationFailure {
            return .presentation(.invalidResponse)
        }
        // ASVS 13.4.1 and 16.5.1: native error strings do not cross the
        // transport boundary or drive presentation behavior.
        return .presentation(.invalidResponse)
    }
}
