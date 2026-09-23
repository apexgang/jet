import SwiftUI

@main
struct jetApp: App {
    @State private var session: DesktopSession

    init() {
#if os(macOS)
        let socketURL = JetClient.defaultLocalSocketURL(
            homeDirectory: FileManager.default.homeDirectoryForCurrentUser
        )
        let configuration = JetClientConfiguration(clientID: JetClientIdentity.load())
        _session = State(
            initialValue: DesktopSession(
                makeJetClient: {
                    try await JetClient.connectLocal(
                        socketURL: socketURL,
                        configuration: configuration
                    )
                },
                notifications: SystemJetNotificationCenter()
            )
        )
#else
        _session = State(initialValue: DesktopSession())
#endif
    }

    var body: some Scene {
#if os(macOS)
        Window("Jet", id: "main") {
            ContentView(session: session)
        }
        .defaultSize(width: 1280, height: 800)
        .commands {
            JetCommands(session: session)
        }

        Settings {
            JetSettingsView(session: session)
        }
#else
        WindowGroup {
            ContentView(session: session)
        }
#endif
    }
}
