#if DEBUG
import Foundation

extension DesktopSession {
    /// A live-mode session for previews and screenshots. It renders the live
    /// (non-fixture) paths, never reaches a Plane, and keeps client memory in a
    /// throwaway defaults suite. `configure` runs synchronously before it returns.
    static func preview(configure: @MainActor (DesktopSession) -> Void) -> DesktopSession {
        let suiteName = "jet.preview"
        let defaults = UserDefaults(suiteName: suiteName) ?? .standard
        defaults.removePersistentDomain(forName: suiteName)
        let session = DesktopSession(
            makeJetClient: { throw CancellationError() },
            notificationPreference: { _ in false },
            memory: ClientMemory(defaults: defaults),
            isPreviewSession: true
        )
        configure(session)
        return session
    }
}
#endif
