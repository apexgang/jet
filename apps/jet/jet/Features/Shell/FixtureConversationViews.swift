import SwiftUI

struct FixtureConversationHeader: View {
    let session: DesktopSession
    let scenario: DesktopFixtureScenario

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 14) {
                    Text(scenario.conversation?.title ?? "New task")
                        .font(.headline)
                        .fixedSize(horizontal: true, vertical: false)
                    Spacer(minLength: 12)
                    StatusLabel(scenario: scenario)
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text(scenario.conversation?.title ?? "New task")
                        .font(.headline)
                    StatusLabel(scenario: scenario)
                }
            }
            Text("\(session.selectedProjectName) · Runs on \(scenario.plane.name)")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 13)
    }
}

private struct StatusLabel: View {
    let scenario: DesktopFixtureScenario
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        Label(label, systemImage: symbol)
            .font(.caption.weight(.medium))
            .foregroundStyle(contrast == .increased ? Color.primary : color)

            .accessibilityLabel("Task status: \(label)")
    }

    private var label: String {
        switch scenario.state {
        case .firstLaunch: return "Connecting"
        case .ready: return "Ready"
        case .queued: return "Queued"
        case .completed: return "Completed"
        case .offline: return "Offline"
        case .staleCursor: return "Refreshing"
        case .denied: return "Action denied"
        case .unsupported: return "Unsupported"
        case .recovery: return "Recovery needed"
        case .active, .approval: break
        }

        if let activity = scenario.run?.activity {
            switch activity {
            case .working: return "Working"
            case .waitingForUser: return "Waiting for you"
            case .waitingForApproval: return "Approval needed"
            case .waitingForAuth: return "Sign-in needed"
            case .waitingForQuota: return "Usage limited"
            case .reconnecting: return "Reconnecting"
            }
        } else if scenario.run?.lifecycle == .completed {
            return "Completed"
        } else {
            return "Ready"
        }
    }

    private var symbol: String {
        switch scenario.run?.activity {
        case .working: "sparkles"
        case .waitingForApproval, .waitingForAuth, .waitingForUser: "exclamationmark.circle"
        case .waitingForQuota, .reconnecting: "arrow.clockwise"
        case nil: scenario.run?.lifecycle == .completed ? "checkmark.circle" : "circle"
        }
    }

    private var color: Color {
        switch scenario.run?.activity {
        case .waitingForApproval, .waitingForAuth, .waitingForUser, .waitingForQuota, .reconnecting:
            .orange
        case .working:
            JetDesign.accent
        case nil:
            scenario.run?.lifecycle == .completed ? .green : .secondary
        }
    }
}

struct FixtureTimelineView: View {
    let scenario: DesktopFixtureScenario

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                if let notice = scenario.notice {
                    NoticeView(notice: notice)
                }

                if scenario.timeline.isEmpty {
                    ContentUnavailableView {
                        Label("What should Jet do?", systemImage: "text.bubble")
                    } description: {
                        Text("Describe the outcome. You can choose where it runs before sending.")
                    }
                    .frame(maxWidth: .infinity, minHeight: 260)
                } else {
                    ForEach(scenario.timeline, id: \.id) { entry in
                        TimelineEntryView(entry: entry)
                    }
                }
            }
            .frame(maxWidth: 760)
            .padding(.horizontal, 28)
            .padding(.vertical, 24)
            .frame(maxWidth: .infinity)
        }
        .defaultScrollAnchor(.bottom)
    }
}

private struct NoticeView: View {
    let notice: DesktopNoticeFixture

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(color)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                Text(notice.title)
                    .font(.subheadline.weight(.semibold))
                Text(notice.message)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(color.opacity(0.08), in: RoundedRectangle(cornerRadius: JetDesign.controlRadius))
        .accessibilityElement(children: .combine)
    }

    private var symbol: String {
        switch notice.tone {
        case .informational: "info.circle"
        case .warning: "exclamationmark.triangle"
        case .critical: "xmark.octagon"
        }
    }

    private var color: Color {
        switch notice.tone {
        case .informational: JetDesign.accent
        case .warning: .orange
        case .critical: .red
        }
    }
}

private struct TimelineEntryView: View {
    let entry: DesktopTimelineEntryFixture

    var body: some View {
        switch entry.kind {
        case .user:
            HStack {
                Spacer(minLength: 52)
                Text(entry.text)
                    .textSelection(.enabled)
                    .padding(.horizontal, 13)
                    .padding(.vertical, 9)
                    .background(.quaternary, in: RoundedRectangle(cornerRadius: 13))
            }
        case .activity:
            Label(entry.text, systemImage: "bolt.horizontal.circle")
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityLabel("Activity: \(entry.text)")
        case .approval:
            Label(entry.text, systemImage: "checkmark.shield")
                .font(.subheadline)
                .foregroundStyle(.orange)
        case .result:
            Label(entry.text, systemImage: "checkmark.circle")
                .font(.subheadline)
                .foregroundStyle(.green)
        case .recovery:
            Label(entry.text, systemImage: "lifepreserver")
                .font(.subheadline)
                .foregroundStyle(.orange)
        case .agent:
            Text(entry.text)
                .textSelection(.enabled)
                .font(.body)
                .lineSpacing(3)
        }
    }
}

struct FixtureComposerView: View {
    @Bindable var session: DesktopSession
    let scenario: DesktopFixtureScenario
    let composerFocused: FocusState<Bool>.Binding

    var body: some View {
        VStack(spacing: 8) {
            if let actionNotice = session.actionNotice {
                Text(actionNotice)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: 760, alignment: .leading)
                    .accessibilityLabel(actionNotice)
            }

            HStack(alignment: .bottom, spacing: 10) {
                TextField(
                    "Describe what you want Jet to do",
                    text: $session.draft,
                    axis: .vertical
                )
                .textFieldStyle(.plain)
                .lineLimit(2 ... 6)
                .focused(composerFocused)
                .accessibilityLabel("Task message")

                Button("Send") {
                    Task { await session.submitDraft() }
                }
                    .buttonStyle(.borderedProminent)
                    .disabled(!session.canSubmitDraft)
                    .keyboardShortcut(.return, modifiers: [.command])
            }
            .padding(12)
            .background(.background, in: RoundedRectangle(cornerRadius: JetDesign.fieldRadius))
            .overlay {
                RoundedRectangle(cornerRadius: JetDesign.fieldRadius)
                    .stroke(.separator, lineWidth: 1)
            }

            ViewThatFits(in: .horizontal) {
                HStack(spacing: 12) {
                    ContextValue(label: "Project", value: session.selectedProjectName)
                    ContextValue(label: "Agent", value: session.selectedHarnessName)
                    ContextValue(label: "Runs on", value: scenario.plane.name)
                    Spacer(minLength: 0)
                }
                VStack(alignment: .leading, spacing: 6) {
                    ContextValue(label: "Project", value: session.selectedProjectName)
                    ContextValue(label: "Agent", value: session.selectedHarnessName)
                    ContextValue(label: "Runs on", value: scenario.plane.name)
                }
            }
            .frame(maxWidth: 760)
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 24)
        .padding(.top, 12)
        .padding(.bottom, 16)
        .modifier(LegibleBarBackground())
    }
}

private struct ContextValue: View {
    let label: String
    let value: String

    var body: some View {
        HStack(spacing: 4) {
            Text(label)
                .foregroundStyle(.tertiary)
            Text(value)
                .foregroundStyle(.secondary)
        }
        .font(.caption)
        .accessibilityElement(children: .combine)
    }
}
