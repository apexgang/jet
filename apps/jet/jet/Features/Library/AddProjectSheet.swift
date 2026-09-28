import SwiftUI

/// Compile-only stub. WP7 replaces it with the real sheet.
struct AddProjectSheet: View {
    @Bindable var session: DesktopSession
    let planeRegistryID: UUID
    let droppedURL: URL?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Add Project")
                .font(.title3.weight(.semibold))
            HStack {
                Spacer()
                Button("Cancel", action: session.dismissSheet)
                    .keyboardShortcut(.cancelAction)
            }
        }
        .padding(24)
        .frame(width: 420)
    }
}
