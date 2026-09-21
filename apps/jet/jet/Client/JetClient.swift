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
                state = .failed(presentation)
            }
            throw failure
        }
    }

    private func openConnection() async throws {
        state = .connecting
        let candidate = makeTransport()
        do {
            try await candidate.connect()
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
            state = .connected(accepted)
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
        state = newState

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
        state = .reconnecting(attempt: attempt + 1)
        try await ContinuousClock().sleep(for: configuration.reconnectDelays[attempt])
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
