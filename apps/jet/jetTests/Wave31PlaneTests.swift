import Foundation
import Testing
@testable import jet

struct Wave31PlaneTests {
    @Test(arguments: [
        "studio",
        "alex@studio.example",
        "deploy@[2001:db8::1]",
        "host-with_zone%en0",
    ])
    func acceptsSafeSSHEndpoints(_ endpoint: String) throws {
        #expect(try JetSSHEndpoint.validated(endpoint) == endpoint)
    }

    @Test(arguments: [
        "",
        "-oProxyCommand=touch",
        "host name",
        "host;command",
        "user@@host",
        "user@-host",
    ])
    func rejectsSSHEndpointsThatCouldChangeTheCommand(_ endpoint: String) {
        do {
            _ = try JetSSHEndpoint.validated(endpoint)
            Issue.record("Unsafe SSH endpoint was accepted: \(endpoint)")
        } catch let JetClientFailure.presentation(error) {
            #expect(error.code == "remote.ssh_endpoint_invalid")
        } catch {
            Issue.record("Unexpected endpoint error: \(error)")
        }
    }

    @Test
    func clientHelloEncodingMatchesTheSignedWireTranscript() throws {
        let clientID = UUID(uuidString: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")!
        let hello = try JetHandshakeCodec.clientHello(
            clientID: clientID,
            schema: JetWireSchema.bundled()
        )

        #expect(String(decoding: hello, as: UTF8.self) ==
            #"{"protocol":{"min":1,"max":1},"minor":43,"codec":"json-v1","client_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","max_control_frame":1048576,"max_data_frame":262144,"capabilities":[]}"#
        )
    }

    @Test
    func remoteChallengeSignsTheDomainSeparatedHelloAndNonce() async throws {
        let clientID = UUID(uuidString: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")!
        let nonce = Data(repeating: 0x2a, count: 32)
        let signer = CapturingConnectionSigner(clientID: clientID)
        let transport = ChallengeTransport(nonce: nonce)
        let client = JetClient(
            configuration: JetClientConfiguration(clientID: clientID),
            schema: try JetWireSchema.bundled(),
            makeTransport: { transport },
            remoteSigner: signer
        )

        try await client.connect()
        let hello = try #require(await transport.clientHello)
        var expected = Data("jet.connection.v1\0ed25519\0".utf8)
        expected.append(hello)
        expected.append(nonce)

        #expect(await signer.signedBytes == expected)
        #expect(await transport.proofSignature == String(repeating: "5a", count: 64))
        await client.disconnect()
    }

    @Test
    func pairingWireKeepsClientAccessAndTargetConfirmationIdentity() throws {
        let clientID = UUID(uuidString: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")!
        let offerID = UUID(uuidString: "cccccccc-cccc-cccc-cccc-cccccccccccc")!
        let client = try #require(JetPairingWire.client([
            "client_id": clientID.uuidString.lowercased(),
            "access": "disabled",
            "paired_at_unix_ms": NSNumber(value: 123),
            "pairing_protocol": "direct-v1",
            "key": [
                "algorithm": "ed25519",
                "key": String(repeating: "ab", count: 32),
            ],
        ]))
        let pending = try #require(JetPairingWire.pending([
            "offer_id": offerID.uuidString.lowercased(),
            "method": ["method": "manual_code"],
            "progress": [
                "progress": "awaiting_confirmation",
                "client_id": clientID.uuidString.lowercased(),
                "authentication_string": "river-paper-sunset",
            ],
            "attempts_remaining": NSNumber(value: 4),
            "opened_at_unix_ms": NSNumber(value: 100),
            "expires_at_unix_ms": NSNumber(value: 200),
        ]))

        #expect(client.access == .disabled)
        #expect(client.id == clientID)
        #expect(pending.id == offerID)
        #expect(pending.progress.clientID == clientID)
        #expect(pending.progress.authenticationString == "river-paper-sunset")
    }

    @Test @MainActor
    func planeRegistryPersistsLabelsButNoPairingSecrets() throws {
        let suite = "Wave31PlaneTests.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = JetPlaneRegistryStore(defaults: defaults)
        let profile = JetRemotePlaneProfile(
            id: UUID(),
            name: "Studio",
            endpoint: "alex@studio.example",
            planeID: UUID()
        )

        store.save([profile])

        #expect(store.load() == [profile])
        let persisted = String(
            decoding: try #require(defaults.data(forKey: "jet.remote-planes.v1")),
            as: UTF8.self
        )
        #expect(!persisted.contains("signing_bytes"))
        #expect(!persisted.contains("secret"))
        #expect(!persisted.contains("private"))
    }
}

@MainActor
struct Wave31PairingTests {
    private let plane = UUID()

    @Test
    func allowingOpensTheGateOnlyWhenClosedAndClosesOnlyWhatItOpened() async {
        let closed = PairingAccessFake(gate: "closed")
        let model = makeModel(closed)
        await model.beginAllowing(on: plane)
        #expect(model.ownsGate)
        #expect(await closed.gateChanges == ["open"])
        await model.endAllowing()
        #expect(await closed.gateChanges == ["open", "closed"])
        #expect(!model.ownsGate)
        #expect(model.allowPhase == .idle)

        let open = PairingAccessFake(gate: "open")
        let other = makeModel(open)
        await other.beginAllowing(on: plane)
        #expect(!other.ownsGate)
        await other.endAllowing()
        #expect(await open.gateChanges.isEmpty)
        #expect(await open.gate == "open")
    }

    @Test
    func theSheetFollowsTheOfferUntilTheOtherMacIsListed() async throws {
        let access = PairingAccessFake(gate: "closed")
        let model = makeModel(access)
        let clientID = UUID()

        await model.beginAllowing(on: plane)
        #expect(model.allowPhase == .showingCode("4821-0937"))
        #expect(await model.pollOnce())
        #expect(model.allowPhase == .showingCode("4821-0937"))

        await access.claim(clientID: clientID, numbers: "482-913")
        #expect(await model.pollOnce())
        #expect(model.allowPhase == .comparing("482-913"))
        #expect(model.claimingClientID == clientID)

        await model.confirmCodesMatch()
        #expect(model.allowPhase == .waitingForOtherMac)
        #expect(await access.confirmedNumbers == ["482-913"])
        #expect(await model.pollOnce())
        #expect(model.allowPhase == .waitingForOtherMac)

        await access.complete()
        #expect(await model.pollOnce() == false)
        #expect(model.allowPhase == .connected)
        #expect(model.clients(of: plane, fallback: nil).map(\.id) == [clientID])
    }

    @Test
    func anExpiredCodeEndsTheSheet() async {
        let access = PairingAccessFake(gate: "closed")
        let model = makeModel(access)
        await model.beginAllowing(on: plane)

        await access.end("expired")
        #expect(await model.pollOnce() == false)
        #expect(model.allowPhase == .ended(.expired))

        // An offer that is simply gone after its expiry also reads as expired.
        let later = PairingAccessFake(gate: "closed")
        let clock = TestClock(now: Date(timeIntervalSince1970: 1_000))
        let other = makeModel(later, now: { clock.now })
        await other.beginAllowing(on: plane)
        clock.now = Date(timeIntervalSince1970: 1_000 + 121)
        await later.dropOffer()
        #expect(await other.pollOnce() == false)
        #expect(other.allowPhase == .ended(.expired))

        let tooMany = PairingAccessFake(gate: "closed")
        let third = makeModel(tooMany)
        await third.beginAllowing(on: plane)
        await tooMany.end("too_many_attempts")
        await third.pollOnce()
        #expect(third.allowPhase == .ended(.tooManyAttempts))
    }

    @Test
    func dontMatchClosesTheGateAndNeverConfirms() async {
        let access = PairingAccessFake(gate: "closed")
        let model = makeModel(access)
        await model.beginAllowing(on: plane)
        await access.claim(clientID: UUID(), numbers: "482-913")
        await model.pollOnce()
        #expect(model.allowPhase == .comparing("482-913"))

        await model.codesDontMatch()

        #expect(await access.confirmedNumbers.isEmpty)
        #expect(await access.gate == "closed")
        #expect(model.allowPhase == .ended(.stopped))
        #expect(!model.ownsGate)
        await model.endAllowing()
        #expect(await access.gateChanges == ["open", "closed"])
    }

    @Test
    func anUnconfirmedMatchKeepsItsCommandIDForTheExplicitRetry() async {
        let access = PairingAccessFake(gate: "closed")
        let model = makeModel(access)
        await model.beginAllowing(on: plane)
        await access.claim(clientID: UUID(), numbers: "482-913")
        await model.pollOnce()

        await access.failNextConfirm(.commandOutcomeUnknown(commandID: UUID()))
        await model.confirmCodesMatch()
        #expect(model.issue?.category == .outcomeUnknown)
        #expect(model.allowPhase == .comparing("482-913"))
        #expect(await access.confirmCommandIDs.count == 1)

        await model.confirmCodesMatch()
        let ids = await access.confirmCommandIDs
        #expect(ids.count == 2)
        #expect(ids[0] == ids[1])
        #expect(model.allowPhase == .waitingForOtherMac)
    }

    @Test(arguments: [
        ("1234-5678", true),
        ("12345678", true),
        (" 1234 5678 ", true),
        ("123", false),
        ("1234-567", false),
        ("1234-567a", false),
        ("123456789", false),
        ("", false),
    ])
    func pairingCodesAreEightDigits(_ code: String, valid: Bool) {
        #expect(PairingCode.isValid(code) == valid)
    }

    private func makeModel(
        _ access: PairingAccessFake,
        now: @escaping @Sendable () -> Date = { Date(timeIntervalSince1970: 1_000) }
    ) -> ComputerPairingModel {
        ComputerPairingModel(
            makeAccess: { _ in access },
            now: now,
            sleep: { _ in throw CancellationError() }
        )
    }
}

private final class TestClock: @unchecked Sendable {
    // Written only from the test's main actor before the model reads it.
    var now: Date

    init(now: Date) {
        self.now = now
    }
}

/// One computer's Pairing: a gate, at most one offer and the paired Jet apps.
private actor PairingAccessFake: JetPairingAccess {
    private(set) var gate: String
    private(set) var gateChanges: [String] = []
    private(set) var confirmedNumbers: [String] = []
    private(set) var confirmCommandIDs: [UUID] = []
    private var pending: JetPendingPairing?
    private var clients: [JetPairedClientSummary] = []
    private var nextConfirmFailure: JetClientFailure?
    private let offerID = UUID()

    init(gate: String) {
        self.gate = gate
    }

    func claim(clientID: UUID, numbers: String) {
        pending = offer(.awaitingConfirmation(clientID: clientID, authenticationString: numbers))
    }

    func complete() {
        guard let clientID = pending?.progress.clientID else { return }
        pending = nil
        clients.append(JetPairedClientSummary(
            id: clientID,
            access: .enabled,
            pairedAtUnixMilliseconds: 1_000_000,
            pairingProtocol: "jet.pairing.v1",
            publicKey: String(repeating: "ab", count: 32)
        ))
    }

    func end(_ reason: String) {
        pending = offer(.ended(reason: reason))
    }

    func dropOffer() {
        pending = nil
    }

    func failNextConfirm(_ failure: JetClientFailure) {
        nextConfirmFailure = failure
    }

    func pairing() async throws -> JetPairingSummary {
        JetPairingSummary(cursor: 1, gate: gate, clients: clients, pending: pending)
    }

    func setPairingGate(_ gate: String, commandID: UUID) async throws -> String {
        gateChanges.append(gate)
        self.gate = gate
        if gate == "closed", pending != nil {
            pending = offer(.ended(reason: "gate_closed"))
        }
        return gate
    }

    func openManualPairing(commandID: UUID) async throws -> JetOpenedPairing {
        guard gate == "open" else {
            throw JetClientFailure.presentation(.invalidInput(code: "pairing.gate_closed", message: "Closed."))
        }
        let offered = offer(.offered)
        pending = offered
        return JetOpenedPairing(disclosure: .manualCode("4821-0937"), pending: offered)
    }

    func confirmPairing(
        offerID: UUID,
        authenticationString: String,
        commandID: UUID
    ) async throws -> JetPendingPairing {
        confirmCommandIDs.append(commandID)
        if let failure = nextConfirmFailure {
            nextConfirmFailure = nil
            throw failure
        }
        guard let clientID = pending?.progress.clientID else {
            throw JetClientFailure.presentation(.invalidInput(code: "pairing.offer_ended", message: "Ended."))
        }
        confirmedNumbers.append(authenticationString)
        let confirmed = offer(.confirmed(clientID: clientID, authenticationString: authenticationString))
        pending = confirmed
        return confirmed
    }

    func setPairedClientAccess(
        clientID: UUID,
        access: JetPairedClientAccess,
        commandID: UUID
    ) async throws -> JetPairedClientSummary {
        throw JetClientFailure.presentation(.invalidInput(code: "pairing.client_missing", message: "Missing."))
    }

    func revokePairedClient(clientID: UUID, commandID: UUID) async throws -> UUID {
        clients.removeAll { $0.id == clientID }
        return clientID
    }

    private func offer(_ progress: JetPairingProgress) -> JetPendingPairing {
        JetPendingPairing(
            id: offerID,
            method: "manual_code",
            progress: progress,
            attemptsRemaining: 5,
            openedAtUnixMilliseconds: 1_000_000,
            expiresAtUnixMilliseconds: 1_120_000
        )
    }
}

private actor CapturingConnectionSigner: JetConnectionSigning {
    nonisolated let clientID: UUID
    private(set) var signedBytes: Data?

    init(clientID: UUID) {
        self.clientID = clientID
    }

    func publicKey() -> Data { Data(repeating: 0x11, count: 32) }

    func sign(_ bytes: Data) -> Data {
        signedBytes = bytes
        return Data(repeating: 0x5a, count: 64)
    }
}

private actor ChallengeTransport: JetByteTransport {
    private enum Phase {
        case preface
        case hello
        case proof
        case active
        case closed
    }

    private let nonce: Data
    private var phase = Phase.preface
    private var inbound = Data()
    private(set) var clientHello: Data?
    private(set) var proofSignature: String?

    init(nonce: Data) {
        self.nonce = nonce
    }

    func connect() {}

    func write(_ data: Data) throws {
        switch phase {
        case .preface:
            guard data == JetHandshakeCodec.preface else { throw JetTransportFailure.closed }
            phase = .hello
        case .hello:
            clientHello = try decodePayload(data, multiplexed: false)
            enqueue(try frame([
                "kind": "challenge",
                "nonce": nonce.hexadecimal,
            ]))
            phase = .proof
        case .proof:
            let payload = try decodePayload(data, multiplexed: false)
            let value = try JSONSerialization.jsonObject(with: payload) as? [String: Any]
            proofSignature = value?["signature"] as? String
            enqueue(try frame([
                "kind": "welcome",
                "protocol": 1,
                "minor": 43,
                "codec": "json-v1",
                "max_control_frame": 1_048_576,
                "max_data_frame": 262_144,
                "capabilities": [],
            ]))
            phase = .active
        case .active, .closed:
            throw JetTransportFailure.closed
        }
    }

    func readExactly(_ count: Int) throws -> Data {
        guard count >= 0 else { throw JetTransportFailure.invalidReadLength }
        guard inbound.count >= count else { throw JetTransportFailure.closed }
        let result = Data(inbound.prefix(count))
        inbound.removeFirst(count)
        return result
    }

    func close() {
        phase = .closed
        inbound = Data()
    }

    private func enqueue(_ data: Data) {
        inbound.append(data)
    }

    private func frame(_ object: [String: Any]) throws -> Data {
        let payload = try JSONSerialization.data(
            withJSONObject: object,
            options: [.sortedKeys, .withoutEscapingSlashes]
        )
        return try JetFrameCodec.encode(
            JetFrame(kind: .control, streamID: 0, payload: payload),
            multiplexed: false,
            limits: .protocolMaximum
        )
    }

    private func decodePayload(_ data: Data, multiplexed: Bool) throws -> Data {
        let headerSize = multiplexed ? 9 : 5
        guard data.count >= headerSize, data[0] == JetFrameKind.control.rawValue else {
            throw JetTransportFailure.closed
        }
        let lengthOffset = multiplexed ? 5 : 1
        let bytes = Array(data)
        let length = (Int(bytes[lengthOffset]) << 24)
            | (Int(bytes[lengthOffset + 1]) << 16)
            | (Int(bytes[lengthOffset + 2]) << 8)
            | Int(bytes[lengthOffset + 3])
        guard data.count == headerSize + length else { throw JetTransportFailure.closed }
        return data.subdata(in: headerSize..<data.count)
    }
}
