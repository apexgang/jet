import SwiftUI

struct ProjectPreviewView: View {
    let session: DesktopSession
    let preview: JetProjectPreview

    var body: some View {
        HStack(alignment: .center, spacing: 18) {
            VStack(alignment: .leading, spacing: 3) {
                Text(preview.canRegister ? "Ready to add" : "Choose another folder")
                    .font(.subheadline.weight(.semibold))
                Text(preview.root)
                    .font(.caption)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 10)
            if preview.canRegister {
                Button("Add Project") {
                    Task { await session.registerPreviewedProject() }
                }
                .buttonStyle(.borderedProminent)
                .disabled(session.setupOperation != nil)
            }
        }
        .padding(12)
        .background(Color.accentColor.opacity(0.08), in: RoundedRectangle(cornerRadius: JetDesign.controlRadius))
    }

    private var detail: String {
        switch preview.registrability {
        case let .registrable(detail): detail
        case let .unavailable(_, detail): detail
        }
    }
}
