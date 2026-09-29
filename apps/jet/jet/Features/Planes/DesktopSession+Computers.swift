import Foundation

/// Computers for Settings: pairing access, This Mac, and reconnecting.
extension DesktopSession {
    func pairingAccess(for planeRegistryID: UUID) async throws -> any JetPairingAccess {
        try await client(for: planeRegistryID)
    }

    /// This Mac.
    var localPlane: JetPlanePresentation? {
        planes.first { $0.id == localPlaneRegistryID }
    }

    /// Try Again for one computer: This Mac restarts its bounded setup retries;
    /// another computer reloads every computer's snapshot.
    func reconnectComputer(_ planeRegistryID: UUID) async {
        if isLocalPlane(planeRegistryID) {
            await retryConnection()
        } else {
            await refreshPlanes()
        }
    }

    /// A computer's live connection. This Mac's comes from the session itself.
    func computerConnection(_ planeRegistryID: UUID) -> JetConnectionState {
        if isLocalPlane(planeRegistryID) { return connectionState }
        return planes.first { $0.id == planeRegistryID }?.connection ?? .disconnected
    }

    /// The name Jet shows for a computer, such as "This Mac" or "Studio Mac".
    func computerName(_ planeRegistryID: UUID) -> String {
        planes.first { $0.id == planeRegistryID }?.name ?? String(localized: "This Mac")
    }
}
