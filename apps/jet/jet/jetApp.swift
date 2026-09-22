import SwiftUI

@main
struct jetApp: App {
    @State private var session = DesktopSession()

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
            JetSettingsView()
        }
#else
        WindowGroup {
            ContentView(session: session)
        }
#endif
    }
}
