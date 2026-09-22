import Foundation

enum JetClientIdentity {
    private static let defaultsKey = "jet.client-id"

    static func load() -> UUID {
        if let stored = UserDefaults.standard.string(forKey: defaultsKey),
           let identity = UUID(uuidString: stored)
        {
            return identity
        }
        let identity = UUID()
        // ASVS 14.3.3: the installation identifier is non-secret client
        // state. Credentials and pairing keys never use preferences.
        UserDefaults.standard.set(identity.uuidString.lowercased(), forKey: defaultsKey)
        return identity
    }
}
