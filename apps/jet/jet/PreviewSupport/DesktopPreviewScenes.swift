#if DEBUG
import SwiftUI

/// One representative state of the Mac app for previews and harness screenshots.
/// Scenes without a fixed colour scheme are captured in light and dark.
struct DesktopPreviewScene: Identifiable {
    let id: String
    var size: CGSize = CGSize(width: 1280, height: 800)
    var colorScheme: ColorScheme? = nil
    let makeView: @MainActor () -> AnyView
}

/// Every preview scene, grouped by the package that owns it. Each group lives in
/// its own file under PreviewSupport/Scenes.
enum DesktopPreviewScenes {
    @MainActor static var all: [DesktopPreviewScene] {
        shell + sidebar + status + transcript + history + composer + newTask + details + keepChanges + library + settings
    }

    /// The view of the scene with this id, for `#Preview` blocks.
    @MainActor static func view(_ id: String) -> AnyView {
        all.first { $0.id == id }?.makeView()
            ?? AnyView(Text(verbatim: "No preview scene named \(id)"))
    }
}
#endif
