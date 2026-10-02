#if DEBUG
import SwiftUI

extension DesktopPreviewScenes {
    /// The whole window in the design's main states (WP5 owns these).
    @MainActor static var shell: [DesktopPreviewScene] {
        [
            // Only Details, disabled.
            window("new-task") {
                DesktopPreviewData.connect($0)
                $0.open(.newTask)
            },
            // Setup still loading: no banner and no alert.
            window("first-launch") {
                $0.setupState = .loading
                $0.connectionState = .connecting
                $0.open(.newTask)
            },
            // "Working · Editing files", Keep Changes disabled, Interrupt.
            window("task-working") { DesktopPreviewData.working($0) },
            // Keep Changes prominent, no Interrupt.
            window("task-waiting-changes") { DesktopPreviewData.waitingWithChanges($0) },
            // Orange "Needs permission", Interrupt.
            window("task-permission") { DesktopPreviewData.needsPermission($0) },
            // The not-connected banner and the Offline status.
            window("task-offline") { DesktopPreviewData.offline($0) },
            window("task-details", size: CGSize(width: 1440, height: 860)) {
                DesktopPreviewData.inspector($0)
            },
            window("task-compact", size: CGSize(width: 900, height: 600)) {
                DesktopPreviewData.working($0)
            },
            // Narrow with Details: the sidebar hides itself and nothing overflows.
            window("task-compact-details", size: CGSize(width: 900, height: 600)) {
                DesktopPreviewData.working($0)
                ShellSeeds.showDetails($0)
            },
            // Details at the default size: the status drops its phase.
            window("task-details-1280") {
                DesktopPreviewData.working($0)
                ShellSeeds.showDetails($0)
            },
            // The subtitle ends with "· Studio Mac".
            window("task-remote") {
                DesktopPreviewData.working($0)
                ShellSeeds.remote($0)
            },
            // The project name, its folder as the subtitle, and More.
            window("project-page") {
                DesktopPreviewData.connect($0)
                $0.open(.project(DesktopPreviewData.webApp.id))
            },
            // "Jet Trash", Details disabled.
            window("jet-trash") {
                DesktopPreviewData.connect($0)
                $0.open(.trash)
            },
            window("banner-data-protection") {
                DesktopPreviewData.waitingWithChanges($0)
                ShellSeeds.recoveryReadOnly($0)
            },
            window("banner-disk-space") {
                DesktopPreviewData.connect($0)
                $0.open(.newTask)
                ShellSeeds.diskPressure($0)
            },
        ]
    }

    /// A scene that shows the main window for a seeded preview session.
    @MainActor static func window(
        _ id: String,
        size: CGSize = CGSize(width: 1280, height: 800),
        colorScheme: ColorScheme? = nil,
        seed: @escaping @MainActor (DesktopSession) -> Void
    ) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id, size: size, colorScheme: colorScheme) {
            AnyView(ContentView(session: .preview(configure: seed)))
        }
    }
}

/// Extra seeds for the shell's banners and computers.
@MainActor
fileprivate enum ShellSeeds {
    static let studioMacID = UUID(uuidString: "7A3C0000-0000-4000-8000-000000000901") ?? UUID()
    static let studioPlaneID = UUID(uuidString: "7A3C0000-0000-4000-8000-000000000902") ?? UUID()

    static func showDetails(_ session: DesktopSession) {
        session.selectedWorkPanel = .changes
        session.isWorkPanelPresented = true
    }

    /// This Mac's core is in read-only recovery.
    static func recoveryReadOnly(_ session: DesktopSession) {
        let base = DesktopPreviewData.setupSnapshot
        let status = JetPlaneStatus(
            cursor: base.status.cursor,
            planeID: base.status.planeID,
            daemonStarts: base.status.daemonStarts,
            startedAtUnixMilliseconds: base.status.startedAtUnixMilliseconds,
            coreVersion: base.status.coreVersion,
            security: base.status.security,
            recovery: JetRawJSON(source: #"{"state":"read_only","reason":"integrity_check_failed","snapshots":[]}"#)
        )
        let snapshot = JetSetupSnapshot(
            status: status,
            capabilities: base.capabilities,
            projects: base.projects,
            accounts: base.accounts,
            pairing: base.pairing
        )
        session.setupState = .ready(snapshot)
        session.updatePlane(session.localPlaneRegistryID) { $0.snapshot = snapshot }
    }

    /// The last action failed because This Mac is almost out of disk space.
    static func diskPressure(_ session: DesktopSession) {
        session.actionError = JetPresentationError(
            category: .unavailable,
            code: "storage.disk_pressure",
            message: "Not enough free disk space.",
            retryable: true
        )
    }

    /// A connected Studio Mac that owns the open task.
    static func remote(_ session: DesktopSession) {
        let local = session.planes.first { $0.id == session.localPlaneRegistryID }
        session.planes.append(
            JetPlanePresentation(
                id: studioMacID,
                name: "Studio Mac",
                endpoint: "alex@studio.local",
                isLocal: false,
                planeID: studioPlaneID,
                connection: .connected(DesktopPreviewData.negotiation),
                snapshot: local?.snapshot,
                failure: nil,
                conversationCursor: DesktopPreviewData.headCursor
            )
        )
        if let conversationID = session.selectedConversationID {
            session.conversationPlaneRegistryIDs[conversationID] = studioMacID
        }
    }
}
#endif
