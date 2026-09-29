import SwiftUI

/// Details › Activity: what the assistant is doing now, messages waiting to send,
/// the working copy, Git activity and Technical Details.
struct DetailsActivityView: View {
    @Bindable var session: DesktopSession
    @Bindable var model: DetailsPanelModel

    var body: some View {
        Form {
            Section("Now") {
                now
            }
            if session.usesLivePlane {
                let queued = session.queuedMessages
                if !queued.isEmpty {
                    Section("Waiting to Send") {
                        ForEach(queued) { message in
                            queuedRow(message)
                        }
                    }
                }
                Section("Working Copy") {
                    workingCopy
                }
                Section {
                    GitActivityList(session: session)
                }
                Section {
                    DisclosureGroup("Technical Details", isExpanded: $model.showsTechnicalDetails) {
                        DetailsTechnicalDetails(session: session)
                    }
                }
            }
        }
        .formStyle(.grouped)
    }

    private var assistantStart: String {
        DetailsCopy.assistant(session.selectedAssistantName, position: .start)
    }

    private var assistantMid: String {
        DetailsCopy.assistant(session.selectedAssistantName, position: .mid)
    }

    // MARK: Now

    @ViewBuilder
    private var now: some View {
        TaskStatusLabel(status: session.selectedTaskStatus)
        if let run = session.selectedRun {
            TimelineView(.everyMinute) { context in
                Group {
                    if run.lifecycle.isLive {
                        Text("Started \(JetCopy.relative(ms: run.createdAtUnixMilliseconds, now: context.date))")
                    } else if let ended = run.endedAtUnixMilliseconds {
                        Text("Ended \(JetCopy.relative(ms: ended, now: context.date))")
                    }
                }
                .font(.system(size: JetDesign.TextSize.control))
                .foregroundStyle(.secondary)
            }
            if run.lifecycle.isLive {
                HStack(spacing: 8) {
                    Button("Interrupt…") { session.requestInterrupt(thenReply: false) }
                        .disabled(!session.canInterruptTurn || session.supervisionOperation != nil || session.detailsIsOffline)
                        .accessibilityIdentifier("activity-interrupt")
                    Button("Stop Assistant…") { session.requestStopAssistant() }
                        .disabled(!session.canStopRun || session.supervisionOperation != nil || session.detailsIsOffline)
                        .accessibilityIdentifier("activity-stop-assistant")
                }
            } else if !statusSaysSendToContinue {
                Text("Send a message to continue.")
                    .font(.system(size: JetDesign.TextSize.control))
                    .foregroundStyle(.secondary)
            }
        } else {
            Text("\(assistantStart) hasn't started on this task yet.")
                .font(.system(size: JetDesign.TextSize.control))
                .foregroundStyle(.secondary)
        }
    }

    /// "Finished · send a message to continue" and "Stopped · …" already say it.
    private var statusSaysSendToContinue: Bool {
        switch session.selectedTaskStatus {
        case .finished, .stopped: true
        default: false
        }
    }

    // MARK: Waiting to Send

    private func queuedRow(_ message: DetailsQueuedMessage) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(verbatim: message.ordinal)
                .font(.system(size: JetDesign.TextSize.metadata).monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(minWidth: 24, alignment: .leading)
            Text(verbatim: message.text)
                .font(.system(size: JetDesign.TextSize.control))
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
            if message.entry.withdrawable {
                Button("Remove") {
                    Task { await session.removeQueuedMessage(message.entry) }
                }
                .controlSize(.small)
                .disabled(session.supervisionOperation != nil || session.detailsIsOffline)
            }
        }
    }

    // MARK: Working copy

    @ViewBuilder
    private var workingCopy: some View {
        if let path = session.detailsWorkingCopyPath, session.selectedRun != nil || session.detailsSnapshot?.workspaceRoot != nil {
            LabeledContent("Location") {
                Text(verbatim: path)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .help(path)
            }
        }
        Text(workingCopySentence)
            .font(.system(size: JetDesign.TextSize.control))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        if session.detailsWorkingCopyPath != nil, session.selectedRun != nil {
            HStack(spacing: 8) {
#if os(macOS)
                if session.canRevealDetailsFolder {
                    Button("Show in Finder") { session.revealDetailsFolder() }
                }
#endif
                Button("Copy Path") { session.copyDetailsFolderPath() }
            }
        }
    }

    private var workingCopySentence: String {
        guard session.selectedRun != nil else {
            return String(localized: "Jet creates a working copy when \(assistantMid) starts.")
        }
        let project = session.detailsProjectName
        if session.detailsWorksInProjectFolder {
            return project.map { String(localized: "\(assistantStart) works directly in your \($0) folder.") }
                ?? String(localized: "\(assistantStart) works directly in your project folder.")
        }
        return project.map {
            String(localized: "\(assistantStart) works in this separate copy. Your \($0) folder doesn't change until you keep the changes.")
        } ?? String(localized: "\(assistantStart) works in this separate copy. Your project folder doesn't change until you keep the changes.")
    }
}

/// Technical Details: the domain facts behind the casual view (design §6.8).
/// Domain words are allowed here.
struct DetailsTechnicalDetails: View {
    @Bindable var session: DesktopSession

    var body: some View {
        if let conversationID = session.selectedConversationID {
            DetailsTechnicalRow("Task ID", conversationID.uuidString.lowercased()) {
                Button {
                    session.copyTaskID()
                } label: {
                    Label("Copy Task ID", systemImage: "document.on.document")
                        .labelStyle(.iconOnly)
                        .frame(width: 28, height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.borderless)
                .tint(.secondary)
                .help("Copy Task ID")
            }
        }
        DetailsTechnicalRow("Computer", session.selectedPlaneName)
        if let run = session.selectedRun {
            DetailsTechnicalRow("Run ID", run.id.uuidString.lowercased())
            DetailsTechnicalRow("Run lifecycle", run.lifecycle.rawValue)
            DetailsTechnicalRow("Run activity", session.runExecution?.activity?.rawValue ?? "—")
            DetailsTechnicalRow("Run revision", JetCopy.number(run.revision))
        }
        if let snapshot = session.detailsSnapshot {
            DetailsTechnicalRow("Runs", JetCopy.number(snapshot.runs.count))
            DetailsTechnicalRow("Workspace ID", snapshot.workspaceID?.uuidString.lowercased() ?? "None (local checkout)")
            if let root = snapshot.workspaceRoot {
                DetailsTechnicalRow("Workspace root", root)
            }
            DetailsTechnicalRow("Event cursor", JetCopy.number(snapshot.cursor))
        }
        if let diff = session.workDiff {
            DetailsTechnicalRow("Diff scope", diff.scope.label)
            DetailsTechnicalRow("Latest checkpoint Turn", JetCopy.number(diff.latestTurn))
            DetailsTechnicalRow(
                "Patch artifact",
                "\(JetCopy.byteCount(Int64(diff.artifact.size))) · \(diff.artifact.availability.rawValue) · \(String(diff.artifact.sha256.prefix(12)))"
            )
            .help(diff.artifact.sha256)
            DetailsTechnicalRow(
                "Patch loaded",
                "\(JetCopy.number(session.workPatchBytesLoaded)) of \(JetCopy.number(diff.artifact.size)) bytes"
            )
        }
        if let file = session.editableFile {
            DetailsTechnicalRow("Edited file revision", file.revision.label)
        }
        if let termination = session.runExecution?.termination {
            DetailsTechnicalRow("Termination", termination.summary)
        }
        ForEach(session.turnQueue?.turns ?? []) { turn in
            DetailsTechnicalRow(
                "Queue position \(turn.position)",
                "Turn \(turn.sequence) · \(turn.state.rawValue) · \(turn.source.rawValue)"
            )
        }
        Text("Up to \(JetTurnQueue.maximumEntries) unsettled Turns · \(JetCopy.number(JetTurnQueue.maximumPromptBytes)) bytes per prompt")
            .font(.system(size: JetDesign.TextSize.metadata))
            .foregroundStyle(.secondary)
    }
}

/// A label over a selectable monospaced value, so long identifiers wrap instead
/// of truncating at 300 pt.
struct DetailsTechnicalRow<Accessory: View>: View {
    let label: LocalizedStringKey
    let value: String
    let accessory: Accessory

    init(_ label: LocalizedStringKey, _ value: String, @ViewBuilder accessory: () -> Accessory) {
        self.label = label
        self.value = value
        self.accessory = accessory()
    }

    var body: some View {
        HStack(alignment: .center, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(label)
                    .font(.system(size: JetDesign.TextSize.metadata))
                    .foregroundStyle(.secondary)
                Text(verbatim: value)
                    .font(.system(size: JetDesign.TextSize.metadata, design: .monospaced))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            accessory
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
    }
}

extension DetailsTechnicalRow where Accessory == EmptyView {
    init(_ label: LocalizedStringKey, _ value: String) {
        self.init(label, value) { EmptyView() }
    }
}

#if DEBUG
#Preview { DesktopPreviewScenes.details[0].makeView() }
#endif
