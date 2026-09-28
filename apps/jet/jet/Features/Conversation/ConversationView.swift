import SwiftUI

struct ConversationView: View {
    @Bindable var session: DesktopSession
    @FocusState private var composerFocused: Bool

    var body: some View {
        if session.usesLivePlane {
            LiveConversationView(session: session, composerFocused: $composerFocused)
                .onChange(of: session.composerFocusRequest) { _, _ in
                    composerFocused = true
                }
        } else {
            fixtureContent
        }
    }

    @ViewBuilder
    private var fixtureContent: some View {
        switch session.contentState {
        case .loading:
            ProgressView("Loading workspace")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case let .failed(message):
            ContentUnavailableView {
                Label("Workspace unavailable", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            } actions: {
                Button("Try again", action: session.retryFixtureLoad)
            }
        case let .ready(scenario):
            VStack(spacing: 0) {
                FixtureConversationHeader(session: session, scenario: scenario)
                Divider()
                FixtureTimelineView(scenario: scenario)
                Divider()
                FixtureComposerView(session: session, scenario: scenario, composerFocused: $composerFocused)
            }
            .navigationTitle(scenario.conversation?.title ?? "New task")
            .onChange(of: session.composerFocusRequest) { _, _ in
                composerFocused = true
            }
        }
    }
}

private struct LiveConversationView: View {
    @Bindable var session: DesktopSession
    let composerFocused: FocusState<Bool>.Binding

    var body: some View {
        Group {
            if session.selectedConversationID == nil {
                NewTaskView(session: session, composerFocused: composerFocused)
            } else {
                VStack(spacing: 0) {
                    HStack(spacing: 16) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(session.selectedConversationTitle).font(.headline).lineLimit(1)
                            Text("\(session.selectedProjectName) · \(session.selectedPlaneName)")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 12)
                        LiveStatusLabel(session: session)
                        Button("Changes") { session.selectedWorkPanel = .changes; session.isWorkPanelPresented = true }
                            .disabled(session.selectedRun == nil)
                        Menu {
                        Button("Rename task…") { session.isRenamePresented = true }
                            .disabled(session.selectedConversation?.revision == nil || !session.planeIsConnected)
                        Divider()
                            Button("Open Terminal") { session.selectedWorkPanel = .terminal; session.isWorkPanelPresented = true }
                            Divider()
                            Button("Interrupt Turn…") { session.requestRunControl(.interruptTurn) }.disabled(!session.canInterruptTurn)
                            Button("Stop Run…", role: .destructive) { session.requestRunControl(.stopRun) }.disabled(!session.canStopRun)
                        } label: { Image(systemName: "ellipsis") }
                        .menuIndicator(.hidden)
                        .help("Task actions")
                    }
                    .padding(.horizontal, 24).padding(.vertical, 16)
                    Divider()
                    LiveTimelineView(session: session)
                    WorkspaceComposer(session: session, composerFocused: composerFocused)
                        .padding(.horizontal, 28).padding(.top, 16).padding(.bottom, 20)
                }
            }
        }
        .navigationTitle(session.selectedConversationTitle)
    }
}

private struct LiveStatusLabel: View {
    let session: DesktopSession
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        Label(label, systemImage: symbol)
            .font(.caption.weight(.medium))
            .foregroundStyle(contrast == .increased ? Color.primary : color)

            .accessibilityLabel("Task status: \(label)")
    }

    private var label: String {
        if session.conversationFreshness == .cached { return "Offline cache" }
        if !session.planeIsConnected { return "Reconnecting" }
        if session.conversationOperation != nil { return "Loading" }
        switch session.runExecution?.activity {
        case .waitingForApproval: return "Approval needed"
        case .waitingForUser: return "Waiting for you"
        case .waitingForAuth: return "Sign-in needed"
        case .waitingForQuota: return "Usage limited"
        case .reconnecting: return "Reconnecting"
        case .working, nil: break
        }
        switch session.selectedRun?.lifecycle {
        case .starting: return "Starting"
        case .active: return "Working"
        case .stopping: return "Stopping"
        case .completed: return "Completed"
        case .failed: return "Failed"
        case .canceled: return "Canceled"
        case .lost: return "Recovery needed"
        case .created, nil: return "Ready"
        }
    }

    private var symbol: String {
        if !session.planeIsConnected { return "arrow.clockwise" }
        if session.runExecution?.needsAttention == true {
            return "exclamationmark.circle"
        }
        return switch session.selectedRun?.lifecycle {
        case .starting, .active, .stopping: "bolt.horizontal.circle"
        case .completed: "checkmark.circle"
        case .failed, .canceled, .lost: "exclamationmark.circle"
        case .created, nil: "circle"
        }
    }

    private var color: Color {
        if session.conversationFreshness == .cached { return .orange }
        if !session.planeIsConnected { return .orange }
        if session.runExecution?.needsAttention == true { return .orange }
        return switch session.selectedRun?.lifecycle {
        case .starting, .active, .stopping:
            JetDesign.accent
        case .completed: .green
        case .failed, .canceled, .lost: .orange
        case .created, nil: .secondary
        }
    }
}
