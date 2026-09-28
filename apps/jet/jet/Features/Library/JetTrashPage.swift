import SwiftUI

/// Compile-only stub. WP10 replaces it with the Jet Trash table.
struct JetTrashPage: View {
    @Bindable var session: DesktopSession

    var body: some View {
        ContentUnavailableView("Jet Trash", systemImage: "trash")
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .navigationTitle("Jet Trash")
    }
}
