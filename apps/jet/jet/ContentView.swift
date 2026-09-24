import SwiftUI

struct ContentView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        DesktopShellView(session: session)
    }
}

#Preview {
    ContentView(session: DesktopSession())
        .frame(width: 1280, height: 800)
}

#if os(macOS)
#Preview("Compact") {
    ContentView(session: DesktopSession())
        .frame(width: 900, height: 600)
}

#Preview("Wide dark") {
    ContentView(session: DesktopSession())
        .frame(width: 1600, height: 900)
        .preferredColorScheme(.dark)
}
#endif
