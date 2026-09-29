import SwiftUI

/// What the Keep status line says about a task's Git steps: the running plan, or
/// else the newest recorded delivery. Nil when there is nothing to report.
struct KeepChangesSummary: Equatable {
    typealias Step = DeliveryCoordinator.Step

    enum Tone: Equatable, Sendable {
        case running
        case success
        case warning
        case failure
        case neutral
    }

    enum Action: Equatable {
        case checkStatus
        case markAsChecked(JetGitDelivery)
        /// Sends the same acknowledgement Command again after it failed.
        case retryAcknowledgement(JetGitDelivery)
        case sendSameRequestAgain
        case tryAgainStep(Step)
        case tryAgainDelivery(JetGitDelivery)
        case chooseAnotherName
        case keepChanges
        case continuePlan
        case howToSetUp
        case openPullRequest(URL)
        case copyBranchName(String)

        var title: String {
            switch self {
            case .checkStatus: String(localized: "Check Status")
            case .markAsChecked: String(localized: "Mark as Checked…")
            case .retryAcknowledgement: String(localized: "Try Again")
            case .sendSameRequestAgain: String(localized: "Send Same Request Again")
            case .tryAgainStep, .tryAgainDelivery: String(localized: "Try Again…")
            case .chooseAnotherName: String(localized: "Choose Another Name…")
            case .keepChanges: String(localized: "Keep Changes…")
            case .continuePlan: String(localized: "Continue…")
            case .howToSetUp: String(localized: "How to Set Up…")
            case .openPullRequest: String(localized: "Open Pull Request")
            case .copyBranchName: String(localized: "Copy Branch Name")
            }
        }

        /// Actions that reach the computer are disabled while it is offline.
        var needsConnection: Bool {
            switch self {
            case .howToSetUp, .openPullRequest, .copyBranchName: false
            default: true
            }
        }
    }

    struct Context: Equatable {
        var computer: String
        var assistant: String?
        /// The task works directly in the project folder: no `git switch` line.
        var isLocalCheckout: Bool
        var acknowledgements: [UUID: DeliveryCoordinator.Acknowledgement] = [:]
    }

    var tone: Tone
    /// Nil shows a spinner.
    var systemImage: String?
    var headline: String
    var details: [String] = []
    /// The branch named in the headline, shown in monospace and offered for copying.
    var branch: String?
    var pullRequestURL: URL?
    /// "git switch <branch>", shown with Copy when the branch lives outside the project folder.
    var switchCommand: String?
    var actions: [Action] = []

    static func make(
        progress: DeliveryCoordinator.Progress?,
        history: [JetGitDelivery],
        context: Context
    ) -> KeepChangesSummary? {
        if let progress, progress.isRunning || progress.hasStarted {
            return summary(of: progress, history: history, context: context)
        }
        guard let newest = history.first else { return nil }
        return summary(of: newest, history: history, context: context)
    }

    // MARK: - From a plan

    private static func summary(
        of progress: DeliveryCoordinator.Progress,
        history: [JetGitDelivery],
        context: Context
    ) -> KeepChangesSummary {
        let plan = progress.plan
        let remote = plan.remote
        let branch = plan.branchName.isEmpty ? DeliveryCoordinator.knownBranch(in: history) : plan.branchName

        if progress.isRunning || progress.currentStep != nil {
            let step = progress.currentStep ?? progress.notStartedSteps.first ?? plan.steps.first ?? .branch
            var details: [String] = []
            if plan.steps.count > 1, let index = plan.steps.firstIndex(of: step) {
                details.append(String(localized: "Step \(index + 1) of \(plan.steps.count)"))
            }
            if progress.lastCheckFailed {
                details.append(String(localized: "Can't reach \(context.computer). Jet keeps checking."))
            }
            return KeepChangesSummary(
                tone: .running,
                systemImage: nil,
                headline: GitStepCopy.running(step, branch: branch, remote: remote),
                details: details,
                branch: step == .branch ? branch : nil
            )
        }

        if let step = progress.stoppedStep {
            switch progress.state(of: step) {
            case let .failed(code, _):
                return failure(
                    step: step,
                    code: code,
                    remote: remote,
                    retry: .tryAgainStep(step),
                    history: history,
                    context: context
                )
            case let .unconfirmed(delivery):
                return unconfirmed(step: step, delivery: delivery, continues: !progress.notStartedSteps.isEmpty, context: context)
            case .admissionUncertain:
                return KeepChangesSummary(
                    tone: .warning,
                    systemImage: "questionmark.circle",
                    headline: GitStepCopy.admissionUncertain,
                    details: [GitStepCopy.title(step, branch: branch, remote: remote)],
                    actions: [.checkStatus, .sendSameRequestAgain]
                )
            default:
                break
            }
        }

        if let next = progress.notStartedSteps.first {
            let finished = plan.steps.last { progress.state(of: $0).isCompleted }
            let details = finished.map { step -> [String] in
                let delivery = delivery(in: progress.state(of: step))
                return [GitStepCopy.done(
                    step,
                    branch: delivery?.branchName ?? branch,
                    remote: remote,
                    reply: delivery?.checkpoint?.turn ?? plan.checkpoint?.turn
                )]
            } ?? []
            return KeepChangesSummary(
                tone: .neutral,
                systemImage: "pause.circle",
                headline: String(localized: "\(GitStepCopy.title(next, branch: branch, remote: remote)) · Not started"),
                details: details,
                actions: [.continuePlan]
            )
        }

        let deliveries = plan.steps.compactMap { delivery(in: progress.state(of: $0)) }
        let createdBranch = deliveries.last?.branchName ?? branch
        let draft = deliveries.first { $0.step == .draftPullRequest }
        return completed(
            onlyBranch: plan.steps == [.branch],
            pushed: plan.steps.contains(.push) || plan.steps.contains(.draftPullRequest),
            draft: draft,
            branch: createdBranch,
            remote: remote,
            fallback: plan.steps.last.map {
                GitStepCopy.done($0, branch: createdBranch, remote: remote, reply: plan.checkpoint?.turn)
            } ?? "",
            context: context
        )
    }

    private static func delivery(in state: DeliveryCoordinator.StepState) -> JetGitDelivery? {
        switch state {
        case let .completed(delivery), let .unconfirmed(delivery): delivery
        case let .failed(_, delivery): delivery
        case .notStarted, .submitting, .pending, .admissionUncertain: nil
        }
    }

    // MARK: - From history

    private static func summary(
        of delivery: JetGitDelivery,
        history: [JetGitDelivery],
        context: Context
    ) -> KeepChangesSummary {
        let step = delivery.step
        let remote = delivery.remoteName ?? "origin"
        let branch = delivery.branchName ?? DeliveryCoordinator.knownBranch(in: history)
        switch delivery.outcome {
        case .pending:
            return KeepChangesSummary(
                tone: .running,
                systemImage: nil,
                headline: GitStepCopy.running(step, branch: branch, remote: remote),
                branch: step == .branch ? branch : nil
            )
        case .completed:
            return completed(
                onlyBranch: step == .branch,
                pushed: step == .push || step == .draftPullRequest,
                draft: step == .draftPullRequest ? delivery : nil,
                branch: branch,
                remote: remote,
                fallback: GitStepCopy.done(step, branch: branch, remote: remote, reply: delivery.checkpoint?.turn),
                context: context
            )
        case let .failed(code):
            return failure(
                step: step,
                code: code,
                remote: remote,
                retry: .tryAgainDelivery(delivery),
                history: history,
                context: context
            )
        case .outcomeUnknown:
            return unconfirmed(step: step, delivery: delivery, continues: false, context: context)
        }
    }

    // MARK: - Shared states

    private static func completed(
        onlyBranch: Bool,
        pushed: Bool,
        draft: JetGitDelivery?,
        branch: String?,
        remote: String,
        fallback: String,
        context: Context
    ) -> KeepChangesSummary {
        var summary = KeepChangesSummary(tone: .success, systemImage: "checkmark.circle", headline: fallback)
        if let branch {
            summary.headline = onlyBranch
                ? String(localized: "Created branch \(branch)")
                : String(localized: "Saved to branch \(branch)")
            summary.branch = branch
            summary.actions.append(.copyBranchName(branch))
        }
        if pushed {
            summary.details.append(String(localized: "Pushed to \(remote)"))
        }
        if let draft {
            summary.details.append(String(localized: "Draft pull request opened"))
            if let url = draft.pullRequestURL {
                summary.pullRequestURL = url
                summary.actions.insert(.openPullRequest(url), at: 0)
            }
        } else if let branch, !context.isLocalCheckout {
            summary.switchCommand = "git switch \(branch)"
        }
        return summary
    }

    private static func failure(
        step: Step,
        code: String,
        remote: String,
        retry: Action,
        history: [JetGitDelivery],
        context: Context
    ) -> KeepChangesSummary {
        let copy = GitFailureCopy.make(
            code: code,
            step: step,
            computer: context.computer,
            assistant: context.assistant,
            remote: remote
        )
        var actions: [Action] = []
        switch copy.fix {
        case .tryAgain: actions.append(retry)
        case .chooseAnotherName: actions.append(.chooseAnotherName)
        case .keepChanges: actions.append(.keepChanges)
        case .howToSetUp: actions.append(.howToSetUp)
        case .markAsChecked:
            if let unchecked = history.first(where: \.needsAcknowledgement) {
                actions.append(.markAsChecked(unchecked))
            } else {
                actions.append(.checkStatus)
            }
        case nil: break
        }
        return KeepChangesSummary(
            tone: .failure,
            systemImage: "xmark.octagon.fill",
            headline: GitStepCopy.failed(step, remote: remote),
            details: [copy.sentence],
            actions: actions
        )
    }

    private static func unconfirmed(
        step: Step,
        delivery: JetGitDelivery?,
        continues: Bool,
        context: Context
    ) -> KeepChangesSummary {
        let acknowledgement = delivery.flatMap { context.acknowledgements[$0.id] }
        if delivery?.acknowledgedBy != nil || acknowledgement == .marked {
            return KeepChangesSummary(
                tone: .neutral,
                systemImage: "questionmark.circle",
                headline: GitStepCopy.couldNotConfirm(step),
                details: [GitStepCopy.acknowledged],
                actions: continues ? [.continuePlan] : []
            )
        }
        var details = [GitStepCopy.followUp(step)]
        var actions: [Action] = [.checkStatus]
        switch acknowledgement {
        case .marking:
            details.append(String(localized: "Marking as checked…"))
            actions = []
        case .failed:
            details.append(String(localized: "Couldn't mark it as checked."))
            if let delivery { actions.append(.retryAcknowledgement(delivery)) }
        case .marked, nil:
            if let delivery { actions.append(.markAsChecked(delivery)) }
        }
        return KeepChangesSummary(
            tone: .warning,
            systemImage: "questionmark.circle",
            headline: GitStepCopy.couldNotConfirm(step),
            details: details,
            actions: actions
        )
    }
}

extension KeepChangesSummary.Tone {
    var tint: Color {
        switch self {
        case .running: JetDesign.accentText
        case .success: .green
        case .warning: .orange
        case .failure: .red
        case .neutral: .secondary
        }
    }
}

/// The Keep status line under the transcript and in the Details footer: progress,
/// the outcome with its next step, or nothing.
struct KeepChangesStatusLine: View {
    @Bindable var session: DesktopSession
    let ref: ConversationRef

    var body: some View {
        if let summary = session.keepChangesSummary(for: ref) {
            KeepChangesStatusContent(session: session, ref: ref, summary: summary)
                .task(id: ref) { await session.observeDeliveries(for: ref) }
                .onChange(of: session.gitDeliveries) { _, deliveries in
                    guard ref.conversationID == session.selectedConversationID else { return }
                    session.deliveries.absorb(deliveries, for: session.deliveryKey(for: ref))
                }
        }
    }
}

private struct KeepChangesStatusContent: View {
    let session: DesktopSession
    let ref: ConversationRef
    let summary: KeepChangesSummary

    @Environment(\.openURL) private var openURL

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: 16) {
                message
                Spacer(minLength: 12)
                actions
            }
            VStack(alignment: .leading, spacing: 8) {
                message
                actions
            }
        }
        .frame(maxWidth: JetDesign.readingWidth, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("keep-changes-status")
    }

    private var message: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                icon
                VStack(alignment: .leading, spacing: 3) {
                    Text(styledHeadline)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .font(.system(size: JetDesign.TextSize.navigation, weight: .medium))
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    ForEach(summary.details, id: \.self) { detail in
                        Text(detail)
                            .font(.system(size: JetDesign.TextSize.metadata))
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel(Text("Keep Changes: \(summary.headline)"))
            .accessibilityValue(Text(summary.details.joined(separator: "\n")))

            if let command = summary.switchCommand {
                // One line when it fits; otherwise the command gets its own line.
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        switchLabel
                        switchCommand(command)
                    }
                    VStack(alignment: .leading, spacing: 2) {
                        switchLabel
                        switchCommand(command)
                    }
                }
                .padding(.leading, 24)
            }
        }
    }

    private var switchLabel: some View {
        Text("To use it in your project:")
            .font(.system(size: JetDesign.TextSize.metadata))
            .foregroundStyle(.secondary)
    }

    private func switchCommand(_ command: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(command)
                .font(.system(size: JetDesign.TextSize.control, design: .monospaced))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            DeliveryCopyButton(title: String(localized: "Copy Command"), value: command, iconOnly: true)
                .buttonStyle(.borderless)
                .controlSize(.small)
                .tint(JetDesign.accentText)
        }
    }

    @ViewBuilder
    private var icon: some View {
        if let symbol = summary.systemImage {
            Image(systemName: symbol)
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(summary.tone.tint)
                .frame(width: 16)
                .accessibilityHidden(true)
        } else {
            ProgressView()
                .controlSize(.small)
                .frame(width: 16, height: 16)
                .accessibilityHidden(true)
        }
    }

    /// The headline with its branch name in monospace, keeping one localized sentence.
    private var styledHeadline: AttributedString {
        var text = AttributedString(summary.headline)
        if let branch = summary.branch, let range = text.range(of: branch) {
            text[range].font = .system(size: JetDesign.TextSize.control, design: .monospaced)
        }
        return text
    }

    @ViewBuilder
    private var actions: some View {
        if !summary.actions.isEmpty {
            HStack(spacing: 8) {
                ForEach(Array(summary.actions.enumerated()), id: \.offset) { _, action in
                    button(for: action)
                }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            // Copper text on the bordered fill stays legible in dark mode (L10).
            .tint(JetDesign.accentText)
            .frame(minHeight: 28)
            .fixedSize()
        }
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
        case .sendSameRequestAgain:
            session.deliveries.resendUncertain(for: key)
        case let .tryAgainStep(step):
            session.requestRetry(.retryStep(key, step))
        case let .tryAgainDelivery(delivery):
            session.requestRetry(.retryDelivery(key, delivery))
        case .chooseAnotherName, .keepChanges, .continuePlan:
            session.presentKeepChanges(for: ref)
        case .howToSetUp, .openPullRequest, .copyBranchName:
            break
        }
    }
}
