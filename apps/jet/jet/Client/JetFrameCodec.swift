import Foundation

nonisolated enum JetFrameKind: UInt8, Sendable {
    case control = 0
    case data = 1
}

nonisolated struct JetFrame: Sendable, Equatable {
    let kind: JetFrameKind
    let streamID: UInt32
    let payload: Data
}

nonisolated struct JetFrameLimits: Sendable, Equatable {
    static let protocolMaximum = JetFrameLimits(
        control: 1_048_576,
        data: 262_144
    )

    let control: Int
    let data: Int

    func negotiated(with peer: JetFrameLimits) -> JetFrameLimits {
        JetFrameLimits(
            control: min(control, peer.control),
            data: min(data, peer.data)
        )
    }

    func limit(for kind: JetFrameKind) -> Int {
        switch kind {
        case .control: control
        case .data: data
        }
    }
}

nonisolated enum JetFrameFailure: Error, Equatable, Sendable {
    case unknownKind(UInt8)
    case invalidStream
    case oversized(declared: Int, limit: Int)
    case multiplexingRequired
}

nonisolated enum JetFrameCodec {
    static func read(
        from transport: any JetByteTransport,
        multiplexed: Bool,
        limits: JetFrameLimits = .protocolMaximum
    ) async throws -> JetFrame {
        let header = try await transport.readExactly(multiplexed ? 9 : 5)
        guard let kindByte = header.first,
              let kind = JetFrameKind(rawValue: kindByte)
        else {
            throw JetFrameFailure.unknownKind(header.first ?? .max)
        }

        let streamID = multiplexed ? uint32(header, at: 1) : 0
        let declared = Int(uint32(header, at: multiplexed ? 5 : 1))
        let limit = limits.limit(for: kind)

        // ASVS 1.5.2, 2.2.1, and 15.3.5: reject an invalid envelope before
        // allocating attacker-controlled payload storage.
        guard declared <= limit else {
            throw JetFrameFailure.oversized(declared: declared, limit: limit)
        }
        guard !(multiplexed && kind == .data && streamID == 0) else {
            throw JetFrameFailure.invalidStream
        }

        return JetFrame(
            kind: kind,
            streamID: streamID,
            payload: try await transport.readExactly(declared)
        )
    }

    static func encode(
        _ frame: JetFrame,
        multiplexed: Bool,
        limits: JetFrameLimits
    ) throws -> Data {
        guard multiplexed || frame.streamID == 0 else {
            throw JetFrameFailure.multiplexingRequired
        }
        guard !(multiplexed && frame.kind == .data && frame.streamID == 0) else {
            throw JetFrameFailure.invalidStream
        }

        let limit = limits.limit(for: frame.kind)
        guard frame.payload.count <= limit else {
            throw JetFrameFailure.oversized(
                declared: frame.payload.count,
                limit: limit
            )
        }

        var result = Data([frame.kind.rawValue])
        if multiplexed {
            append(frame.streamID, to: &result)
        }
        append(UInt32(frame.payload.count), to: &result)
        result.append(frame.payload)
        return result
    }

    private static func append(_ value: UInt32, to data: inout Data) {
        data.append(UInt8((value >> 24) & 0xff))
        data.append(UInt8((value >> 16) & 0xff))
        data.append(UInt8((value >> 8) & 0xff))
        data.append(UInt8(value & 0xff))
    }

    private static func uint32(_ data: Data, at offset: Int) -> UInt32 {
        let bytes = Array(data[offset..<(offset + 4)])
        return (UInt32(bytes[0]) << 24)
            | (UInt32(bytes[1]) << 16)
            | (UInt32(bytes[2]) << 8)
            | UInt32(bytes[3])
    }
}
