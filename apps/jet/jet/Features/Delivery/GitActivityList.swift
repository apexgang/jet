import SwiftUI

/// Details › Activity › Git Activity: every Git step of the open task in plain
/// sentences, newest first, with the step's next action (design §6.8, §6.9).
struct GitActivityList: View {
    @Bindable var session: DesktopSession

    @State private var showsAll = false

    static let initialCount = 10

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            if let ref = session.selectedDeliveryRef {
                content(ref)
                    .task(id: ref) { await session.observeDeliveries(for: ref) }
                    .onChange(of: session.gitDeliveries) { _, deliveries in
                        session.deliveries.absorb(deliveries, for: session.deliveryKey(for: ref))
                    }
            } else {
                placeholder(String(localized: "Git steps for this task appear here."))
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("git-activity")
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            Text("Git Activity")
                .font(.system(size: JetDesign.TextSize.navigation, weight: .semibold))
                .accessibilityAddTraits(.isHeader)
            Spacer()
            if let ref = session.selectedDeliveryRef {
                let connected = session.isDeliveryConnected(ref)
                Button {
                    Task { await session.checkGitStatus(for: ref) }
                } label: {
                    Label("Check Status", systemImage: "arrow.clockwise")
                        .labelStyle(.iconOnly)
                        .frame(minWidth: 28, minHeight: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.borderless)
                .tint(JetDesign.accentText)
                .disabled(!connected)
                .help(connected ? Text("Check Status") : Text("Not connected to \(session.deliveryComputerName(ref))."))
            }
        }
    }

    @ViewBuilder
    private func content(_ ref: ConversationRef) -> some View {
        let history = session.deliveryHistory(for: ref)
        let status = session.deliveryHistoryStatus(for: ref)
        if history.isEmpty {
            switch status {
            case .loading, .unknown:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Loading Git activity…").foregroundStyle(.secondary)
                }
                .font(.system(size: JetDesign.TextSize.control))
            case .failed:
                InlineNotice(
                    notice: ComposerNotice(
                        kind: .error,
                        text: String(localized: "Couldn't load Git activity."),
                        action: .tryAgainConnection
                    )
                ) { _ in
                    Task { await session.checkGitStatus(for: ref) }
                }
            case .loaded:
                placeholder(String(localized: "Git steps for this task appear here."))
            }
        } else {
            let visible = showsAll ? history : Array(history.prefix(Self.initialCount))
            let fixable = GitActivityRules.newestUnsupersededFailure(in: history)
            VStack(alignment: .leading, spacing: 14) {
                ForEach(visible) { delivery in
                    GitActivityRow(
                        session: session,
                        ref: ref,
                        delivery: delivery,
                        history: history,
                        showsFix: delivery.id == fixable?.id
                    )
                }
                if history.count > visible.count {
                    Button("Show More") { showsAll = true }
                        .buttonStyle(.borderless)
                        .foregroundStyle(JetDesign.accentText)
                        .font(.system(size: JetDesign.TextSize.control))
                }
                if status == .failed {
                    Label(
                        String(localized: "Couldn't check for newer Git steps. Showing what Jet saw last."),
                        systemImage: "wifi.slash"
                    )
                    .font(.system(size: JetDesign.TextSize.metadata))
                    .foregroundStyle(.secondary)
                }
            }
        }
    }

    private func placeholder(_ text: String) -> some View {
        Text(text)
            .font(.system(size: JetDesign.TextSize.control))
            .foregroundStyle(.secondary)
    }
}

enum GitActivityRules {
    /// The newest failure that no later attempt of the same step replaced. Only it
    /// offers its fix.
    static func newestUnsupersededFailure(in history: [JetGitDelivery]) -> JetGitDelivery? {
        for (index, delivery) in history.enumerated() where delivery.failureCode != nil {
            let superseded = history[..<index].contains { $0.step == delivery.step }
            if !superseded { return delivery }
        }
        return nil
    }

    /// The 11-point line under a row: automatic, when, and the commit title.
    static func metadata(for delivery: JetGitDelivery, now: Date = .now) -> String {
        var parts: [String] = []
        if delivery.policy.automatic { parts.append(String(localized: "Automatic")) }
        if let milliseconds = DeliveryTime.createdAt(delivery.id) {
            parts.append(JetCopy.relative(ms: milliseconds, now: now))
        }
        if let title = delivery.message?.title, !title.isEmpty, delivery.step == .commit || delivery.step == .draftPullRequest {
            parts.append(title)
        }
        return parts.joined(separator: " · ")
    }
}

private struct GitActivityRow: View {
    let session: DesktopSession
    let ref: ConversationRef
    let delivery: JetGitDelivery
    let history: [JetGitDelivery]
    let showsFix: Bool

    @Environment(\.openURL) private var openURL

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            symbol
                .frame(width: 16)
            VStack(alignment: .leading, spacing: 4) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(GitStepCopy.sentence(for: delivery, history: history))
                        .font(.system(size: JetDesign.TextSize.navigation))
                        .fixedSize(horizontal: false, vertical: true)
                    let metadata = GitActivityRules.metadata(for: delivery)
                    if !metadata.isEmpty {
                        Text(metadata)
                            .font(.system(size: JetDesign.TextSize.metadata))
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    }
                    ForEach(notes, id: \.self) { note in
                        Text(note)
                            .font(.system(size: JetDesign.TextSize.control))
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .accessibilityElement(children: .combine)
                if !actions.isEmpty {
                    HStack(spacing: 8) {
                        ForEach(Array(actions.enumerated()), id: \.offset) { _, action in
                            button(for: action)
                        }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    // Copper text on the bordered fill stays legible in dark mode (L10).
                    .tint(JetDesign.accentText)
                    .padding(.top, 2)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contextMenu { contextMenu }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("git-delivery-\(delivery.id.uuidString.lowercased())")
    }

    @ViewBuilder
    private var symbol: some View {
        switch delivery.outcome {
        case .pending:
            ProgressView().controlSize(.mini)
                .accessibilityLabel(Text("Running"))
        case .completed:
            statusImage("checkmark.circle", .green, label: String(localized: "Done"))
        case .failed:
            statusImage("xmark.octagon.fill", .red, label: String(localized: "Failed"))
        case .outcomeUnknown:
            statusImage(
                "questionmark.circle",
                delivery.acknowledgedBy == nil ? .orange : .secondary,
                label: String(localized: "Couldn't confirm")
            )
        }
    }

    private func statusImage(_ name: String, _ tint: Color, label: String) -> some View {
        Image(systemName: name)
            .symbolRenderingMode(.hierarchical)
            .foregroundStyle(tint)
            .accessibilityLabel(Text(label))
    }

    private var acknowledgement: DeliveryCoordinator.Acknowledgement? {
        session.deliveries.acknowledgement(for: delivery.id)
    }

    private var isAcknowledged: Bool {
        delivery.acknowledgedBy != nil || acknowledgement == .marked
    }

    /// Lines under the sentence: the failure's reason, or what to check.
    private var notes: [String] {
        switch delivery.outcome {
        case let .failed(code):
            return [GitFailureCopy.make(
                code: code,
                step: delivery.step,
                computer: session.deliveryComputerName(ref),
                assistant: session.assistantName(for: ref.conversationID),
                remote: delivery.remoteName ?? "origin"
            ).sentence]
        case .outcomeUnknown:
            if isAcknowledged { return [GitStepCopy.acknowledged] }
            switch acknowledgement {
            case .marking: return [GitStepCopy.followUp(delivery.step), String(localized: "Marking as checked…")]
            case .failed: return [GitStepCopy.followUp(delivery.step), String(localized: "Couldn't mark it as checked.")]
            case .marked, nil: return [GitStepCopy.followUp(delivery.step)]
            }
        case .pending, .completed:
            return []
        }
    }

    private var actions: [KeepChangesSummary.Action] {
        var actions: [KeepChangesSummary.Action] = []
        switch delivery.outcome {
        case .outcomeUnknown where !isAcknowledged:
            switch acknowledgement {
            case .marking: break
            case .failed: actions = [.checkStatus, .retryAcknowledgement(delivery)]
            case .marked, nil: actions = [.checkStatus, .markAsChecked(delivery)]
            }
        case let .failed(code) where showsFix:
            let copy = GitFailureCopy.make(code: code, step: delivery.step, computer: session.deliveryComputerName(ref))
            switch copy.fix {
            case .tryAgain: actions = [.tryAgainDelivery(delivery)]
            case .chooseAnotherName: actions = [.chooseAnotherName]
            case .keepChanges: actions = [.keepChanges]
            case .howToSetUp: actions = [.howToSetUp]
            case .markAsChecked:
                if let unchecked = history.first(where: \.needsAcknowledgement) {
                    actions = [.markAsChecked(unchecked)]
                }
            case nil: break
            }
        case .completed:
            if let url = delivery.pullRequestURL { actions.append(.openPullRequest(url)) }
            if delivery.step == .branch, let branch = delivery.branchName { actions.append(.copyBranchName(branch)) }
        default:
            break
        }
        return actions
    }

    @ViewBuilder
    private func button(for action: KeepChangesSummary.Action) -> some View {
        let offline = action.needsConnection && !session.isDeliveryConnected(ref)
        switch action {
        case .howToSetUp:
            GitHubSetupButton(computer: session.deliveryComputerName(ref))
        case let .copyBranchName(branch):
            DeliveryCopyButton(title: action.title, value: branch)
        case let .openPullRequest(url):
            Button(action.title) { openURL(url) }
        default:
            Button(action.title) { perform(action) }
                .disabled(offline)
                .help(offline ? Text("Not connected to \(session.deliveryComputerName(ref)).") : Text(action.title))
        }
    }

    private func perform(_ action: KeepChangesSummary.Action) {
        let key = session.deliveryKey(for: ref)
        switch action {
        case .checkStatus:
            Task { await session.checkGitStatus(for: ref) }
        case let .markAsChecked(delivery):
            session.requestMarkAsChecked(delivery)
        case let .retryAcknowledgement(delivery):
            Task { await session.confirmMarkAsChecked(delivery, for: key) }
        case let .tryAgainDelivery(delivery):
            session.requestRetry(.retryDelivery(key, delivery))
        case .chooseAnotherName, .keepChanges, .continuePlan:
            session.presentKeepChanges(for: ref)
        case .tryAgainStep, .sendSameRequestAgain, .howToSetUp, .openPullRequest, .copyBranchName:
            break
        }
    }

    @ViewBuilder
    private var contextMenu: some View {
        if let branch = delivery.branchName {
            Button("Copy Branch Name") { DesktopSession.copyToPasteboard(branch) }
        }
        if let commit = delivery.commitID {
            Button("Copy Commit ID") { DesktopSession.copyToPasteboard(commit) }
        }
        if let code = delivery.failureCode {
            Button("Copy Error Code") { DesktopSession.copyToPasteboard(code) }
        }
    }
}
