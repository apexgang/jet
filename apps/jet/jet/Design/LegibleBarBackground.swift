import SwiftUI

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
