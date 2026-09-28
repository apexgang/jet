import SwiftUI

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
