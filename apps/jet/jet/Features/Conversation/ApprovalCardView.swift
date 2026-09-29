import SwiftUI

/// A permission request in the transcript (design §6.10). The latest request the
/// reply waits for shows the full card; earlier and resolved ones collapse to one
/// line. There is no Approve button because no Command grants a request, and the
/// automatic review is never offered as the way out.
struct ApprovalCardView: View {
    let row: PermissionRow
    let session: DesktopSession

    @State private var showsDetails: Bool
    @Environment(\.transcriptScale) private var scale
    @Environment(\.colorSchemeContrast) private var contrast

    init(row: PermissionRow, session: DesktopSession, showsDetailsInitially: Bool = false) {
        self.row = row
        self.session = session
        _showsDetails = State(initialValue: showsDetailsInitially)
    }

    var body: some View {
        Group {
            switch row.presentation {
            case .pending:
                card
            case .asked:
                line(symbol: "hand.raised", tint: .secondary) {
                    Text("Asked for permission: \(subject)")
                }
            case .allowed:
                line(symbol: "checkmark.shield", tint: .green) {
                    Text("Allowed by the safety review: \(subject)")
                }
            case let .blocked(offersRetry):
                line(symbol: "xmark.shield", tint: .red) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Blocked by the safety review: \(subject)")
                        if offersRetry { retry }
                    }
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Permission request")
    }

    // MARK: - Pending card

    private var card: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label {
                Text(title).fontWeight(.semibold)
            } icon: {
                Image(systemName: "hand.raised.fill")
                    .foregroundStyle(.orange)
                    .accessibilityHidden(true)
            }
            if let command = row.command {
                Text("It wants to run: \(Text(command).font(TranscriptFont.code(scale)))")
            } else {
                Text("It wants to use: \(row.approval.tool)")
            }
            Text("Jet can't grant permissions from the app yet, so this reply is paused.")
                .foregroundStyle(secondaryText)
            HStack(spacing: 8) {
                Button("Interrupt and Reply…") { session.requestInterrupt(thenReply: true) }
                    .buttonStyle(.borderedProminent)
                    .disabled(!canInterrupt)
                    .help(interruptHelp)
                    .accessibilityIdentifier("interrupt-and-reply")
                Menu("More") {
                    if let command = row.command {
                        Button("Copy Command") { TranscriptPasteboard.copy(command) }
                    }
                    Button("Open Terminal") { session.showDetails(.terminal) }
                    Divider()
                    Button("Stop Assistant…") { session.requestStopAssistant() }
                        .disabled(!session.canStopRun)
                }
                .fixedSize()
                .accessibilityLabel("More actions for this request")
            }
            .controlSize(TranscriptFont.controlSize(scale))
            DisclosureGroup("Details", isExpanded: $showsDetails) {
                details.padding(.top, 6)
            }
        }
        .font(TranscriptFont.content(scale))
        .padding(12)
        .padding(.leading, 3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background.secondary)
        .overlay(alignment: .leading) {
            Rectangle().fill(.orange).frame(width: 3)
        }
        .clipShape(RoundedRectangle(cornerRadius: JetDesign.fieldRadius))
        .overlay {
            RoundedRectangle(cornerRadius: JetDesign.fieldRadius).stroke(.separator)
        }
    }

    private var details: some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
            detail("Tool", row.approval.tool)
            detail("Target", row.approval.target)
            detail("Scope", row.approval.scope)
            detail("Consequence", row.approval.consequence)
            GridRow {
                // The scrolling action has no baseline; both cells start at the top.
                label("Requested action")
                    .gridCellAnchor(.topLeading)
                ScrollView {
                    Text(row.approval.action)
                        .font(TranscriptFont.code(scale))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 160)
                .fixedSize(horizontal: false, vertical: true)
            }
            if let rationale = row.approval.rationale {
                detail("Reviewer's reason", rationale)
            }
        }
    }

    private func detail(_ name: LocalizedStringKey, _ value: String) -> some View {
        GridRow {
            label(name)
            Text(value)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func label(_ name: LocalizedStringKey) -> some View {
        Text(name)
            .foregroundStyle(secondaryText)
            .gridColumnAlignment(.leading)
    }

    // MARK: - Collapsed lines

    private func line<Content: View>(
        symbol: String,
        tint: Color,
        @ViewBuilder text: () -> Content
    ) -> some View {
        Label {
            text().foregroundStyle(secondaryText)
        } icon: {
            Image(systemName: symbol)
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(tint)
                .accessibilityHidden(true)
        }
        .font(TranscriptFont.content(scale))
        .frame(maxWidth: .infinity, alignment: .leading)
        .contextMenu {
            if let command = row.command {
                Button("Copy Command") { TranscriptPasteboard.copy(command) }
            }
        }
    }

    private var retry: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button("Ask Reviewer Again") {
                Task { await session.authorizeApprovalRetry(row.approval) }
            }
            .buttonStyle(.bordered)
            .controlSize(TranscriptFont.controlSize(scale))
            .disabled(!session.planeIsConnected || session.supervisionOperation != nil)
            .accessibilityIdentifier("ask-reviewer-again")
            Text("The same safety checks apply; it may be blocked again.")
                .font(TranscriptFont.metadata(scale))
                .foregroundStyle(secondaryText)
        }
    }

    // MARK: - Values

    private var subject: Text {
        Text(row.subject).font(TranscriptFont.code(scale))
    }

    private var title: String {
        if let name = session.selectedAssistantName {
            return String(localized: "\(name) is waiting for permission")
        }
        return String(localized: "The assistant is waiting for permission")
    }

    private var interruptHelp: String {
        if let name = session.selectedAssistantName {
            return String(localized: "Interrupt the current reply, then tell \(name) what to do instead")
        }
        return String(localized: "Interrupt the current reply, then tell the assistant what to do instead")
    }

    private var canInterrupt: Bool {
        session.canInterruptTurn && session.planeIsConnected && session.supervisionOperation == nil
    }

    /// Secondary text becomes primary under Increase Contrast.
    private var secondaryText: HierarchicalShapeStyle {
        contrast == .increased ? .primary : .secondary
    }
}
