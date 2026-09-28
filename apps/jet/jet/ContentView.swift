import SwiftUI

struct ContentView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        DesktopShellView(session: session)
    }
}

#if DEBUG
#Preview("New Task") {
    DesktopPreviewScenes.view("new-task")
        .frame(width: 1280, height: 800)
}

#if os(macOS)
#Preview("Compact") {
    DesktopPreviewScenes.view("task-compact")
        .frame(width: 900, height: 600)
}

#Preview("Wide dark") {
    DesktopPreviewScenes.view("task-waiting-changes")
        .frame(width: 1600, height: 900)
        .preferredColorScheme(.dark)
}
#endif
#endif

#Preview("Fixture") {
    ContentView(session: DesktopSession())
        .frame(width: 1280, height: 800)
}
