import SwiftUI

struct RunSummaryView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        List {
            Section("Current Run") {
                LabeledContent("Lifecycle", value: lifecycleLabel)
                LabeledContent("Activity", value: activityLabel)
                LabeledContent("Runs on", value: "This Mac")
                LabeledContent(
                    "Checkpoint",
                    value: session.workDiff?.scope.label ?? "Unavailable"
                )
                LabeledContent(
                    "Latest Turn",
                    value: session.workDiff?.latestTurn.formatted() ?? "—"
                )
                LabeledContent(
                    "Changed files",
                    value: session.workDiff?.totalFiles.formatted() ?? "0"
                )
                if let run = session.selectedRun {
                    LabeledContent("Revision", value: run.revision.formatted())
                }
                if let termination = session.runExecution?.termination {
                    Text(termination.summary)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                HStack {
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

            Section("Turn queue") {
                if session.supervisionOperation == "refresh", session.turnQueue == nil {
                    ProgressView("Loading the authoritative queue")
                } else if let turns = session.turnQueue?.turns, !turns.isEmpty {
                    ForEach(turns) { turn in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(turn.state == .active ? "Current Turn" : "Position \(turn.position)")
                                    .font(.subheadline.weight(.semibold))
                                Text("\(turn.source.rawValue.replacingOccurrences(of: "_", with: " ")) · \(turn.targetLabel)")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                Text("Turn \(turn.sequence) · \(turn.state.rawValue.replacingOccurrences(of: "_", with: " "))")
                                    .font(.caption2)
                                    .foregroundStyle(.tertiary)
                            }
                            Spacer(minLength: 8)
                            if turn.withdrawable {
                                Button("Withdraw") {
                                    Task { await session.withdrawTurn(turn) }
                                }
                                .disabled(session.supervisionOperation != nil)
                            }
                        }
                    }
                } else {
                    Text("No active or queued Turns.")
                        .foregroundStyle(.secondary)
                }
                Text("Up to \(JetTurnQueue.maximumEntries) unsettled Turns; each prompt can contain \(JetTurnQueue.maximumPromptBytes.formatted()) UTF-8 bytes.")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
        }
        .listStyle(.inset)
        .refreshable { await session.loadWorkPanel() }
    }

    private var lifecycleLabel: String {
        if session.usesLivePlane {
            return session.selectedRun?.lifecycle.rawValue.capitalized ?? "Not started"
        }
        return session.scenario?.run?.lifecycle.rawValue.capitalized ?? "Not started"
    }

    private var activityLabel: String {
        session.runExecution?.activity?.rawValue
            .replacingOccurrences(of: "_", with: " ")
            .capitalized
            ?? (session.hasLiveRun ? "Starting" : "Idle")
    }
}
