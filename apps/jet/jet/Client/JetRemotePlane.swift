import CryptoKit
@preconcurrency import Foundation
import Security

nonisolated enum JetIdentityFailure: Error, Sendable, Equatable {
    case keychainUnavailable
    case invalidKey
    case invalidSignature
}

nonisolated protocol JetConnectionSigning: Sendable {
    var clientID: UUID { get }
    func publicKey() async throws -> Data
    func sign(_ bytes: Data) async throws -> Data
}

actor JetKeychainSigningIdentity: JetConnectionSigning {
    nonisolated let clientID: UUID

    private static let service = "me.heeka.jet.pairing-identity"
    private var cachedKey: Curve25519.Signing.PrivateKey?

    init(clientID: UUID) {
        self.clientID = clientID
    }

    func publicKey() throws -> Data {
        try privateKey().publicKey.rawRepresentation
    }

    func sign(_ bytes: Data) throws -> Data {
        let signature = try privateKey().signature(for: bytes)
        guard signature.count == 64 else { throw JetIdentityFailure.invalidSignature }
        return signature
    }

    private func privateKey() throws -> Curve25519.Signing.PrivateKey {
        if let cachedKey { return cachedKey }

        let account = clientID.uuidString.lowercased()
        // ASVS 11.2.1, 11.6.1, 13.3.1, and 13.3.2: CryptoKit owns
        // Ed25519 generation/signing and Keychain is the only durable store.
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecSuccess {
            guard let data = result as? Data,
                  let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: data)
            else {
                throw JetIdentityFailure.invalidKey
            }
            cachedKey = key
            return key
        }
        guard status == errSecItemNotFound else {
            throw JetIdentityFailure.keychainUnavailable
        }

        let key = Curve25519.Signing.PrivateKey()
        let add: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: account,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData as String: key.rawRepresentation,
        ]
        let addStatus = SecItemAdd(add as CFDictionary, nil)
        if addStatus == errSecDuplicateItem {
            var duplicate: CFTypeRef?
            guard SecItemCopyMatching(query as CFDictionary, &duplicate) == errSecSuccess,
                  let data = duplicate as? Data,
                  let stored = try? Curve25519.Signing.PrivateKey(rawRepresentation: data)
            else {
                throw JetIdentityFailure.keychainUnavailable
            }
            cachedKey = stored
            return stored
        }
        guard addStatus == errSecSuccess else {
            throw JetIdentityFailure.keychainUnavailable
        }
        cachedKey = key
        return key
    }
}

nonisolated enum JetSSHEndpoint {
    static func validated(_ value: String) throws -> String {
        let parts = value.split(separator: "@", omittingEmptySubsequences: false)
        let allowed = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._:@[]%".utf8)
        guard !value.isEmpty,
              value.utf8.count <= 512,
              parts.count <= 2,
              parts.allSatisfy({ !$0.isEmpty && !$0.hasPrefix("-") }),
              value.utf8.allSatisfy({ allowed.contains($0) })
        else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "remote.ssh_endpoint_invalid",
                message: "Enter an SSH host alias or [user@]host without options, spaces, or shell syntax."
            ))
        }
        return value
    }
}

nonisolated enum JetRemotePlaneName {
    static func validated(_ value: String, fallback: String) throws -> String {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        let candidate = trimmed.isEmpty ? fallback : trimmed
        guard candidate.utf8.count <= 128,
              candidate.unicodeScalars.allSatisfy({
                  !CharacterSet.controlCharacters.contains($0)
              })
        else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "remote.name_invalid",
                message: "Use a Plane name without control characters and no longer than 128 bytes."
            ))
        }
        return candidate
    }
}

#if os(macOS)
actor JetSSHTransport: JetByteTransport {
    private let endpoint: String
    private var process: Process?
    private var input: FileHandle?
    private var output: FileHandle?

    init(endpoint: String) throws {
        self.endpoint = try JetSSHEndpoint.validated(endpoint)
    }

    init(validatedEndpoint: String) {
        endpoint = validatedEndpoint
    }

    func connect() throws {
        guard process == nil else { return }
        let process = Process()
        let inputPipe = Pipe()
        let outputPipe = Pipe()
        // ASVS 1.2.5 and 12.3.1: invoke the system SSH client with an
        // argument array, strict host-key checking, and no plaintext path.
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
        process.arguments = [
            "-T", "-a", "-S", "none",
            "-o", "StrictHostKeyChecking=yes",
            "-o", "VerifyHostKeyDNS=no",
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=10",
            "-o", "ConnectionAttempts=1",
            "-o", "ClearAllForwardings=yes",
            "-o", "PermitLocalCommand=no",
            "-o", "RemoteCommand=none",
            "--", endpoint, "jetd", "connect", "--stdio",
        ]
        process.standardInput = inputPipe
        process.standardOutput = outputPipe
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            throw JetTransportFailure.unavailable
        }
        self.process = process
        input = inputPipe.fileHandleForWriting
        output = outputPipe.fileHandleForReading
    }

    func readExactly(_ count: Int) async throws -> Data {
        guard count >= 0 else { throw JetTransportFailure.invalidReadLength }
        guard count > 0 else { return Data() }
        guard let output else { throw JetTransportFailure.closed }

        return try await withTaskCancellationHandler {
            var result = Data()
            result.reserveCapacity(count)
            while result.count < count {
                let remaining = count - result.count
                let chunk = try await Task.detached(priority: .userInitiated) {
                    try output.read(upToCount: remaining) ?? Data()
                }.value
                guard !chunk.isEmpty else { throw JetTransportFailure.closed }
                result.append(chunk)
            }
            return result
        } onCancel: {
            Task { await self.close() }
        }
    }

    func write(_ data: Data) throws {
        guard let input else { throw JetTransportFailure.closed }
        do {
            try input.write(contentsOf: data)
        } catch {
            throw JetTransportFailure.closed
        }
    }

    func close() {
        try? input?.close()
        try? output?.close()
        if process?.isRunning == true { process?.terminate() }
        input = nil
        output = nil
        process = nil
    }
}

actor JetRemotePlaneService {
    private let identity: any JetConnectionSigning

    init(identity: any JetConnectionSigning) {
        self.identity = identity
    }

    func connect(endpoint: String) async throws -> JetClient {
        try await JetClient.connectRemote(
            endpoint: endpoint,
            configuration: JetClientConfiguration(clientID: identity.clientID),
            signer: identity
        )
    }

    func claim(
        endpoint: String,
        secret: String,
        commandID: UUID
    ) async throws -> JetRemotePairingClaim {
        let trimmed = secret.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed.utf8.count <= 4_096 else {
            throw JetClientFailure.presentation(.invalidInput(
                code: "pairing.secret_invalid",
                message: "Enter the manual code or QR payload shown by the target Plane."
            ))
        }
        let publicKey = try await identity.publicKey()
        guard publicKey.count == 32 else { throw JetIdentityFailure.invalidKey }
        let request: [String: Any] = [
            "pairing": "claim",
            "command_id": commandID.uuidString.lowercased(),
            "secret": trimmed,
            "key": [
                "algorithm": "ed25519",
                "key": publicKey.hexadecimal,
            ],
        ]
        let response = try await exchange(endpoint: endpoint, request: request)
        guard response["kind"] as? String == "claimed",
              let pending = response["pending"] as? [String: Any],
              let decoded = JetPairingWire.pending(pending),
              let authenticationString = decoded.progress.authenticationString,
              let signingValues = response["signing_bytes"] as? [NSNumber]
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        let signingBytes = Data(signingValues.map(\.uint8Value))
        return JetRemotePairingClaim(
            endpoint: endpoint,
            offerID: decoded.id,
            authenticationString: authenticationString,
            signingBytes: signingBytes
        )
    }

    func complete(
        claim: JetRemotePairingClaim,
        commandID: UUID
    ) async throws -> JetPairedClientSummary {
        let signature = try await identity.sign(claim.signingBytes)
        guard signature.count == 64 else { throw JetIdentityFailure.invalidSignature }
        let response = try await exchange(
            endpoint: claim.endpoint,
            request: [
                "pairing": "complete",
                "command_id": commandID.uuidString.lowercased(),
                "offer_id": claim.offerID.uuidString.lowercased(),
                "signature": signature.hexadecimal,
            ]
        )
        guard response["kind"] as? String == "completed",
              let client = response["client"] as? [String: Any],
              let decoded = JetPairingWire.client(client)
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return decoded
    }

    private func exchange(
        endpoint: String,
        request: [String: Any]
    ) async throws -> [String: Any] {
        let schema = try JetWireSchema.bundled()
        let transport = try JetSSHTransport(endpoint: endpoint)
        do {
            try await transport.connect()
            let hello = try JetHandshakeCodec.clientHello(
                clientID: identity.clientID,
                schema: schema
            )
            try await transport.write(JetHandshakeCodec.preface)
            try await transport.write(JetFrameCodec.encode(
                JetFrame(kind: .control, streamID: 0, payload: hello),
                multiplexed: false,
                limits: .protocolMaximum
            ))
            let challenge = try await readHandshakeFrame(from: transport)
            guard challenge.kind == .control, challenge.streamID == 0 else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            _ = try JetHandshakeCodec.challenge(challenge.payload, schema: schema)

            let requestData = try JetHandshakeCodec.encode(
                request,
                definition: "RemotePairingRequest",
                schema: schema
            )
            try await transport.write(JetFrameCodec.encode(
                JetFrame(kind: .control, streamID: 0, payload: requestData),
                multiplexed: false,
                limits: .protocolMaximum
            ))
            let response = try await readHandshakeFrame(from: transport)
            guard response.kind == .control, response.streamID == 0 else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            let document = try schema.validate(
                response.payload,
                definition: "RemotePairingResponse"
            )
            guard let object = document.value as? [String: Any] else {
                throw JetClientFailure.presentation(.invalidResponse)
            }
            if object["kind"] as? String == "rejected" {
                throw JetClientFailure.presentation(
                    JetPairingWire.presentationError(object["error"])
                )
            }
            await transport.close()
            return object
        } catch {
            await transport.close()
            throw error
        }
    }

    private func readHandshakeFrame(from transport: JetSSHTransport) async throws -> JetFrame {
        // ASVS 13.2.6 and 16.5.2: a stalled SSH peer cannot hold pairing
        // forever; cancellation closes the Process pipes and fails closed.
        return try await withThrowingTaskGroup(of: JetFrame.self) { group in
            group.addTask {
                try await JetFrameCodec.read(from: transport, multiplexed: false)
            }
            group.addTask {
                try await Task.sleep(for: .seconds(12))
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
}
#endif

nonisolated enum JetHandshakeCodec {
    static let preface = Data("jet-protocol\n".utf8)

    static func clientHello(clientID: UUID, schema: JetWireSchema) throws -> Data {
        let text = "{\"protocol\":{\"min\":1,\"max\":1},\"minor\":\(JetClientConfiguration.protocolMinor),\"codec\":\"json-v1\",\"client_id\":\"\(clientID.uuidString.lowercased())\",\"max_control_frame\":1048576,\"max_data_frame\":262144,\"capabilities\":[]}"
        let data = Data(text.utf8)
        _ = try schema.validate(data, definition: "ClientHello")
        return data
    }

    static func challenge(_ data: Data, schema: JetWireSchema) throws -> Data {
        let document = try schema.validate(data, definition: "ServerHello")
        guard let object = document.value as? [String: Any],
              object["kind"] as? String == "challenge",
              let nonce = object["nonce"] as? String,
              let bytes = Data(hexadecimal: nonce),
              bytes.count == 32
        else {
            throw JetClientFailure.presentation(.invalidResponse)
        }
        return bytes
    }

    static func encode(
        _ object: [String: Any],
        definition: String,
        schema: JetWireSchema
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
}

nonisolated enum JetPairingWire {
    static func client(_ value: [String: Any]) -> JetPairedClientSummary? {
        guard let clientID = UUID(uuidString: value["client_id"] as? String ?? ""),
              let accessText = value["access"] as? String,
              let access = JetPairedClientAccess(rawValue: accessText),
              let pairedAt = (value["paired_at_unix_ms"] as? NSNumber)?.int64Value,
              let pairingProtocol = value["pairing_protocol"] as? String,
              let key = value["key"] as? [String: Any],
              key["algorithm"] as? String == "ed25519",
              let publicKey = key["key"] as? String,
              publicKey.count == 64
        else { return nil }
        return JetPairedClientSummary(
            id: clientID,
            access: access,
            pairedAtUnixMilliseconds: pairedAt,
            pairingProtocol: pairingProtocol,
            publicKey: publicKey
        )
    }

    static func pending(_ value: [String: Any]) -> JetPendingPairing? {
        guard let offerID = UUID(uuidString: value["offer_id"] as? String ?? ""),
              let methodObject = value["method"] as? [String: Any],
              let method = methodObject["method"] as? String,
              let progressObject = value["progress"] as? [String: Any],
              let progressKind = progressObject["progress"] as? String,
              let attempts = (value["attempts_remaining"] as? NSNumber)?.uint32Value,
              let opened = (value["opened_at_unix_ms"] as? NSNumber)?.int64Value,
              let expires = (value["expires_at_unix_ms"] as? NSNumber)?.int64Value
        else { return nil }
        let progress: JetPairingProgress
        switch progressKind {
        case "offered":
            progress = .offered
        case "awaiting_confirmation", "confirmed":
            guard let clientID = UUID(uuidString: progressObject["client_id"] as? String ?? ""),
                  let authenticationString = progressObject["authentication_string"] as? String
            else { return nil }
            progress = progressKind == "awaiting_confirmation"
                ? .awaitingConfirmation(
                    clientID: clientID,
                    authenticationString: authenticationString
                )
                : .confirmed(
                    clientID: clientID,
                    authenticationString: authenticationString
                )
        case "ended":
            guard let reason = progressObject["reason"] as? String else { return nil }
            progress = .ended(reason: reason)
        default:
            return nil
        }
        return JetPendingPairing(
            id: offerID,
            method: method,
            progress: progress,
            attemptsRemaining: attempts,
            openedAtUnixMilliseconds: opened,
            expiresAtUnixMilliseconds: expires
        )
    }

    static func presentationError(_ value: Any?) -> JetPresentationError {
        guard let error = value as? [String: Any],
              let categoryText = error["category"] as? String,
              let code = error["code"] as? String,
              let message = error["message"] as? String,
              let retryable = error["retryable"] as? Bool
        else { return .invalidResponse }
        let category: JetPresentationErrorCategory = switch categoryText {
        case "invalid_input": .invalidInput
        case "unauthorized": .unauthorized
        case "conflict": .conflict
        case "unavailable": .unavailable
        case "incompatible": .incompatible
        case "rate_limited": .rateLimited
        case "not_found": .notFound
        case "outcome_unknown": .outcomeUnknown
        case "internal": .internalFailure
        default: .invalidResponse
        }
        return JetPresentationError(
            category: category,
            code: code,
            message: message,
            retryable: retryable
        )
    }
}

@MainActor
struct JetPlaneRegistryStore {
    private static let key = "jet.remote-planes.v1"
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func load() -> [JetRemotePlaneProfile] {
        guard let data = defaults.data(forKey: Self.key),
              let profiles = try? JSONDecoder().decode([JetRemotePlaneProfile].self, from: data)
        else { return [] }
        // ASVS 2.2.1 and 14.3.3: preferences are untrusted and may contain
        // connection labels only, never pairing keys, proofs, or one-time codes.
        return profiles.compactMap { profile in
            guard let endpoint = try? JetSSHEndpoint.validated(profile.endpoint),
                  let name = try? JetRemotePlaneName.validated(
                      profile.name,
                      fallback: endpoint
                  )
            else { return nil }
            return JetRemotePlaneProfile(
                id: profile.id,
                name: name,
                endpoint: endpoint,
                planeID: profile.planeID
            )
        }
    }

    func save(_ profiles: [JetRemotePlaneProfile]) {
        // Profiles contain only user-visible labels, SSH aliases, and Plane IDs.
        // Pairing keys and proofs remain in Keychain and memory.
        guard let data = try? JSONEncoder().encode(profiles) else { return }
        defaults.set(data, forKey: Self.key)
    }
}

extension Data {
    nonisolated var hexadecimal: String {
        map { String(format: "%02x", $0) }.joined()
    }

    nonisolated init?(hexadecimal: String) {
        guard hexadecimal.count.isMultiple(of: 2) else { return nil }
        var bytes = [UInt8]()
        bytes.reserveCapacity(hexadecimal.count / 2)
        var index = hexadecimal.startIndex
        while index < hexadecimal.endIndex {
            let next = hexadecimal.index(index, offsetBy: 2)
            guard let byte = UInt8(hexadecimal[index..<next], radix: 16) else { return nil }
            bytes.append(byte)
            index = next
        }
        self.init(bytes)
    }
}
