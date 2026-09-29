import SwiftUI

/// The detail column for New Task and an open task. The shell's toolbar owns the
/// title, status and task actions; this view holds the transcript and composer.
struct ConversationView: View {
    @Bindable var session: DesktopSession
    @FocusState private var composerFocused: Bool

    var body: some View {
        if session.usesLivePlane {
            liveContent
                // The only observer of focus requests: ⌘N, Return on a row, Remove.
                .onChange(of: session.composerFocusRequest) { _, _ in
                    composerFocused = true
                }
        } else {
            fixtureContent
        }
    }

    @ViewBuilder
    private var liveContent: some View {
        if session.selectedConversationID == nil {
            NewTaskView(session: session, composerFocused: $composerFocused)
        } else {
            TranscriptView(session: session)
                .id(session.selectedConversationID)
                .safeAreaBar(edge: .bottom) {
                    WorkspaceComposer(session: session, composerFocused: $composerFocused)
                }
        }
    }

    @ViewBuilder
    private var fixtureContent: some View {
        switch session.contentState {
        case .loading:
            ProgressView("Loading Tasks…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case let .failed(message):
            ContentUnavailableView {
                Label("Couldn't Load Tasks", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            } actions: {
                Button("Try Again", action: session.retryFixtureLoad)
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
