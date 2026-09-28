import SwiftUI

/// Compile-only stub. WP10 replaces it with the project page; until then it shows
/// the existing project setup.
struct ProjectPageView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        ProjectSetupView(session: session)
    }
}
