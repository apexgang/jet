import SwiftUI

/// Compile-only stub. WP10 replaces it with the real sheet.
struct MoveToTrashSheet: View {
    @Bindable var session: DesktopSession
    let ref: ConversationRef
    let deleteEverywhere: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Move to Jet Trash")
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
