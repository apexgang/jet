import SwiftUI

// The sidebar's rows (design §6.3, §8). Views only: they render what the session
// derived and call back through closures.

/// A task: its status glyph, a one-line title and "web-app · 2 hr ago".
struct SidebarTaskRow: View {
    let presentation: TaskRowPresentation

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 1) {
                Text(presentation.title)
                    .lineLimit(1)
                    .sidebarDimmed(presentation.isDimmed)
                SidebarSecondaryLine(
                    project: presentation.secondaryProject,
                    detail: presentation.secondaryDetail,
                    computer: presentation.secondaryComputer
                )
            }
        } icon: {
            TaskStatusGlyph(status: presentation.status, isUnread: presentation.isUnread)
        }
        .help(presentation.help)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(presentation.accessibilityLabel)
#if os(macOS)
        .typeSelectEquivalent(Text(presentation.title))
#endif
    }
}

/// "web-app · 2 hr ago · Studio Mac". When the row is too narrow, the computer
/// name goes first and then the project shortens, so a status such as "Needs
/// permission" stays readable. VoiceOver reads the whole line from the row label.
private struct SidebarSecondaryLine: View {
    let project: String?
    let detail: String
    let computer: String?

    var body: some View {
        ViewThatFits(in: .horizontal) {
            line(computer: computer)
            line(computer: nil)
        }
        .lineLimit(1)
        .font(.subheadline)
        .foregroundStyle(.secondary)
    }

    private func line(computer: String?) -> some View {
        HStack(spacing: 0) {
            if let project {
                Text(project)
                separator
            }
            Text(detail)
                .layoutPriority(2)
            if let computer {
                separator
                Text(computer)
                    .layoutPriority(1)
            }
        }
    }

    private var separator: some View {
        Text(verbatim: " · ")
            .layoutPriority(3)
    }
}

/// New Task, with "Draft" under it while its draft has text.
struct SidebarNewTaskRow: View {
    let hasDraft: Bool

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 1) {
                Text("New Task")
                    .lineLimit(1)
                if hasDraft {
                    Text("Draft")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
            }
        } icon: {
            Image(systemName: "square.and.pencil")
        }
        .accessibilityElement(children: .combine)
    }
}

/// A project folder, with its computer and "Offline" when they apply.
struct SidebarProjectRow: View {
    let name: String
    let detail: String?
    let root: String
    let isDimmed: Bool

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 1) {
                Text(name)
                    .lineLimit(1)
                    .sidebarDimmed(isDimmed)
                if let detail {
                    Text(detail)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
        } icon: {
            Image(systemName: "folder")
        }
        .help(root)
        .accessibilityElement(children: .combine)
#if os(macOS)
        .typeSelectEquivalent(Text(name))
#endif
    }
}

/// A task a computer found by name, file or branch.
struct SidebarOtherMatchRow: View {
    let match: SidebarOtherMatch
    let status: TaskStatus
    let isUnread: Bool

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 1) {
                Text(match.displayTitle)
                    .lineLimit(1)
                field
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        } icon: {
            TaskStatusGlyph(status: status, isUnread: isUnread)
        }
        .help(match.displayTitle)
        .accessibilityElement(children: .combine)
#if os(macOS)
        .typeSelectEquivalent(Text(match.displayTitle))
#endif
    }

    @ViewBuilder
    private var field: some View {
        switch match.field {
        case .name:
            Text("Name: \(match.excerpt)")
                .truncationMode(.tail)
        case .path:
            Text("File: \(match.excerpt)")
                .truncationMode(.middle)
        case .branch:
            Text("Branch: \(match.excerpt)")
                .truncationMode(.middle)
        }
    }
}

/// A problem with a Try Again button under its text, such as "Studio Mac is
/// offline". The text and the button stay separate for VoiceOver.
struct SidebarNoticeRow: View {
    let systemImage: String
    let tint: Color
    let text: String
    var isWorking = false
    let action: @MainActor () -> Void

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 6) {
                Text(text)
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 6) {
                    Button("Try Again", action: action)
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .disabled(isWorking)
                    if isWorking {
                        ProgressView()
                            .controlSize(.small)
                    }
                }
            }
            .padding(.vertical, 2)
        } icon: {
            Image(systemName: systemImage)
                .foregroundStyle(tint)
        }
        .accessibilityElement(children: .contain)
    }
}

/// A stand-in task row while the list loads.
struct SidebarPlaceholderRow: View {
    let index: Int

    private static let titles = [
        "Fix the login redirect loop",
        "Update payment dependencies",
        "Add dark mode to settings",
        "Speed up the dashboard",
    ]

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 1) {
                Text(verbatim: Self.titles[index % Self.titles.count])
                    .lineLimit(1)
                Text(verbatim: "web-app · 2 hr ago")
                    .font(.subheadline)
                    .lineLimit(1)
            }
        } icon: {
            Color.clear
                .frame(width: 16, height: 16)
        }
        .redacted(reason: .placeholder)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text("Loading tasks"))
        .accessibilityHidden(index != 0)
    }
}

extension View {
    /// Rows on an offline computer use the secondary style; other rows keep the
    /// sidebar's own text style, which the system adapts to selection and to an
    /// inactive window.
    @ViewBuilder
    func sidebarDimmed(_ isDimmed: Bool) -> some View {
        if isDimmed {
            foregroundStyle(.secondary)
        } else {
            self
        }
    }
}

/// Where a folder is being dragged over the sidebar.
enum SidebarDropTarget: Hashable {
    case newTask
    case projects
    case project(UUID)
}

extension View {
    /// Accepts folders dropped from Finder, outlining the row while one hovers
    /// over it. The outline is transient, never a row background.
    func sidebarFolderDrop(
        _ target: SidebarDropTarget,
        current: Binding<SidebarDropTarget?>,
        perform: @escaping @MainActor ([URL]) -> Bool
    ) -> some View {
        dropDestination(for: URL.self) { urls, _ in
            perform(urls)
        } isTargeted: { isTargeted in
            if isTargeted {
                current.wrappedValue = target
            } else if current.wrappedValue == target {
                current.wrappedValue = nil
            }
        }
        .overlay {
            if current.wrappedValue == target {
                RoundedRectangle(cornerRadius: JetDesign.controlRadius)
                    .strokeBorder(JetDesign.accent, lineWidth: 2)
                    .allowsHitTesting(false)
            }
        }
    }
}
