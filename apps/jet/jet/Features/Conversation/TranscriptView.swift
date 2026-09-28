import SwiftUI

struct LiveTimelineView: View {
    let session: DesktopSession

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                if session.conversationFreshness == .cached {
                    HStack(alignment: .top, spacing: 10) {
                        Image(systemName: "wifi.slash")
                            .foregroundStyle(.orange)
                        VStack(alignment: .leading, spacing: 3) {
                            Text("Showing cached state")
                                .font(.subheadline.weight(.semibold))
                            Text("Jet will refresh this task after \(session.selectedPlaneName) reconnects.")
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .padding(12)
                    .background(.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: JetDesign.controlRadius))
                    .accessibilityElement(children: .combine)
                }

                if session.timeline.isEmpty {
                    ContentUnavailableView {
                        Label(
                            session.selectedConversationID == nil
                                ? "What should Jet do?"
                                : "Waiting for the first update",
                            systemImage: "text.bubble"
                        )
                    } description: {
                        Text(
                            session.selectedConversationID == nil
                                ? "Describe the outcome. Jet will create an isolated Workspace in the selected Project."
                                : "Your messages and results will appear here as work progresses."
                        )
                    }
                    .frame(maxWidth: .infinity, minHeight: 260)
                } else {
                    ForEach(session.timeline) { entry in
                        LiveTimelineEntryView(entry: entry, session: session)
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

private struct LiveTimelineEntryView: View {
    let entry: JetTimelineEntry
    let session: DesktopSession

    var body: some View {
        switch entry.kind {
        case .user:
            VStack(alignment: .leading, spacing: 10) {
                Text("You").font(.caption.weight(.medium)).foregroundStyle(.secondary)
                MessageText(text: entry.text).font(.body).textSelection(.enabled).lineSpacing(5)
                Divider().padding(.top, 14)
            }
        case .activity:
            Label(entry.text, systemImage: "bolt.horizontal.circle")
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityLabel("Activity: \(entry.text)")
        case .approval:
            if let approval = entry.approval {
                ApprovalCardView(approval: approval, session: session)
            }
        case .result, .agent:
            VStack(alignment: .leading, spacing: 10) {
                Text("Jet").font(.caption.weight(.medium)).foregroundStyle(.secondary)
                // Render native Markdown as text. OpenURL remains a platform action.
                MessageText(text: entry.text).textSelection(.enabled).font(.body).lineSpacing(5)
            }

        }
    }
}
