import SwiftUI

struct ProjectRemovalSheet: View {
    let session: DesktopSession
    let preview: JetProjectRemovalPreview

    @Environment(\.dismiss) private var dismiss
    @State private var typedName = ""
    @State private var permanently = false

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 5) {
                Text("Remove \(preview.name)?")
                    .font(.title2.weight(.semibold))
                Text("This removes Jet's registration and the Project folder.")
                    .foregroundStyle(.secondary)
            }

            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 8) {
                fact("Folder", preview.root)
                fact("On disk", ByteCountFormatter.string(fromByteCount: Int64(preview.diskUseBytes), countStyle: .file))
                fact("Changed files", preview.dirtyFiles.formatted())
                fact("Unpushed commits", preview.unpushedCommits.formatted())
                fact("Workspaces", preview.workspaceCount.formatted())
            }

            if preview.obstacles.isEmpty {
                TextField("Type \(preview.name) to confirm", text: $typedName)
                    .textFieldStyle(.roundedBorder)

                if session.permanentRemovalAllowed {
                    Toggle(isOn: $permanently) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Delete permanently")
                            Text("Trash is unavailable. \(preview.permanentWarning)")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            } else {
                VStack(alignment: .leading, spacing: 7) {
                    Text("This Project cannot be removed yet.")
                        .font(.subheadline.weight(.semibold))
                    ForEach(preview.obstacles, id: \.self) { obstacle in
                        Label(obstacle, systemImage: "exclamationmark.circle")
                            .foregroundStyle(.orange)
                    }
                }
                .padding(12)
                .background(Color.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: JetDesign.controlRadius))
            }

            HStack {
                Spacer()
                Button("Cancel") {
                    session.cancelProjectRemoval()
                    dismiss()
                }
                Button(permanently ? "Delete permanently" : "Move to Trash", role: .destructive) {
                    Task {
                        await session.confirmProjectRemoval(
                            typedName: typedName,
                            permanently: permanently
                        )
                        if session.removalPreview == nil { dismiss() }
                    }
                }
                .disabled(
                    typedName != preview.name
                        || !preview.obstacles.isEmpty
                        || session.setupOperation != nil
                )
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(width: 520)
        .interactiveDismissDisabled(session.setupOperation != nil)
    }

    private func fact(_ label: String, _ value: String) -> some View {
        GridRow {
            Text(label)
                .foregroundStyle(.secondary)
            Text(value)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .font(.subheadline)
    }
}
