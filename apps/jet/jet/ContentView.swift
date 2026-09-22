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
