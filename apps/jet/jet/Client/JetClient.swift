import Foundation

typealias JetTransportFactory = @Sendable () -> any JetByteTransport

actor JetClient {
    private struct PendingReply {
        var continuation: CheckedContinuation<Data, Error>?
        var sent: Bool
        let cancellationFailure: JetClientFailure
    }

    private static let preface = Data("jet-protocol\n".utf8)
    private static let maximumInFlightRequests = 256
    private static let protocolVersion = JetClientConfiguration.protocolVersion
    private static let protocolMinor = JetClientConfiguration.protocolMinor
    private static let codec = JetClientConfiguration.codec

    private let configuration: JetClientConfiguration
    private let schema: JetWireSchema
    private let makeTransport: JetTransportFactory

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

    init(
        configuration: JetClientConfiguration,
        schema: JetWireSchema,
        makeTransport: @escaping JetTransportFactory
    ) {
        self.configuration = configuration
        self.schema = schema
        self.makeTransport = makeTransport
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

    func pairing() async throws -> JetPairingSummary {
        let (data, requestID) = try await sendQuery(["type": "pairing"])
        return try decodePairing(data, requestID: requestID)
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

    /// A durable Command keeps the same command ID and exact command object
    /// across reconnect attempts. Cancelling the caller never means rollback.
    func clearSetting(
        _ key: SettingKey,
        scope: JetSettingScope,
        commandID: UUID = UUID()
    ) async throws -> JetSettingCleared {
        let command = [
            "type": "clear_setting",
            "key": key.rawValue,
            "scope": wireScope(scope),
        ] as [String: Any]
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
                return try decodeSettingCleared(
                    response,
                    requestID: requestID,
                    expectedKey: key,
                    expectedScope: scope
                )
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
            try await candidate.write(Self.preface)
            try await candidate.write(
                JetFrameCodec.encode(
                    JetFrame(kind: .control, streamID: 0, payload: hello),
                    multiplexed: false,
                    limits: .protocolMaximum
                )
            )
            let frame = try await JetFrameCodec.read(
                from: candidate,
                multiplexed: false
            )
            guard frame.kind == .control, frame.streamID == 0 else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            let accepted = try decodeServerHello(frame.payload)

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
        try encode(
            [
                "protocol": [
                    "min": NSNumber(value: Self.protocolVersion),
                    "max": NSNumber(value: Self.protocolVersion),
                ],
                "minor": NSNumber(value: Self.protocolMinor),
                "codec": Self.codec,
                "client_id": configuration.clientID.uuidString.lowercased(),
                "max_control_frame": NSNumber(value: JetFrameLimits.protocolMaximum.control),
                "max_data_frame": NSNumber(value: JetFrameLimits.protocolMaximum.data),
                "capabilities": [],
            ],
            definition: "ClientHello"
        )
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
        let workspaceRoot: String?
        if let workspace = result["workspace"] as? [String: Any] {
            guard let root = workspace["root"] as? String else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            workspaceRoot = safeDisplayText(root, maximumBytes: 4_096, fallback: "Workspace")
        } else {
            workspaceRoot = nil
        }
        return JetConversationSnapshot(
            cursor: cursor,
            conversation: try decodeConversationSummary(conversation),
            workspaceRoot: workspaceRoot,
            runs: try runs.map(decodeRun)
        )
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
        return JetConversationSummary(
            id: id,
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
        let clients = result["clients"] as? [[String: Any]] ?? []
        return JetPairingSummary(
            cursor: cursor,
            gate: gate,
            pairedClients: clients.count,
            hasPendingOffer: result["pending"] != nil && !(result["pending"] is NSNull)
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
        let actions = try node.member("recovery_actions")?.elements?.map {
            try document.rawJSON(for: $0)
        } ?? []
        return JetPresentationError(
            category: category,
            code: code,
            message: message,
            retryable: retryable,
            recoveryActions: actions
        )
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
        if error is JetFrameFailure || error is JetWireValidationFailure {
            return .presentation(.invalidResponse)
        }
        // ASVS 13.4.1 and 16.5.1: native error strings do not cross the
        // transport boundary or drive presentation behavior.
        return .presentation(.invalidResponse)
    }
}
