import SwiftUI

/// Move Project Folder to Trash… (design §6.11): the reviewed removal with a
/// typed-name confirmation. Cancel is the default button.
struct ProjectRemovalSheet: View {
    let session: DesktopSession
    let preview: JetProjectRemovalPreview

    @State private var typedName = ""
    @State private var permanently = false

    /// JetClient's labels for these two obstacles; they read as casual sentences instead.
    static let liveRunsLabel = "Stop active Runs first"
    static let schedulesLabel = "Disable scheduled tasks first"

    var body: some View {
        VStack(alignment: .leading, spacing: JetDesign.gap) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Move the “\(preview.name)” folder to the Trash?")
                    .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
                    .fixedSize(horizontal: false, vertical: true)
                Text("Jet moves the whole “\(preview.name)” folder to the macOS Trash, including changes that aren't committed, and removes the project from Jet. Its tasks stay, but their working copies are deleted. Jet can't remove a project without its folder yet.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Grid(alignment: .leading, horizontalSpacing: JetDesign.gap, verticalSpacing: 6) {
                fact("Folder", preview.root)
                fact("Size", JetCopy.byteCount(Int64(clamping: preview.diskUseBytes)))
                fact("Changed files", JetCopy.number(preview.dirtyFiles))
                fact("Unpushed commits", JetCopy.number(preview.unpushedCommits))
                fact("Working copies", JetCopy.number(preview.workspaceCount))
            }

            let obstacles = Self.obstacleSentences(preview)
            if obstacles.isEmpty {
                TextField("Type “\(preview.name)” to confirm", text: $typedName)
                    .textFieldStyle(.roundedBorder)

                if session.permanentRemovalAllowed {
                    Toggle(isOn: $permanently) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Delete Permanently")
                            Text("The macOS Trash isn't available for this folder.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text(preview.permanentWarning)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(obstacles, id: \.self) { obstacle in
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .symbolRenderingMode(.monochrome)
                                .foregroundStyle(.orange)
                                .accessibilityHidden(true)
                            Text(obstacle)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            }

            if let notice = session.setupNotice {
                LibraryNoticeRow(
                    issue: LibraryIssue(kind: .error, text: notice),
                    perform: { _ in }
                )
            }

            HStack(spacing: JetDesign.smallGap) {
                Spacer()
                Button("Cancel") {
                    session.cancelProjectRemoval()
                }
                .keyboardShortcut(.defaultAction)
                Button(permanently ? "Delete Folder Permanently" : "Move Folder to Trash", role: .destructive) {
                    let name = typedName
                    let permanently = permanently
                    Task { await session.removeProjectFolder(typedName: name, permanently: permanently) }
                }
                .disabled(
                    typedName != preview.name
                        || !obstacles.isEmpty
                        || session.setupOperation != nil
                )
                .accessibilityIdentifier("project-removal-confirm")
            }
        }
        .padding(JetDesign.sectionGap)
        .frame(width: 520, alignment: .leading)
        .interactiveDismissDisabled(session.setupOperation != nil)
    }

    /// Casual sentences for what blocks removal; other labels show as the daemon wrote them.
    static func obstacleSentences(_ preview: JetProjectRemovalPreview) -> [String] {
        var sentences: [String] = []
        if preview.liveRuns > 0 || preview.obstacles.contains(liveRunsLabel) {
            sentences.append(String(localized: "An assistant is still working on one of its tasks. Stop it first."))
        }
        if preview.schedules > 0 || preview.obstacles.contains(schedulesLabel) {
            sentences.append(String(localized: "Some of its tasks repeat a message daily. Stop repeating first."))
        }
        sentences += preview.obstacles.filter { $0 != liveRunsLabel && $0 != schedulesLabel }
        return sentences
    }

    private func fact(_ label: LocalizedStringKey, _ value: String) -> some View {
        GridRow {
            Text(label)
                .foregroundStyle(.secondary)
            Text(value)
                .lineLimit(1)
                .truncationMode(.middle)
                .help(value)
        }
    }
}

#if DEBUG
#Preview("Project removal") {
    DesktopPreviewScenes.view("library-project-removal")
        .frame(width: 520, height: 480)
}
#endif
