import SwiftUI

struct NewTaskView: View {
    @Bindable var session: DesktopSession
    let composerFocused: FocusState<Bool>.Binding

    private let examples: [(String, String)] = [
        ("Understand a Project", "Explain what this Project does and how its main parts fit together. Start with a plain-language overview."),
        ("Make a change", "I would like to change "),
        ("Find a problem", "Help me investigate a problem in this Project. Here is what happens: ")
    ]

    var body: some View {
        GeometryReader { geometry in
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    JetMark()
                    Text("What are you working on?").accessibilityAddTraits(.isHeader)
                        .font(.system(size: 26, weight: .medium))
                        .tracking(-0.7)
                        .padding(.top, 20)
                    Text("Describe what you need. You can review the changes as you go.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .padding(.top, 8)
                        .padding(.bottom, 28)
                    if !session.canStartTask {
                        HStack(alignment: .firstTextBaseline, spacing: 12) {
                            Text(requirement)
                                .foregroundStyle(.secondary)
                            Button("Open setup", action: session.showProjects)
                                .buttonStyle(.plain).foregroundStyle(JetDesign.accent)
                        }
                        .font(.caption)
                        .padding(.bottom, 16)
                    }
                    WorkspaceComposer(session: session, composerFocused: composerFocused, starting: true)
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Start with an idea")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        ViewThatFits(in: .horizontal) {
                            HStack(spacing: 24) { exampleButtons }
                            VStack(alignment: .leading, spacing: 12) { exampleButtons }
                        }
                    }
                    .padding(.top, 28)
                    Text("Your tasks stay here, ready to continue.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(.top, 32)
                }
                .frame(maxWidth: JetDesign.writingWidth, alignment: .leading)
                .padding(32)
                .frame(maxWidth: .infinity, minHeight: geometry.size.height)
            }
        }
        .accessibilityIdentifier("new-task-workspace")
    }

    private var requirement: String {
        if !session.planeIsConnected { return "Connect Jet to start working on this computer." }
        if session.selectedProject == nil { return "Add a Project to give your work a home." }
        return "Choose an installed Harness to start."
    }

    @ViewBuilder private var exampleButtons: some View {
        ForEach(examples, id: \.0) { title, prompt in
            Button {
                session.draft = prompt
                composerFocused.wrappedValue = true
            } label: {
                HStack(spacing: 8) { Text(title); Image(systemName: "arrow.up.right").font(.caption2) }
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .font(.caption)
            .disabled(!session.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
    }
}

#if DEBUG
#Preview("New task") {
    DesktopPreviewScenes.view("new-task")
        .frame(width: 1280, height: 800)
}

#Preview("First launch, dark") {
    DesktopPreviewScenes.view("first-launch")
        .frame(width: 1280, height: 800)
        .preferredColorScheme(.dark)
}
#endif
