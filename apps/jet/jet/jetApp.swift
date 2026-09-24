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
        let identity = JetKeychainSigningIdentity(clientID: configuration.clientID)
        let remoteService = JetRemotePlaneService(identity: identity)
        let registry = JetPlaneRegistryStore()
        _session = State(
            initialValue: DesktopSession(
                makeJetClient: {
                    let bundled = try await JetLocalCoreInstaller.ensureRunning()
                    for attempt in 0..<40 {
                        do {
                            return try await JetClient.connectLocal(
                                socketURL: socketURL,
                                configuration: configuration
                            )
                        } catch let failure as JetClientFailure {
                            guard bundled, attempt < 39,
                                  case let .presentation(error) = failure,
                                  error.category == .offline
                            else { throw failure }
                            try await Task.sleep(for: .milliseconds(250))
                        }
                    }
                    throw JetClientFailure.presentation(.offline)
                },
                localPlaneRegistryID: configuration.clientID,
                remoteProfiles: registry.load(),
                makeRemoteClient: { endpoint in
                    try await remoteService.connect(endpoint: endpoint)
                },
                claimRemotePairing: { endpoint, secret, commandID in
                    try await remoteService.claim(
                        endpoint: endpoint,
                        secret: secret,
                        commandID: commandID
                    )
                },
                completeRemotePairing: { claim, commandID in
                    try await remoteService.complete(claim: claim, commandID: commandID)
                },
                saveRemoteProfiles: registry.save,
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
        .defaultLaunchBehavior(.presented)
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
