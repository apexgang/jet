import SwiftUI

/// Compile-only stub. WP9 replaces it with the real sheet.
struct KeepChangesSheet: View {
    @Bindable var session: DesktopSession
    let ref: ConversationRef
    let mode: KeepChangesMode

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Keep Changes")
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
