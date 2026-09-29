import SwiftUI

/// A reviewed folder: its title, what adding it means and its path (design §6.7).
struct ProjectPreviewView: View {
    let preview: JetProjectPreview
    let computerName: String
    let isLocal: Bool
    /// Set only by the legacy initializer, which also offers Add Project.
    private let session: DesktopSession?

    init(preview: JetProjectPreview, computerName: String, isLocal: Bool) {
        self.preview = preview
        self.computerName = computerName
        self.isLocal = isLocal
        session = nil
    }

    /// The block plus an Add Project button that registers through the session's
    /// setup flow. Kept for the setup view.
    init(session: DesktopSession, preview: JetProjectPreview) {
        self.preview = preview
        computerName = session.selectedPlaneName
        isLocal = session.isLocalPlane(session.selectedPlaneRegistryID)
        self.session = session
    }

    var body: some View {
        let copy = AddProjectCopy.review(preview, computerName: computerName, isLocal: isLocal)
        VStack(alignment: .leading, spacing: 8) {
            Text(copy.title)
                .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
            Text(copy.message)
                .font(.system(size: JetDesign.TextSize.navigation))
                .fixedSize(horizontal: false, vertical: true)
            if copy.showsPath {
                pathRow
                    .padding(.top, 4)
            }
            if let session, preview.canRegister {
                Button("Add Project") {
                    Task { await session.registerPreviewedProject() }
                }
                .buttonStyle(.borderedProminent)
                .disabled(session.setupOperation != nil)
                .padding(.top, 4)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var pathRow: some View {
        Label {
            Text(verbatim: displayPath)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
        } icon: {
            Image(systemName: "folder")
        }
        .font(.system(size: JetDesign.TextSize.control))
        .foregroundStyle(.secondary)
        .help(preview.root)
    }

    /// This Mac's paths read from the home folder ("~/code/web-app").
    private var displayPath: String {
        guard isLocal else { return preview.root }
        return (preview.root as NSString).abbreviatingWithTildeInPath
    }
}
