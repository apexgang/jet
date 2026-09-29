import SwiftUI

struct ContentView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        DesktopShellView(session: session)
    }
}

#if DEBUG
#Preview("Working") {
    ContentView(session: .preview { DesktopPreviewData.working($0) })
        .frame(width: 1280, height: 800)
}
#endif

#Preview("Fixture") {
    ContentView(session: DesktopSession())
        .frame(width: 1280, height: 800)
}
