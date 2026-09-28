import SwiftUI

struct ApprovalCardView: View {
    let approval: JetApprovalPresentation
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .firstTextBaseline) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(approval.tool)
                        .font(.headline)
                    Text(stateLabel)
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.orange)
                }
                Spacer(minLength: 12)
                Text(approval.scope)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("Target").foregroundStyle(.secondary)
                Text(approval.target).textSelection(.enabled)
                Text("Consequence").foregroundStyle(.secondary)
                Text(approval.consequence)
            }
            .font(.caption)

            VStack(alignment: .leading, spacing: 5) {
                Text("Requested action")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                Text(approval.action)
                    .font(.caption.monospaced())
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(9)
                    .background(.background, in: RoundedRectangle(cornerRadius: 7))
            }

            if let rationale = approval.rationale {
                Text(rationale)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) {
                    approvalDecision
                    Spacer(minLength: 8)
                    runControls
                }
                VStack(alignment: .leading, spacing: 8) {
                    approvalDecision
                    runControls
                }
            }
        }
        .padding(16)
        .background(.background, in: RoundedRectangle(cornerRadius: 4))
        .overlay {
            RoundedRectangle(cornerRadius: 4)
                .stroke(.orange.opacity(0.35), lineWidth: 1)
        }
        .accessibilityElement(children: .contain)
    }

    private var stateLabel: String {
        switch approval.state {
        case .requested: "Approval needed"
        case .allowed: "Action allowed"
        case .denied: "Action denied"
        case .unavailable: "Decision needed"
        }
    }

    @ViewBuilder
    private var approvalDecision: some View {
        if approval.canAuthorizeRetry {
            Button("Authorize one retry") {
                Task { await session.authorizeApprovalRetry(approval) }
            }
            .buttonStyle(.borderedProminent)
            .disabled(session.supervisionOperation != nil)
        } else if approval.state == .requested || approval.state == .unavailable {
            Text("This version of Jet cannot answer this request. Interrupt the Turn or stop the Run to continue safely.")
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
    }

    private var runControls: some View {
        HStack(spacing: 8) {
            Button("Interrupt Turn…") {
                session.requestRunControl(.interruptTurn)
            }
            .disabled(!session.canInterruptTurn || session.supervisionOperation != nil)
            Button("Stop Run…", role: .destructive) {
                session.requestRunControl(.stopRun)
            }
            .disabled(!session.canStopRun || session.supervisionOperation != nil)
        }
    }
}
