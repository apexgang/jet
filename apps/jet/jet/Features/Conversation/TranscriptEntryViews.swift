import Accessibility
import SwiftUI

/// Transcript type, multiplied by the person's text size (View › Bigger / Smaller).
/// Nothing goes below 10 pt.
enum TranscriptFont {
    static func content(_ scale: CGFloat) -> Font {
        .system(size: JetDesign.TextSize.content * scale)
    }

    static func heading(level: Int, scale: CGFloat) -> Font {
        level <= 2
            ? .system(size: JetDesign.TextSize.title * scale, weight: .semibold)
            : .system(size: JetDesign.TextSize.content * scale, weight: .semibold)
    }

    static func code(_ scale: CGFloat) -> Font {
        .system(size: 13 * scale, design: .monospaced)
    }

    static func control(_ scale: CGFloat) -> Font {
        .system(size: max(10, JetDesign.TextSize.control * scale))
    }

    static func metadata(_ scale: CGFloat) -> Font {
        .system(size: max(10, JetDesign.TextSize.metadata * scale), weight: .medium)
    }

    /// Bordered buttons size with the control size rather than the font.
    static func controlSize(_ scale: CGFloat) -> ControlSize {
        if scale >= 1.6 { return .extraLarge }
        if scale >= 1.2 { return .large }
        return .regular
    }
}

enum TranscriptPasteboard {
    /// Copies text and tells VoiceOver it happened.
    static func copy(_ text: String) {
        DesktopSession.copyToPasteboard(text)
        AccessibilityNotification.Announcement(String(localized: "Copied")).post()
    }
}

/// One transcript row: a message, a reply, a fold of steps, changes, a status
/// line, a permission request or a technical line.
struct TranscriptRowView: View {
    let row: TranscriptRow
    let session: DesktopSession
    @Binding var expandedSteps: Set<String>

    var body: some View {
        switch row {
        case let .you(you):
            YouRowView(row: you, session: session)
        case let .assistant(reply):
            AssistantRowView(row: reply, name: session.selectedAssistantName)
        case let .steps(steps):
            StepsRowView(row: steps, name: session.selectedAssistantName, isExpanded: Binding(
                get: { expandedSteps.contains(steps.id) },
                set: { expanded in
                    if expanded { expandedSteps.insert(steps.id) } else { expandedSteps.remove(steps.id) }
                }
            ))
        case let .changes(changes):
            ChangesRowView(row: changes, session: session)
        case let .status(status):
            StatusRowView(row: status, session: session)
        case let .permission(permission):
            ApprovalCardView(row: permission, session: session)
        case let .technical(_, text):
            TechnicalRowView(text: text)
        }
    }
}

/// "You · 14:32" or "Claude Code · 14:32" above a message.
private struct RowHeader: View {
    let text: String
    @Environment(\.transcriptScale) private var scale

    var body: some View {
        Text(text)
            .font(TranscriptFont.metadata(scale))
            .foregroundStyle(.secondary)
            .accessibilityAddTraits(.isHeader)
    }

    static func assistant(_ name: String?, at ms: Int64?) -> String {
        let name = name ?? String(localized: "Assistant")
        guard let ms else { return name }
        return String(localized: "\(name) · \(TranscriptFormat.time(ms))")
    }
}

private struct YouRowView: View {
    let row: YouRow
    let session: DesktopSession
    @Environment(\.transcriptScale) private var scale
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            RowHeader(text: row.recordedAt.map { String(localized: "You · \(TranscriptFormat.time($0))") }
                ?? String(localized: "You"))
            MessageText(text: row.text)
            if let queue = row.queue {
                HStack(spacing: 10) {
                    Label {
                        Text("Waiting to send · \(JetCopy.ordinal(queue.ordinal))")
                    } icon: {
                        Image(systemName: "clock").accessibilityHidden(true)
                    }
                    .foregroundStyle(contrast == .increased ? .primary : .secondary)
                    if queue.entry.withdrawable {
                        Button("Remove", action: remove)
                            .buttonStyle(.borderless)
                            .tint(JetDesign.accentText)
                            .help("Remove this message and put its text back in the message box")
                            .accessibilityIdentifier("queued-remove-\(row.id)")
                    }
                }
                .font(TranscriptFont.control(scale))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contextMenu { actions }
        .accessibilityActions { actions }
    }

    @ViewBuilder
    private var actions: some View {
        Button("Copy") { TranscriptPasteboard.copy(row.text) }
        Button("Edit as New Message") { session.editAsNewMessage(row.text) }
        if row.queue?.entry.withdrawable == true {
            Button("Remove", action: remove)
        }
    }

    private func remove() {
        guard let entry = row.queue?.entry else { return }
        Task { await session.removeQueuedMessage(entry) }
    }
}

private struct AssistantRowView: View {
    let row: AssistantRow
    let name: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if row.showsLabel {
                RowHeader(text: RowHeader.assistant(name, at: row.recordedAt))
            }
            MessageText(text: row.text)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contextMenu { copy }
        .accessibilityActions { copy }
    }

    private var copy: some View {
        Button("Copy") { TranscriptPasteboard.copy(row.text) }
    }
}

private struct StepsRowView: View {
    let row: StepsRow
    let name: String?
    @Binding var isExpanded: Bool
    @Environment(\.transcriptScale) private var scale
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if row.showsLabel {
                RowHeader(text: RowHeader.assistant(name, at: row.recordedAt))
            }
            DisclosureGroup(isExpanded: $isExpanded) {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(Array(row.items.enumerated()), id: \.offset) { _, item in
                        switch item {
                        case let .tool(tool):
                            Text(tool).font(TranscriptFont.code(scale))
                        case let .reasoning(text):
                            Text(text)
                                .font(TranscriptFont.content(scale))
                                .foregroundStyle(contrast == .increased ? .primary : .secondary)
                                .lineLimit(4)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 4)
                .padding(.leading, 18)
            } label: {
                HStack(spacing: 6) {
                    if row.isLive {
                        ProgressView().controlSize(.mini).accessibilityHidden(true)
                    }
                    Text(row.title)
                        .font(TranscriptFont.control(scale))
                        .foregroundStyle(contrast == .increased ? .primary : .secondary)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct ChangesRowView: View {
    let row: ChangesRow
    let session: DesktopSession
    @Environment(\.transcriptScale) private var scale

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label {
                if let name = session.selectedAssistantName {
                    Text("\(name) changed files in the working copy.")
                } else {
                    Text("The assistant changed files in the working copy.")
                }
            } icon: {
                Image(systemName: "checkmark.circle")
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(.green)
                    .accessibilityHidden(true)
            }
            .font(TranscriptFont.content(scale))
            HStack(spacing: 8) {
                Button("Review Changes") {
                    session.showChanges(.reply(turn: row.turn, runID: row.runID))
                }
                .help("Show this reply's changes in Details")
                .accessibilityIdentifier("changes-review")
                if row.offersKeep {
                    Button("Keep Changes…") { session.presentKeepChanges(mode: .plan) }
                        .disabled(!session.canKeepChanges)
                        .transcriptHelp(session.keepChangesUnavailableReason)
                        .accessibilityIdentifier("changes-keep")
                }
            }
            .buttonStyle(.bordered)
            .controlSize(TranscriptFont.controlSize(scale))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct StatusRowView: View {
    let row: StatusRow
    let session: DesktopSession
    @Environment(\.transcriptScale) private var scale
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label {
                Text(row.kind.title)
                    .foregroundStyle(needsAttention || contrast == .increased ? .primary : .secondary)
            } icon: {
                Image(systemName: symbol)
                    // The filled error symbol keeps full strength; outlines stay light.
                    .symbolRenderingMode(row.kind == .failed ? .monochrome : .hierarchical)
                    .foregroundStyle(tint)
                    .accessibilityHidden(true)
            }
            .font(TranscriptFont.content(scale))
            if row.kind == .failed || row.kind == .stopUnconfirmed {
                HStack(spacing: 8) {
                    if let text = row.sendAgainText {
                        Button("Send Again") {
                            Task { await session.sendAgain(text) }
                        }
                        .disabled(!session.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .help("Send Again works when the message box is empty.")
                        .accessibilityIdentifier("status-send-again")
                    }
                    Button("Details") { session.showDetails(.run) }
                }
                .buttonStyle(.bordered)
                .controlSize(TranscriptFont.controlSize(scale))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var needsAttention: Bool {
        row.kind == .failed || row.kind == .stopUnconfirmed
    }

    private var symbol: String {
        switch row.kind {
        case .interrupted, .stopped, .canceled, .lost: "stop.circle"
        case .stopUnconfirmed: "questionmark.circle"
        case .failed: "xmark.octagon.fill"
        }
    }

    private var tint: Color {
        switch row.kind {
        case .interrupted, .stopped, .canceled, .lost: .secondary
        case .stopUnconfirmed: .orange
        case .failed: .red
        }
    }
}

private struct TechnicalRowView: View {
    let text: String
    @Environment(\.transcriptScale) private var scale

    var body: some View {
        Label {
            Text(text)
        } icon: {
            Image(systemName: "gearshape").accessibilityHidden(true)
        }
        .font(TranscriptFont.metadata(scale))
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

extension View {
    /// A tooltip only when there is something to say.
    @ViewBuilder
    func transcriptHelp(_ text: String?) -> some View {
        if let text { help(text) } else { self }
    }
}
