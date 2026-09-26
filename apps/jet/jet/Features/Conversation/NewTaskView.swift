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

struct WorkspaceComposer: View {
    @Bindable var session: DesktopSession
    let composerFocused: FocusState<Bool>.Binding
    var starting = false

    private var canSend: Bool {
        session.canSubmitDraft && session.conversationOperation == nil && session.planeIsConnected
            && (!starting || session.canStartTask)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let notice = session.actionNotice {
                Text(notice).font(.caption).foregroundStyle(.secondary)
            }
            if session.queueIsFull {
                Label("There are too many messages waiting. Remove one from the queue or wait for work to finish.", systemImage: "exclamationmark.circle")
                    .font(.caption).foregroundStyle(.orange)
            }
            VStack(alignment: .leading, spacing: 8) {
                TextField(starting ? "What would you like to work on?" : "Reply or describe the next step…", text: $session.draft, axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(.body)
                    .lineLimit(starting ? 4...8 : 2...6)
                    .focused(composerFocused)
                    .accessibilityLabel("Task message")
                    .padding(.horizontal, 16)
                    .padding(.top, 16)
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .center, spacing: 12) { context; Spacer(minLength: 8); sendButton }
                    VStack(alignment: .leading, spacing: 8) { context; HStack { Spacer(); sendButton } }
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 12)
            }
            .background(.background, in: RoundedRectangle(cornerRadius: JetDesign.fieldRadius))
            .overlay {
                RoundedRectangle(cornerRadius: JetDesign.fieldRadius)
                    .stroke(composerFocused.wrappedValue ? JetDesign.accent : Color.secondary.opacity(0.35), lineWidth: composerFocused.wrappedValue ? 2 : 1)
            }
            HStack(spacing: 12) {
                Text(starting ? "Separate working copy · \(session.selectedPlaneName)" : "Runs on \(session.selectedPlaneName)")
                Spacer(minLength: 4)
                if session.draftBytes > JetTurnQueue.maximumPromptBytes * 4 / 5 {
                    Text("\(session.draftBytes.formatted()) / \(JetTurnQueue.maximumPromptBytes.formatted()) bytes")
                        .foregroundStyle(session.draftBytes > JetTurnQueue.maximumPromptBytes ? Color.red : Color.secondary)
                } else {
                    Text("⌘ Return to send")
                }
            }
            .font(.caption2)
            .foregroundStyle(.secondary)
        }
        .frame(maxWidth: JetDesign.readingWidth)
    }

    @ViewBuilder private var context: some View {
        if starting {
            HStack(spacing: 12) {
                Menu {
                    ForEach(session.selectedSetupSnapshot?.projects.projects ?? []) { project in
                        Button(project.name) { session.selectedProjectID = project.id }
                    }
                    Divider()
                    Button("Add Project…", action: session.requestAddProject)
                } label: { Label(session.selectedProjectName, systemImage: "folder") }
                .help("Choose the Project for this task")
                Menu {
                    ForEach(session.selectedSetupSnapshot?.capabilities.crafts ?? []) { craft in
                        Button(DesktopSession.harnessLabel(craft.harnesses.first ?? craft.id)) { session.chooseCraft(craft.id) }
                    }
                } label: { Text(session.selectedHarnessName) }
                .help("Choose Codex, Claude Code, or another installed Harness")
                if session.planes.count > 1 {
                    Menu {
                        ForEach(session.planes) { plane in
                            Button(plane.name) { session.chooseNewTaskPlane(plane.id) }
                        }
                    } label: { Image(systemName: "desktopcomputer") }
                    .help("Runs on \(session.selectedPlaneName)")
                }
            }
            .menuStyle(.borderlessButton)
            .fixedSize(horizontal: false, vertical: true)
            .disabled(session.conversationOperation != nil)
        } else {
            Text(session.hasLiveRun ? "Messages are sent in order" : "Continue this task")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private var sendButton: some View {
        Button {
            Task { await session.submitDraft() }
        } label: {
            HStack(spacing: 12) {
                Text(session.conversationOperation != nil ? "Sending…" : starting ? "Start task" : "Send")
                Image(systemName: "arrow.up")
            }
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.regular)
        .disabled(!canSend)
        .keyboardShortcut(.return, modifiers: [.command])
        .accessibilityIdentifier("send-task")
    }
}

private struct NewTaskPreview: View {
    @State private var session = DesktopSession()
    @FocusState private var composerFocused: Bool

    var body: some View {
        NewTaskView(session: session, composerFocused: $composerFocused)
    }
}

#Preview("New task, compact light") {
    NewTaskPreview().frame(width: 640, height: 600).preferredColorScheme(.light)
}

#Preview("New task, disconnected dark") {
    NewTaskPreview().frame(width: 900, height: 740).preferredColorScheme(.dark)
}
