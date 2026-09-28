#if DEBUG
import SwiftUI

extension DesktopPreviewScenes {
    /// Composer states (WP7).
    @MainActor static var composer: [DesktopPreviewScene] { [] }

    /// New Task and first-run states (WP7).
    @MainActor static var newTask: [DesktopPreviewScene] { [] }
}
#endif
