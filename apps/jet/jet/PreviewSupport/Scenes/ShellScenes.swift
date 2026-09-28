#if DEBUG
import SwiftUI

extension DesktopPreviewScenes {
    /// The whole window in the design's main states (WP5 owns these).
    @MainActor static var shell: [DesktopPreviewScene] {
        [
            window("new-task") {
                DesktopPreviewData.connect($0)
                $0.open(.newTask)
            },
            window("first-launch") {
                $0.setupState = .loading
                $0.connectionState = .connecting
                $0.open(.newTask)
            },
            window("task-working") { DesktopPreviewData.working($0) },
            window("task-waiting-changes") { DesktopPreviewData.waitingWithChanges($0) },
            window("task-permission") { DesktopPreviewData.needsPermission($0) },
            window("task-offline") { DesktopPreviewData.offline($0) },
            window("task-details", size: CGSize(width: 1440, height: 860)) {
                DesktopPreviewData.inspector($0)
            },
            window("task-compact", size: CGSize(width: 900, height: 600)) {
                DesktopPreviewData.working($0)
            },
            window("project-page") {
                DesktopPreviewData.connect($0)
                $0.open(.project(DesktopPreviewData.webApp.id))
            },
            window("jet-trash") {
                DesktopPreviewData.connect($0)
                $0.open(.trash)
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
#endif
