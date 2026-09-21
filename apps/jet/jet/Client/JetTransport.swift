import Foundation
@preconcurrency import Network

nonisolated protocol JetByteTransport: Sendable {
    func connect() async throws
    func readExactly(_ count: Int) async throws -> Data
    func write(_ data: Data) async throws
    func close() async
}

enum JetTransportFailure: Error, Equatable, Sendable {
    case unavailable
    case closed
    case invalidReadLength
}

#if os(macOS)
/// The only callback-based Network.framework adapter. Protocol and feature code
/// see async byte-stream operations instead of connection callbacks.
actor JetUnixSocketTransport: JetByteTransport {
    private let path: String
    private let queue = DispatchQueue(label: "me.heeka.jet.transport")
    private var connection: NWConnection?

    init(socketURL: URL) {
        path = socketURL.path
    }

    func connect() async throws {
        guard connection == nil else { return }

        // ASVS 6.1.1 and 6.3.1: authenticate only through the owner-local
        // endpoint selected by the caller; the Jet handshake establishes the
        // client identity before Plane data is accepted.
        let candidate = NWConnection(
            to: .unix(path: path),
            using: .tcp
        )

        do {
            try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { continuation in
                    var completed = false
                    candidate.stateUpdateHandler = { state in
                        guard !completed else { return }
                        switch state {
                        case .ready:
                            completed = true
                            continuation.resume()
                        case .failed, .cancelled:
                            completed = true
                            continuation.resume(
                                throwing: JetTransportFailure.unavailable
                            )
                        default:
                            break
                        }
                    }
                    candidate.start(queue: queue)
                }
            } onCancel: {
                candidate.cancel()
            }
            try Task.checkCancellation()
            candidate.stateUpdateHandler = nil
            connection = candidate
        } catch {
            candidate.cancel()
            throw error
        }
    }

    func readExactly(_ count: Int) async throws -> Data {
        guard count >= 0 else { throw JetTransportFailure.invalidReadLength }
        guard count > 0 else { return Data() }

        var result = Data()
        result.reserveCapacity(count)
        while result.count < count {
            let remaining = count - result.count
            let chunk = try await receive(maximumLength: remaining)
            guard !chunk.isEmpty else { throw JetTransportFailure.closed }
            result.append(chunk)
        }
        return result
    }

    func write(_ data: Data) async throws {
        guard let connection else { throw JetTransportFailure.closed }
        try await withCheckedThrowingContinuation { continuation in
            connection.send(
                content: data,
                contentContext: .defaultMessage,
                isComplete: true,
                completion: .contentProcessed { error in
                    if error == nil {
                        continuation.resume()
                    } else {
                        continuation.resume(throwing: JetTransportFailure.closed)
                    }
                }
            )
        }
    }

    func close() {
        connection?.stateUpdateHandler = nil
        connection?.cancel()
        connection = nil
    }

    private func receive(maximumLength: Int) async throws -> Data {
        guard let connection else { throw JetTransportFailure.closed }
        return try await withCheckedThrowingContinuation { continuation in
            connection.receive(
                minimumIncompleteLength: 1,
                maximumLength: maximumLength
            ) { data, _, isComplete, error in
                if let error {
                    _ = error
                    continuation.resume(throwing: JetTransportFailure.closed)
                } else if let data, !data.isEmpty {
                    continuation.resume(returning: data)
                } else if isComplete {
                    continuation.resume(throwing: JetTransportFailure.closed)
                } else {
                    continuation.resume(returning: Data())
                }
            }
        }
    }
}
#endif
