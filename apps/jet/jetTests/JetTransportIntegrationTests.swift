import Foundation
@preconcurrency import Network
import Testing
@testable import jet

struct JetTransportIntegrationTests {
    @Test("Recovery and retention use the negotiated, schema-checked wire forms")
    func recoveryAndRetentionWire() async throws {
        let server = HermeticJetd()
        let client = JetClient(
            configuration: JetClientConfiguration(clientID: UUID(), reconnectDelays: [.zero]),
            schema: try JetWireSchema.bundled(),
            makeTransport: { HermeticJetdTransport(server: server) }
        )
        try await client.connect()
        defer { Task { await client.disconnect() } }

        let conversationID = UUID(uuidString: "00000000-0000-0000-0000-000000000020")!
        let health = try await client.systemHealth()
        #expect(health.recoveryState == "serving")
        #expect(health.snapshots.map(\.reason) == ["daily"])
        #expect(health.auditIntegrity == .trusted)

        let trash = try await client.conversationTrash()
        #expect(trash.entries.first?.conversationID == conversationID)
        let preview = try await client.retentionPreview(conversationID: conversationID)
        #expect(preview.protections == ["dirty_workspace"])
        #expect(preview.auditRecords == 2)
        let rule = try #require(try await client.autodeleteRules().rules.first)
        #expect(rule.state == .draft(days: 30))
        #expect(rule.candidates.first?.protections == ["unpushed_work"])
        let audit = try await client.securityAudit(after: 0)
        #expect(audit.entries.first?.decision == "conversation.forgotten")

        let staged = try await client.stageConversation(conversationID, action: .forget, commandID: UUID())
        #expect(staged.reason == "manual_forget")
        try await client.restoreConversation(conversationID, commandID: UUID())
        try await client.approveAutodelete(ruleID: rule.id, days: 30, commandID: UUID())
        #expect(try await client.purgeRecoverySnapshots(commandID: UUID()) == ["older.sqlite3"])
        #expect(await server.recoveryCommandTypes == [
            "forget_conversation", "restore_conversation", "approve_autodelete_rule", "purge_recovery_snapshots",
        ])
    }

    @Test("The client retries a durable command and resumes ordered events")
    func commandReconnectAndEventResume() async throws {
        let server = HermeticJetd()
        let schema = try JetWireSchema.bundled()
        let clientID = UUID(uuidString: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")!
        let client = JetClient(
            configuration: JetClientConfiguration(
                clientID: clientID,
                reconnectDelays: [.zero, .zero, .zero],
                eventPollDelay: .seconds(60)
            ),
            schema: schema,
            makeTransport: { HermeticJetdTransport(server: server) }
        )

        try await client.connect()
        let status = try await client.status()
        #expect(status.cursor == 0)
        #expect(status.coreVersion == "0.2.0-test")

        let commandID = UUID(uuidString: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")!
        let energyConcurrency = SettingKey(rawValue: "energy.concurrency")!
        let cleared = try await client.clearSetting(
            energyConcurrency,
            scope: .plane,
            commandID: commandID
        )
        #expect(cleared == JetSettingCleared(key: energyConcurrency, scope: .plane))

        let events = try await firstThreeEvents(from: client)

        #expect(events.map(\.sequence) == [1, 2, 3])
        #expect(events[0].payload.source == #"{ "z": 1, "a": 2 }"#)
        #expect(await server.connectionCount == 3)
        #expect(await server.commandBodiesAreByteEquivalent)

        await server.rejectNextStatusRequest()
        do {
            _ = try await client.status()
            Issue.record("A stable server error was not surfaced")
        } catch let JetClientFailure.presentation(error) {
            #expect(error.category == .unauthorized)
            #expect(error.code == "test.denied")
            #expect(error.retryable == false)
        } catch {
            Issue.record("Unexpected server error: \(error)")
        }

        await server.holdNextStatusRequest()
        let cancelled = Task { try await client.status() }
        await server.waitForHeldStatusRequest()
        cancelled.cancel()
        do {
            _ = try await cancelled.value
            Issue.record("A cancelled Query unexpectedly completed")
        } catch let JetClientFailure.presentation(error) {
            #expect(error == .cancelled)
        } catch {
            Issue.record("Unexpected cancellation error: \(error)")
        }

        await client.disconnect()
        #expect(await client.currentState() == .disconnected)
    }

    @Test("Git delivery exposes every durable outcome and explicit commands")
    func gitDeliveryOutboxAndCommands() async throws {
        let server = HermeticJetd()
        let client = JetClient(
            configuration: JetClientConfiguration(
                clientID: UUID(),
                reconnectDelays: [.zero],
                eventPollDelay: .seconds(60)
            ),
            schema: try JetWireSchema.bundled(),
            makeTransport: { HermeticJetdTransport(server: server) }
        )
        try await client.connect()

        let conversationID = UUID(uuidString: "00000000-0000-0000-0000-000000000020")!
        let deliveries = try await client.gitDeliveries(conversationID: conversationID)
        #expect(deliveries.count == 4)
        #expect(deliveries[0].outcome == .pending)
        #expect(deliveries[1].outcome == .completed(
            head: String(repeating: "a", count: 40),
            branch: "work/reviewed",
            pullRequest: "https://github.com/example/jet/pull/7"
        ))
        #expect(deliveries[2].outcome == .failed(code: "git.policy_changed"))
        #expect(deliveries[2].canRetry)
        #expect(deliveries[3].outcome == .outcomeUnknown)
        #expect(deliveries[3].needsAcknowledgement)

        let queued = try await client.deliverGit(
            JetGitDeliveryRequest(
                conversationID: conversationID,
                checkpoint: JetGitCheckpoint(
                    runID: UUID(uuidString: "00000000-0000-0000-0000-000000000010")!,
                    turn: 2
                ),
                operation: .draftPullRequest(remote: "origin", base: "main")
            ),
            commandID: UUID()
        )
        #expect(queued.deliveryID == deliveries[0].id)

        let acknowledged = try await client.acknowledgeGitDelivery(
            deliveryID: deliveries[3].id,
            commandID: UUID()
        )
        #expect(acknowledged == deliveries[3].id)
        #expect(await server.gitCommandTypes == ["deliver_git", "acknowledge_git_delivery"])

        await client.disconnect()
    }

    @Test("The production validator accepts the shared valid corpus concurrently")
    func sharedSchemaValidatorIsConcurrencySafe() async throws {
        let schema = try JetWireSchema.bundled()
        let fixtureURL = repositoryRoot()
            .appending(path: "packages/jet-protocol/contracts/jet-fixtures.json")
        let fixtures = try JSONSerialization.jsonObject(
            with: Data(contentsOf: fixtureURL)
        ) as! [[String: Any]]
        let validFixtures = fixtures.compactMap { fixture -> (String, Data)? in
            guard fixture["valid"] as? Bool == true,
                  let definition = fixture["schema"] as? String,
                  let payload = fixture["payload"] as? String
            else {
                return nil
            }
            return (definition, Data(payload.utf8))
        }

        let failures = await withTaskGroup(of: String?.self) { group in
            for (index, fixture) in validFixtures.enumerated() {
                let (definition, payload) = fixture
                group.addTask {
                    do {
                        _ = try schema.validate(payload, definition: definition)
                        return nil
                    } catch {
                        return "fixture \(index) (\(definition)): \(error)"
                    }
                }
            }
            var failures: [String] = []
            for await result in group {
                if let result { failures.append(result) }
            }
            return failures
        }
        #expect(failures.isEmpty, Comment(rawValue: failures.joined(separator: "\n")))
    }

    @Test("Work queries and terminal frames share one multiplexed connection")
    func workQueriesAndTerminalFrames() async throws {
        let server = HermeticJetd()
        let client = JetClient(
            configuration: JetClientConfiguration(
                clientID: UUID(),
                reconnectDelays: [.zero],
                eventPollDelay: .seconds(60)
            ),
            schema: try JetWireSchema.bundled(),
            makeTransport: { HermeticJetdTransport(server: server) }
        )
        try await client.connect()

        let runID = UUID(uuidString: "00000000-0000-0000-0000-000000000010")!
        let diff = try await client.changeDiff(runID: runID, scope: .current)
        #expect(diff.runID == runID)
        #expect(diff.files.map(\.path) == ["Sources/App.swift"])
        #expect(diff.files.first?.status == "modified")

        let turnDiff = try await client.changeDiff(runID: runID, scope: .turn(1))
        #expect(turnDiff.scope == .turn(1))
        let historicalDiff = try await client.changeDiff(
            runID: runID,
            scope: .historical(fromTurn: 0, toTurn: 1)
        )
        #expect(historicalDiff.scope == .historical(fromTurn: 0, toTurn: 1))

        let terminalID = UUID(uuidString: "00000000-0000-0000-0000-000000000020")!
        let stream = try await client.attachTerminal(terminalID: terminalID, after: 0)
        var events: [JetTerminalEvent] = []
        for try await event in stream {
            events.append(event)
        }
        #expect(events == [
            .attached,
            .output(offset: 0, bytes: Data("hello".utf8)),
            .finished(totalBytes: 5),
        ])
        await client.disconnect()
    }

    @Test("Terminal transcript decoding survives split UTF-8 and escape sequences")
    func terminalTranscriptDecodingIsIncrementalAndSafe() {
        var decoder = TerminalTranscriptDecoder()
        let prefix = Data([0x68, 0x69, 0x20, 0xF0, 0x9F])
        let suffix = Data([0x91, 0x8B, 0x1B, 0x5B, 0x33])
        let final = Data([0x31, 0x6D, 0x21, 0x1B, 0x5D, 0x30, 0x3B, 0x78, 0x07])

        #expect(decoder.decode(prefix).isEmpty == false)
        #expect(decoder.decode(suffix) == "👋")
        #expect(decoder.decode(final) == "!")
        #expect(decoder.decode(Data(), final: true).isEmpty)
    }

    @Test("The production validator matches every shared fixture decision")
    func productionValidatorMatchesSharedCorpus() throws {
        let schema = try JetWireSchema.bundled()
        let fixtureURL = repositoryRoot()
            .appending(path: "packages/jet-protocol/contracts/jet-fixtures.json")
        let fixtures = try JSONSerialization.jsonObject(
            with: Data(contentsOf: fixtureURL)
        ) as! [[String: Any]]

        for (index, fixture) in fixtures.enumerated() {
            let definition = fixture["schema"] as! String
            let payload = Data((fixture["payload"] as! String).utf8)
            let expected = fixture["valid"] as! Bool
            let accepted = (try? schema.validate(payload, definition: definition)) != nil
            #expect(
                accepted == expected,
                Comment(rawValue: "fixture \(index) (\(definition))")
            )
        }
    }

    @Test("Oversized frame declarations fail before payload reads")
    func oversizedFrameIsRejectedBeforeAllocation() async {
        let transport = ReadCountingTransport(
            bytes: Data([0, 0, 0x10, 0, 1])
        )
        do {
            _ = try await JetFrameCodec.read(from: transport, multiplexed: false)
            Issue.record("An oversized frame was accepted")
        } catch let JetFrameFailure.oversized(declared, limit) {
            #expect(declared == 1_048_577)
            #expect(limit == 1_048_576)
        } catch {
            Issue.record("Unexpected frame error: \(error)")
        }
        #expect(await transport.readCount == 1)
    }

    @Test("The production transport connects over a UNIX-domain socket")
    func unixDomainSocketRoundTrip() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appending(path: UUID().uuidString, directoryHint: .isDirectory)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: false
        )
        defer { try? FileManager.default.removeItem(at: directory) }

        let socketURL = directory.appending(path: "jetd.sock")
        let server = try UnixEchoServer(socketURL: socketURL)
        try await server.start()
        defer { server.stop() }

        let transport = JetUnixSocketTransport(socketURL: socketURL)
        try await transport.connect()
        try await transport.write(Data("ping".utf8))
        let reply = try await transport.readExactly(4)
        #expect(String(data: reply, encoding: .utf8) == "pong")
        await transport.close()
    }

    private func repositoryRoot() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    private func firstThreeEvents(from client: JetClient) async throws -> [JetEvent] {
        let stream = await client.eventStream(after: 0)
        var events: [JetEvent] = []
        for try await event in stream {
            events.append(event)
            if events.count == 3 { return events }
        }
        return events
    }
}

/// Network.framework invokes these callbacks only on `queue`; the unchecked
/// conformance is contained to this test adapter and exercised by the UDS test.
private final class UnixEchoServer: @unchecked Sendable {
    private let queue = DispatchQueue(label: "me.heeka.jet.tests.unix-server")
    private let listener: NWListener
    private var connection: NWConnection?

    init(socketURL: URL) throws {
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .unix(path: socketURL.path)
        listener = try NWListener(using: parameters)
    }

    func start() async throws {
        try await withCheckedThrowingContinuation { continuation in
            var completed = false
            listener.stateUpdateHandler = { state in
                guard !completed else { return }
                switch state {
                case .ready:
                    completed = true
                    continuation.resume()
                case let .failed(error):
                    completed = true
                    continuation.resume(throwing: error)
                case .cancelled:
                    completed = true
                    continuation.resume(throwing: JetTransportFailure.closed)
                default:
                    break
                }
            }
            listener.newConnectionHandler = { [weak self] connection in
                self?.accept(connection)
            }
            listener.start(queue: queue)
        }
        listener.stateUpdateHandler = nil
    }

    func stop() {
        connection?.cancel()
        listener.cancel()
    }

    private func accept(_ connection: NWConnection) {
        self.connection = connection
        connection.stateUpdateHandler = { [weak connection] state in
            guard case .ready = state else { return }
            connection?.receive(
                minimumIncompleteLength: 4,
                maximumLength: 4
            ) { data, _, _, _ in
                guard data == Data("ping".utf8) else {
                    connection?.cancel()
                    return
                }
                connection?.send(
                    content: Data("pong".utf8),
                    completion: .contentProcessed { _ in }
                )
            }
        }
        connection.start(queue: queue)
    }
}

private actor ReadCountingTransport: JetByteTransport {
    private let bytes: Data
    private(set) var readCount = 0

    init(bytes: Data) {
        self.bytes = bytes
    }

    func connect() {}

    func readExactly(_ count: Int) throws -> Data {
        readCount += 1
        return Data(bytes.prefix(count))
    }

    func write(_ data: Data) {}
    func close() {}
}

private actor HermeticJetd {
    enum Action: Sendable {
        case reply(Data)
        case disconnect
        case hold
    }

    private(set) var connectionCount = 0
    private var commandBodies: [Data] = []
    private(set) var gitCommandTypes: [String] = []
    private(set) var recoveryCommandTypes: [String] = []
    private var disconnectedEventPage = false
    private var shouldHoldStatus = false
    private var shouldRejectStatus = false
    private var heldStatusContinuation: CheckedContinuation<Void, Never>?
    private var heldStatusSeen = false

    var commandBodiesAreByteEquivalent: Bool {
        commandBodies.count == 2 && commandBodies[0] == commandBodies[1]
    }

    func openedConnection() -> Int {
        connectionCount += 1
        return connectionCount
    }

    func holdNextStatusRequest() {
        shouldHoldStatus = true
        heldStatusSeen = false
    }

    func rejectNextStatusRequest() {
        shouldRejectStatus = true
    }

    func waitForHeldStatusRequest() async {
        if heldStatusSeen { return }
        await withCheckedContinuation { continuation in
            heldStatusContinuation = continuation
        }
    }

    func handle(_ frameData: Data, connection: Int) throws -> Action {
        let frame = try decodeFrame(frameData)
        let request = try JSONSerialization.jsonObject(with: frame.payload) as! [String: Any]
        if request["type"] as? String == "credit" { return .hold }
        let requestID = (request["id"] as! NSNumber).uint64Value
        switch request["kind"] as! String {
        case "query":
            let query = request["query"] as! [String: Any]
            switch query["type"] as! String {
            case "status":
                if shouldRejectStatus {
                    shouldRejectStatus = false
                    return .reply(try replyFrame(
                        streamID: frame.streamID,
                        object: [
                            "kind": "error",
                            "id": NSNumber(value: requestID),
                            "error": [
                                "category": "unauthorized",
                                "code": "test.denied",
                                "message": "This test request is not authorized.",
                                "retryable": false,
                            ],
                        ]
                    ))
                }
                if shouldHoldStatus {
                    shouldHoldStatus = false
                    heldStatusSeen = true
                    heldStatusContinuation?.resume()
                    heldStatusContinuation = nil
                    return .hold
                }
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "query_result",
                        "id": NSNumber(value: requestID),
                        "result": [
                            "type": "status",
                            "cursor": "0",
                            "plane_id": "00000000-0000-0000-0000-000000000001",
                            "daemon_starts": 1,
                            "started_at_unix_ms": 0,
                            "core_version": "0.2.0-test",
                            "security": ["state": "trusted"],
                            "recovery": [
                                "state": "serving",
                                "snapshots": [[
                                    "name": "plane-1-daily.sqlite3",
                                    "taken_at_unix_ms": 1,
                                    "reason": "daily",
                                    "bytes": 1_024,
                                ]],
                                "deletion_ledger": ["state": "verified", "deletions": 1],
                            ],
                        ],
                    ]
                ))
            case "capabilities":
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "query_result", "id": NSNumber(value: requestID),
                        "result": [
                            "type": "capabilities", "observed_at_unix_ms": 1,
                            "core_version": "0.2.0-test",
                            "platform": ["operating_system": "macos", "architecture": "aarch64"],
                            "external_tools": [], "credential_store": ["status": "available", "kind": "apple_keychain"],
                            "crafts": [], "harnesses": [], "degraded": [],
                        ],
                    ]
                ))
            case "conversation_trash":
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "query_result", "id": NSNumber(value: requestID),
                        "result": ["type": "conversation_trash", "cursor": "4", "entries": [trashEntry()]],
                    ]
                ))
            case "retention_preview":
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "query_result", "id": NSNumber(value: requestID),
                        "result": [
                            "type": "retention_preview", "conversation_id": query["conversation_id"]!,
                            "protections": ["dirty_workspace"], "audit_records": 2,
                            "trash": NSNull(),
                        ],
                    ]
                ))
            case "autodelete_rules":
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "query_result", "id": NSNumber(value: requestID),
                        "result": [
                            "type": "autodelete_rules", "cursor": "4",
                            "rules": [["rule": autodeleteRule(), "candidates": [[
                                "conversation_id": "00000000-0000-0000-0000-000000000020",
                                "last_active_at_unix_ms": 1, "protections": ["unpushed_work"],
                            ]]]],
                        ],
                    ]
                ))
            case "security_audit":
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "query_result", "id": NSNumber(value: requestID),
                        "result": ["type": "security_audit", "cursor": "1", "entries": [[
                            "sequence": "1", "epoch": "1",
                            "record_id": "00000000-0000-0000-0000-000000000050",
                            "recorded_at_unix_ms": 1,
                            "plane_id": "00000000-0000-0000-0000-000000000001",
                            "actor": ["type": "retention"],
                            "target": ["kind": "conversation", "reference": String(repeating: "a", count: 64)],
                            "decision": "conversation.forgotten", "risk": "destructive", "outcome": "succeeded",
                        ]]],
                    ]
                ))
            case "events":
                let after = query["after"] as! String
                if after == "0" {
                    return .reply(try eventReply(
                        streamID: frame.streamID,
                        requestID: requestID,
                        cursor: 2,
                        sequences: [1, 2]
                    ))
                }
                if after == "2", !disconnectedEventPage {
                    disconnectedEventPage = true
                    return .disconnect
                }
                if after == "2" {
                    return .reply(try eventReply(
                        streamID: frame.streamID,
                        requestID: requestID,
                        cursor: 3,
                        sequences: [3]
                    ))
                }
                return .reply(try eventReply(
                    streamID: frame.streamID,
                    requestID: requestID,
                    cursor: UInt64(after)!,
                    sequences: []
                ))
            case "change_diff":
                let runID = query["run_id"] as! String
                let scope = query["scope"] as! [String: Any]
                let emptySHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                let snapshot: [String: Any] = [
                    "content_complete": true,
                    "commit": String(repeating: "a", count: 40),
                    "tree": String(repeating: "b", count: 40),
                    "uncommitted": [
                        "sha256": emptySHA256,
                        "size": 0,
                    ],
                ]
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "query_result",
                        "id": NSNumber(value: requestID),
                        "result": [
                            "type": "change_diff",
                            "total_files": 1,
                            "cursor": "4",
                            "plane_id": "00000000-0000-0000-0000-000000000001",
                            "run_id": runID,
                            "workspace_id": "00000000-0000-0000-0000-000000000030",
                            "scope": scope,
                            "latest_turn": 1,
                            "before": snapshot,
                            "after": snapshot,
                            "files": [[
                                "path": "Sources/App.swift",
                                "before_mode": "100644",
                                "after_mode": "100644",
                                "before_object": String(repeating: "c", count: 40),
                                "after_object": String(repeating: "d", count: 40),
                                "origin": ["kind": "external_or_unknown"],
                            ]],
                            "next_page": NSNull(),
                            "artifact": [
                                "sha256": emptySHA256,
                                "size": 0,
                                "availability": "stored",
                            ],
                            "patch": "",
                            "patch_truncated": false,
                            "outcome": NSNull(),
                        ],
                    ]
                ))
            case "git_deliveries":
                let conversationID = query["conversation_id"] as! String
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "query_result",
                        "id": NSNumber(value: requestID),
                        "result": [
                            "type": "git_deliveries",
                            "deliveries": gitDeliveries(conversationID: conversationID),
                        ],
                    ]
                ))
            default:
                throw JetClientFailure.presentation(.invalidResponse)
            }
        case "attach_terminal":
            var frames = try replyFrame(
                streamID: frame.streamID,
                object: [
                    "kind": "terminal_attached",
                    "id": NSNumber(value: requestID),
                ]
            )
            frames.append(try JetFrameCodec.encode(
                JetFrame(
                    kind: .data,
                    streamID: frame.streamID,
                    payload: Data("hello".utf8)
                ),
                multiplexed: true,
                limits: .protocolMaximum
            ))
            frames.append(try replyFrame(
                streamID: frame.streamID,
                object: ["type": "terminal_finished", "total_bytes": "5"]
            ))
            return .reply(frames)
        case "command":
            let command = request["command"] as! [String: Any]
            let commandType = command["type"] as! String
            switch commandType {
            case "clear_setting":
                commandBodies.append(try JSONSerialization.data(
                    withJSONObject: command,
                    options: [.sortedKeys, .withoutEscapingSlashes]
                ))
                if commandBodies.count == 1 { return .disconnect }
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "command_result",
                        "id": NSNumber(value: requestID),
                        "result": [
                            "type": "setting_cleared",
                            "key": "energy.concurrency",
                            "scope": ["type": "plane"],
                        ],
                    ]
                ))
            case "deliver_git":
                gitCommandTypes.append(commandType)
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "command_result",
                        "id": NSNumber(value: requestID),
                        "result": [
                            "type": "git_delivery_queued",
                            "delivery_id": "00000000-0000-0000-0000-000000000040",
                        ],
                    ]
                ))
            case "acknowledge_git_delivery":
                gitCommandTypes.append(commandType)
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: [
                        "kind": "command_result",
                        "id": NSNumber(value: requestID),
                        "result": [
                            "type": "git_delivery_acknowledged",
                            "delivery_id": command["delivery_id"] as! String,
                        ],
                    ]
                ))
            case "forget_conversation", "restore_conversation", "approve_autodelete_rule", "purge_recovery_snapshots":
                recoveryCommandTypes.append(commandType)
                let result: [String: Any]
                switch commandType {
                case "forget_conversation":
                    result = ["type": "conversation_trashed", "entry": trashEntry()]
                case "restore_conversation":
                    result = ["type": "conversation_restored", "conversation_id": command["conversation_id"]!]
                case "approve_autodelete_rule":
                    var rule = autodeleteRule()
                    rule["state"] = ["state": "approved", "inactive_days": 30, "approved_at_unix_ms": 2]
                    result = ["type": "autodelete_rule_recorded", "rule": rule]
                default:
                    result = ["type": "recovery_snapshots_purged", "snapshot": "new.sqlite3", "removed": ["older.sqlite3"]]
                }
                return .reply(try replyFrame(
                    streamID: frame.streamID,
                    object: ["kind": "command_result", "id": NSNumber(value: requestID), "result": result]
                ))
            default:
                throw JetClientFailure.presentation(.invalidResponse)
            }
        default:
            throw JetClientFailure.presentation(.invalidResponse)
        }
    }

    private func trashEntry() -> [String: Any] {
        [
            "conversation_id": "00000000-0000-0000-0000-000000000020",
            "reason": "manual_forget", "trashed_at_unix_ms": 1,
            "expires_at_unix_ms": 1_000,
        ]
    }

    private func autodeleteRule() -> [String: Any] {
        [
            "rule_id": "00000000-0000-0000-0000-000000000060",
            "prompt": "Forget after a month", "utility_job_id": "00000000-0000-0000-0000-000000000061",
            "state": ["state": "draft", "inactive_days": 30], "scope": "forget",
            "created_at_unix_ms": 1, "updated_at_unix_ms": 1,
        ]
    }

    private func eventReply(
        streamID: UInt32,
        requestID: UInt64,
        cursor: UInt64,
        sequences: [UInt64]
    ) throws -> Data {
        let events = sequences.map { sequence in
            """
            {"sequence":"\(sequence)","event_id":"00000000-0000-0000-0000-00000000000\(sequence)","actor":{"type":"interactive_client","client_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"},"recorded_at_unix_ms":\(sequence),"kind":"test.event","payload_version":1,"payload":{ "z": 1, "a": 2 }}
            """
        }.joined(separator: ",")
        let payload = Data(
            """
            {"kind":"query_result","id":\(requestID),"result":{"type":"events","cursor":"\(cursor)","events":[\(events)]}}
            """.utf8
        )
        return try JetFrameCodec.encode(
            JetFrame(kind: .control, streamID: streamID, payload: payload),
            multiplexed: true,
            limits: .protocolMaximum
        )
    }

    private func gitDeliveries(conversationID: String) -> [[String: Any]] {
        let checkpoint: [String: Any] = [
            "run_id": "00000000-0000-0000-0000-000000000010",
            "turn": 2,
        ]
        let policy: [String: Any] = [
            "automatic": false,
            "branch": true,
            "commit": true,
            "push": true,
            "draft_pull_request": true,
            "branch_prefix": "jet/",
        ]
        func delivery(
            id: String,
            operation: [String: Any],
            outcome: [String: Any]
        ) -> [String: Any] {
            [
                "delivery_id": id,
                "conversation_id": conversationID,
                "checkpoint": checkpoint,
                "operation": operation,
                "policy": policy,
                "utility_job": NSNull(),
                "message": NSNull(),
                "acknowledged_by": NSNull(),
                "outcome": outcome,
            ]
        }
        return [
            delivery(
                id: "00000000-0000-0000-0000-000000000040",
                operation: ["operation": "branch", "name": "work/reviewed"],
                outcome: ["status": "pending"]
            ),
            delivery(
                id: "00000000-0000-0000-0000-000000000041",
                operation: ["operation": "draft_pull_request", "remote": "origin", "base": "main"],
                outcome: [
                    "status": "completed",
                    "head": String(repeating: "a", count: 40),
                    "branch": "work/reviewed",
                    "pull_request": "https://github.com/example/jet/pull/7",
                ]
            ),
            delivery(
                id: "00000000-0000-0000-0000-000000000042",
                operation: ["operation": "commit"],
                outcome: ["status": "failed", "code": "git.policy_changed"]
            ),
            delivery(
                id: "00000000-0000-0000-0000-000000000043",
                operation: ["operation": "push", "remote": "origin"],
                outcome: ["status": "outcome_unknown"]
            ),
        ]
    }

    private func replyFrame(
        streamID: UInt32,
        object: [String: Any]
    ) throws -> Data {
        let payload = try JSONSerialization.data(
            withJSONObject: object,
            options: [.sortedKeys, .withoutEscapingSlashes]
        )
        return try JetFrameCodec.encode(
            JetFrame(kind: .control, streamID: streamID, payload: payload),
            multiplexed: true,
            limits: .protocolMaximum
        )
    }

    private func decodeFrame(_ data: Data) throws -> JetFrame {
        guard data.count >= 9, let kind = JetFrameKind(rawValue: data[0]) else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let bytes = Array(data)
        let streamID = uint32(bytes, at: 1)
        let length = Int(uint32(bytes, at: 5))
        guard data.count == 9 + length else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return JetFrame(
            kind: kind,
            streamID: streamID,
            payload: data.subdata(in: 9..<data.count)
        )
    }

    private func uint32(_ bytes: [UInt8], at offset: Int) -> UInt32 {
        (UInt32(bytes[offset]) << 24)
            | (UInt32(bytes[offset + 1]) << 16)
            | (UInt32(bytes[offset + 2]) << 8)
            | UInt32(bytes[offset + 3])
    }
}

private actor HermeticJetdTransport: JetByteTransport {
    private enum Phase {
        case new
        case prefaced
        case active
    }

    private let server: HermeticJetd
    private var phase: Phase = .new
    private var connectionID = 0
    private var inbound = Data()
    private var closed = false
    private var waiter: (count: Int, continuation: CheckedContinuation<Data, Error>)?

    init(server: HermeticJetd) {
        self.server = server
    }

    func connect() async {
        connectionID = await server.openedConnection()
    }

    func write(_ data: Data) async throws {
        guard !closed else { throw JetTransportFailure.closed }
        switch phase {
        case .new:
            guard data == Data("jet-protocol\n".utf8) else {
                throw JetTransportFailure.closed
            }
            phase = .prefaced
        case .prefaced:
            guard data.count >= 5, data[0] == 0 else {
                throw JetTransportFailure.closed
            }
            let payload = try JSONSerialization.data(
                withJSONObject: [
                    "kind": "welcome",
                    "protocol": 1,
                    "minor": 43,
                    "codec": "json-v1",
                    "max_control_frame": 1_048_576,
                    "max_data_frame": 262_144,
                    "capabilities": [],
                ],
                options: [.sortedKeys]
            )
            let response = try JetFrameCodec.encode(
                JetFrame(kind: .control, streamID: 0, payload: payload),
                multiplexed: false,
                limits: .protocolMaximum
            )
            phase = .active
            enqueue(response)
        case .active:
            switch try await server.handle(data, connection: connectionID) {
            case let .reply(response): enqueue(response)
            case .disconnect: fail()
            case .hold: break
            }
        }
    }

    func readExactly(_ count: Int) async throws -> Data {
        if inbound.count >= count { return take(count) }
        guard !closed, waiter == nil else { throw JetTransportFailure.closed }
        return try await withCheckedThrowingContinuation { continuation in
            waiter = (count, continuation)
        }
    }

    func close() {
        fail()
    }

    private func enqueue(_ data: Data) {
        inbound.append(data)
        guard let waiter, inbound.count >= waiter.count else { return }
        self.waiter = nil
        waiter.continuation.resume(returning: take(waiter.count))
    }

    private func take(_ count: Int) -> Data {
        let result = Data(inbound.prefix(count))
        inbound.removeFirst(count)
        return result
    }

    private func fail() {
        closed = true
        waiter?.continuation.resume(throwing: JetTransportFailure.closed)
        waiter = nil
    }
}
