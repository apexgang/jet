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
