import SwiftUI

/// A bar background that stays opaque when Reduce Transparency is on.
///
/// Fixture (iOS) views only; the live path uses system bars.
struct LegibleBarBackground: ViewModifier {
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    func body(content: Content) -> some View {
        content.background {
            if reduceTransparency {
#if os(macOS)
                Color(nsColor: .windowBackgroundColor)
#else
                Color(uiColor: .systemBackground)
#endif
            } else {
                Rectangle().fill(.bar)
            }
        }
    }
}
