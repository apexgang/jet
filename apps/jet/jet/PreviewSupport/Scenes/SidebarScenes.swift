#if DEBUG
import SwiftUI

extension DesktopPreviewScenes {
    /// Sidebar states (WP3): the library, empty and failed lists, a second
    /// computer that is offline, and Search Tasks. Loading is ShellScenes'
    /// "first-launch".
    @MainActor static var sidebar: [DesktopPreviewScene] {
        [
            sidebarWindow("sidebar-library") { session in
                DesktopPreviewData.connect(session)
                session.beginNewTask()
                session.draft = "Add a dark mode toggle to Settings"
            },
            sidebarWindow("sidebar-empty") { session in
                DesktopPreviewData.connect(session)
                session.planeConversations[session.localPlaneRegistryID] = []
                session.rebuildConversationAggregation()
                session.conversationFreshness = .live
                session.statusStore.reset(conversationIDs: DesktopPreviewData.conversations.map(\.id))
                session.beginNewTask()
            },
            sidebarWindow("sidebar-load-failed") { session in
                session.connectionState = .failed(.offline)
                session.conversationFreshness = .failed
                session.updatePlane(session.localPlaneRegistryID) { plane in
                    plane.connection = .failed(.offline)
                    plane.failure = .offline
                }
                session.beginNewTask()
            },
            sidebarWindow("sidebar-two-computers") { session in
                DesktopPreviewData.connect(session)
                SidebarPreviewSeed.addStudioMac(session)
                session.beginNewTask()
            },
            sidebarWindow("sidebar-search") { session in
                SidebarPreviewSeed.search(session)
                session.searchResult = SidebarPreviewSeed.loginResult(session)
            },
            sidebarWindow("sidebar-search-pending") { session in
                SidebarPreviewSeed.search(session)
                session.searchResult = nil
            },
            sidebarWindow("sidebar-search-empty") { session in
                DesktopPreviewData.connect(session)
                session.beginNewTask()
                session.searchText = "zebra"
                let local = session.localPlaneRegistryID
                session.searchResult = JetFederatedSearchResult(
                    hits: [],
                    cursors: [local: 120],
                    indexedThrough: [local: 120],
                    failures: [:]
                )
            },
            // The narrowest sidebar the window allows, with its longest rows.
            DesktopPreviewScene(id: "sidebar-narrow", size: CGSize(width: 900, height: 600)) {
                AnyView(
                    NavigationSplitView {
                        SidebarView(session: .preview { session in
                            DesktopPreviewData.connect(session)
                            SidebarPreviewSeed.addStudioMac(session)
                            session.beginNewTask()
                        })
#if os(macOS)
                        .navigationSplitViewColumnWidth(220)
#endif
                    } detail: {
                        Color.clear
                    }
                )
            },
        ]
    }

    @MainActor private static func sidebarWindow(
        _ id: String,
        seed: @escaping @MainActor (DesktopSession) -> Void
    ) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id) {
            AnyView(ContentView(session: .preview(configure: seed)))
        }
    }
}

/// A second computer and search results for the sidebar scenes. Times are based
/// on `DesktopPreviewData.now` so relative times don't drift.
@MainActor
private enum SidebarPreviewSeed {
    static let studioID = uuid("7A3C0000-0000-4000-8000-000000000901")
    static let studioProjectID = uuid("7A3C0000-0000-4000-8000-000000000911")
    static let sessionHelpersID = uuid("7A3C0000-0000-4000-8000-000000000921")
    static let unloadedBranchTaskID = uuid("7A3C0000-0000-4000-8000-000000000922")

    /// "Studio Mac", paired but offline, with two tasks.
    static func addStudioMac(_ session: DesktopSession) {
        session.planes.append(JetPlanePresentation(
            id: studioID,
            name: "Studio Mac",
            endpoint: "alex@studio.local",
            isLocal: false,
            planeID: nil,
            connection: .failed(.offline),
            snapshot: nil,
            failure: .offline,
            conversationCursor: nil
        ))
        add(
            [
                task("7A3C0000-0000-4000-8000-000000000931", "Profile slow checkout query", age: 3 * DesktopPreviewData.hour),
                task("7A3C0000-0000-4000-8000-000000000932", "Update deploy script", age: 24 * DesktopPreviewData.hour),
            ],
            on: studioID,
            in: session
        )
    }

    /// Two computers, a local task found by file, and "login" typed into Search Tasks.
    static func search(_ session: DesktopSession) {
        DesktopPreviewData.connect(session)
        addStudioMac(session)
        let helpers = JetConversationSummary(
            id: sessionHelpersID,
            revision: 2,
            title: "Clean up session helpers",
            createdAtUnixMilliseconds: DesktopPreviewData.now - 3 * 24 * DesktopPreviewData.hour,
            projectID: DesktopPreviewData.webApp.id
        )
        add([helpers], on: session.localPlaneRegistryID, in: session)
        session.statusStore.seedForPreview(
            TaskStatusFacts(lifecycle: .completed, hasRuns: true, lastSequence: 90),
            for: helpers.id
        )
        session.beginNewTask()
        session.searchText = "login"
    }

    /// This Mac found "login" in a task's name, in a changed file and in a branch
    /// of a task that isn't loaded yet; Studio Mac couldn't be searched.
    static func loginResult(_ session: DesktopSession) -> JetFederatedSearchResult {
        let local = session.localPlaneRegistryID
        session.conversationPlaneRegistryIDs[unloadedBranchTaskID] = local
        func hit(_ id: UUID, _ sequence: UInt64, _ field: JetSearchField, _ excerpt: String) -> JetFederatedSearchHit {
            JetFederatedSearchHit(
                planeRegistryID: local,
                planeName: "This Mac",
                hit: JetSearchHit(conversationID: id, sequence: sequence, field: field, excerpt: excerpt)
            )
        }
        return JetFederatedSearchResult(
            hits: [
                // Already listed under Tasks by its title, so it isn't repeated.
                hit(DesktopPreviewData.loginRedirect.id, 110, .name, "Fix login redirect loop"),
                hit(sessionHelpersID, 96, .path, "src/auth/login.ts"),
                hit(unloadedBranchTaskID, 40, .branch, "jet/login-redirect"),
            ],
            cursors: [local: 120],
            indexedThrough: [local: 120],
            failures: [studioID: .offline]
        )
    }

    private static func add(
        _ conversations: [JetConversationSummary],
        on planeRegistryID: UUID,
        in session: DesktopSession
    ) {
        session.planeConversations[planeRegistryID, default: []].append(contentsOf: conversations)
        for conversation in conversations {
            session.conversationPlaneRegistryIDs[conversation.id] = planeRegistryID
        }
        session.rebuildConversationAggregation()
    }

    private static func task(_ id: String, _ title: String, age: Int64) -> JetConversationSummary {
        JetConversationSummary(
            id: uuid(id),
            revision: 1,
            title: title,
            createdAtUnixMilliseconds: DesktopPreviewData.now - age,
            projectID: studioProjectID
        )
    }

    private static func uuid(_ value: String) -> UUID {
        guard let id = UUID(uuidString: value) else {
            preconditionFailure("Invalid preview UUID \(value)")
        }
        return id
    }
}
#endif
