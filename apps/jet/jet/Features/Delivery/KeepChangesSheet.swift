import Observation
import SwiftUI

/// What the Keep Changes sheet reviews, loaded once when it opens.
struct KeepChangesFacts: Equatable, Sendable {
    var conversationID: UUID
    var title: String
    var projectName: String?
    var assistantName: String?
    var computerName: String
    var isRemote: Bool
    /// The task works directly in the project folder, without a separate working copy.
    var isLocalCheckout: Bool
    var fileCount: Int
    /// The latest reply's changes, which commit and the draft pull request carry.
    var checkpoint: JetGitCheckpoint?
    var branchPrefix: String
    /// The task's Git deliveries, newest first.
    var history: [JetGitDelivery]
}

/// The plans the sheet offers (design §6.9).
enum KeepChangesChoice: String, CaseIterable, Identifiable, Sendable {
    case saveToBranch
    case saveAndPush
    case draftPullRequest

    var id: Self { self }

    var title: String {
        switch self {
        case .saveToBranch: String(localized: "Save to a branch")
        case .saveAndPush: String(localized: "Save and push")
        case .draftPullRequest: String(localized: "Open a draft pull request")
        }
    }

    /// The plan whose last step matches, for remembering the task's last plan.
    init?(steps: [DeliveryCoordinator.Step]) {
        switch steps.last {
        case .branch, .commit: self = .saveToBranch
        case .push: self = .saveAndPush
        case .draftPullRequest: self = .draftPullRequest
        case nil: return nil
        }
    }
}

/// Which Git steps a plan runs, from the task's history: a branch unless one
/// exists, a commit unless these changes were committed, a push unless the
/// branch was pushed after that commit, and the draft pull request always.
enum KeepChangesPlanner {
    typealias Step = DeliveryCoordinator.Step

    static func steps(
        choice: KeepChangesChoice,
        mode: KeepChangesMode,
        checkpoint: JetGitCheckpoint?,
        history: [JetGitDelivery]
    ) -> [Step] {
        if case let .single(single) = mode { return [Step(single)] }
        var steps: [Step] = []
        if existingBranch(in: history) == nil { steps.append(.branch) }
        let commit = committed(checkpoint, in: history)
        let commits = checkpoint != nil && commit == nil
        if commits { steps.append(.commit) }
        if choice != .saveToBranch {
            let pushedAfterCommit = commit.map { commit in
                history.contains {
                    $0.step == .push && $0.outcome.isCompleted
                        && DeliveryCoordinator.isNewer($0.id, than: commit.id)
                }
            } ?? false
            if commits || !pushedAfterCommit { steps.append(.push) }
        }
        if choice == .draftPullRequest { steps.append(.draftPullRequest) }
        return steps
    }

    /// The branch a completed branch step created earlier.
    static func existingBranch(in history: [JetGitDelivery]) -> String? {
        history.first { $0.step == .branch && $0.outcome.isCompleted }?.branchName
    }

    /// The completed commit of exactly these changes.
    static func committed(_ checkpoint: JetGitCheckpoint?, in history: [JetGitDelivery]) -> JetGitDelivery? {
        guard let checkpoint else { return nil }
        return history.first { $0.step == .commit && $0.outcome.isCompleted && $0.checkpoint == checkpoint }
    }

    static func hasCompletedPush(in history: [JetGitDelivery]) -> Bool {
        history.contains { ($0.step == .push || $0.step == .draftPullRequest) && $0.outcome.isCompleted }
    }

    static func hasOpenedPullRequest(in history: [JetGitDelivery]) -> Bool {
        history.contains { $0.step == .draftPullRequest && $0.outcome.isCompleted }
    }
}

/// Branch names: the default from the task title, and validation that matches
/// what jetd accepts.
enum KeepChangesNaming {
    static let maximumSlugLength = 48
    /// jetd refuses longer Git names.
    static let maximumNameBytes = 240

    /// A lowercase ASCII slug of the title, at most 48 characters, cut at a dash.
    static func slug(_ title: String, conversationID: UUID) -> String {
        let latin = title.applyingTransform(.toLatin, reverse: false) ?? title
        let plain = latin.applyingTransform(.stripDiacritics, reverse: false) ?? latin
        var slug = ""
        var needsDash = false
        for scalar in plain.lowercased().unicodeScalars {
            if ("a" ... "z").contains(scalar) || ("0" ... "9").contains(scalar) {
                if needsDash, !slug.isEmpty { slug.append("-") }
                needsDash = false
                slug.unicodeScalars.append(scalar)
            } else {
                needsDash = true
            }
        }
        if slug.count > maximumSlugLength {
            let cut = String(slug.prefix(maximumSlugLength))
            let nextIndex = slug.index(slug.startIndex, offsetBy: maximumSlugLength)
            if slug[nextIndex] == "-" {
                slug = cut
            } else if let dash = cut.lastIndex(of: "-") {
                slug = String(cut[..<dash])
            } else {
                slug = cut
            }
        }
        if slug.isEmpty {
            slug = "task-" + conversationID.uuidString.lowercased().prefix(8)
        }
        return slug
    }

    static func defaultBranch(
        prefix: String,
        title: String,
        conversationID: UUID,
        taken: Set<String> = []
    ) -> String {
        nextAvailable(prefix + slug(title, conversationID: conversationID), taken: taken)
    }

    /// `name`, or `name-2`, `name-3`… when Git already refused it as existing.
    static func nextAvailable(_ name: String, taken: Set<String>) -> String {
        guard taken.contains(name) else { return name }
        var suffix = 2
        while taken.contains("\(name)-\(suffix)") { suffix += 1 }
        return "\(name)-\(suffix)"
    }

    /// Branch names Git refused because they already exist.
    static func takenNames(in history: [JetGitDelivery]) -> Set<String> {
        Set(history.compactMap { delivery in
            guard case let .branch(name) = delivery.operation,
                  delivery.failureCode == "git.branch_exists"
            else { return nil }
            return name
        })
    }

    static func branchIssue(_ name: String) -> String? {
        if name.isEmpty { return String(localized: "Enter a branch name.") }
        if name.contains(where: \.isWhitespace) { return String(localized: "Branch names can't contain spaces.") }
        if name.contains(where: { "~^:?*[\\".contains($0) }) {
            return String(localized: "Branch names can't contain ~ ^ : ? * [ or \\.")
        }
        if !isValidGitName(name) { return String(localized: "This isn't a valid branch name.") }
        return nil
    }

    static func remoteIssue(_ name: String) -> String? {
        if name.isEmpty || name.contains(where: \.isWhitespace) {
            return String(localized: "Enter a remote name without spaces.")
        }
        if !isValidGitName(name) { return String(localized: "This isn't a valid remote name.") }
        return nil
    }

    /// The base branch may stay empty for the repository default.
    static func baseIssue(_ name: String) -> String? {
        name.isEmpty ? nil : branchIssue(name)
    }

    /// Git's ref rules plus jetd's stricter set: ASCII letters, digits, `/ _ - .`.
    static func isValidGitName(_ name: String) -> Bool {
        guard !name.isEmpty, name.utf8.count <= maximumNameBytes else { return false }
        let forbidden = ["..", "@{", "//"]
        if forbidden.contains(where: { name.contains($0) }) { return false }
        if name.hasPrefix("-") || name.hasPrefix("/") || name.hasPrefix(".") { return false }
        if name.hasSuffix("/") || name.hasSuffix(".") || name.hasSuffix(".lock") { return false }
        return name.unicodeScalars.allSatisfy { scalar in
            scalar.isASCII && (scalar.properties.isAlphabetic || ("0" ... "9").contains(scalar) || "/_-.".unicodeScalars.contains(scalar))
        }
    }
}

/// Sheet copy that depends on the facts.
enum KeepChangesSheetCopy {
    static func subtitle(_ facts: KeepChangesFacts) -> AttributedString? {
        guard facts.fileCount > 0 else { return nil }
        let who = facts.assistantName ?? String(localized: "The assistant")
        let project = facts.projectName ?? String(localized: "project")
        let computer = facts.computerName
        let count = facts.fileCount
        switch (facts.isLocalCheckout, facts.isRemote) {
        case (true, false):
            return AttributedString(localized: "\(who) changed ^[\(count) file](inflect: true) in your \(project) folder.")
        case (true, true):
            return AttributedString(localized: "\(who) changed ^[\(count) file](inflect: true) in your \(project) folder on \(computer).")
        case (false, false):
            return AttributedString(localized: "\(who) changed ^[\(count) file](inflect: true) in a separate working copy. Your \(project) folder hasn't changed.")
        case (false, true):
            return AttributedString(localized: "\(who) changed ^[\(count) file](inflect: true) in a separate working copy. Your \(project) folder on \(computer) hasn't changed.")
        }
    }

    static func caption(
        _ choice: KeepChangesChoice,
        facts: KeepChangesFacts,
        existingBranch: String?,
        remote: String
    ) -> String {
        let project = facts.projectName ?? String(localized: "your project")
        switch choice {
        case .saveToBranch:
            if let existingBranch {
                return String(localized: "Commits the changes to \(existingBranch) in \(project).")
            }
            return facts.isLocalCheckout
                ? String(localized: "Commits the changes to a new branch in \(project) and switches \(project) to it.")
                : String(localized: "Commits the changes to a new branch in \(project). Your checked-out files don't change.")
        case .saveAndPush:
            return String(localized: "Also pushes the branch to \(remote).")
        case .draftPullRequest:
            return String(localized: "Also opens a draft pull request on GitHub.")
        }
    }
}

/// The sheet's state: the loaded facts and the person's choices.
@MainActor
@Observable
final class KeepChangesModel {
    enum Load: Equatable {
        case loading
        case loaded(KeepChangesFacts)
        case failed
    }

    enum EmptyState: Equatable {
        case noChanges
        case alreadyKept

        var text: String {
            switch self {
            case .noChanges: String(localized: "No changes to keep yet.")
            case .alreadyKept: String(localized: "These changes are already kept.")
            }
        }
    }

    typealias Step = DeliveryCoordinator.Step

    let mode: KeepChangesMode
    var load: Load = .loading
    var choice: KeepChangesChoice = .saveToBranch
    var branchName = ""
    var remote = "origin"
    var baseBranch = ""
    var showsOptions = false
    /// Seeded for previews: never loads.
    var isSeeded = false

    init(mode: KeepChangesMode) {
        self.mode = mode
    }

    var facts: KeepChangesFacts? {
        if case let .loaded(facts) = load { return facts }
        return nil
    }

    /// Fills the defaults from the facts and the task's last plan.
    func apply(_ facts: KeepChangesFacts, lastPlan: DeliveryCoordinator.Plan? = nil) {
        load = .loaded(facts)
        if case .plan = mode, let lastPlan, let last = KeepChangesChoice(steps: lastPlan.steps) {
            choice = last
        }
        if let lastPlan, KeepChangesNaming.remoteIssue(lastPlan.remote) == nil {
            remote = lastPlan.remote
        }
        if let base = lastPlan?.baseBranch { baseBranch = base }
        branchName = KeepChangesNaming.defaultBranch(
            prefix: facts.branchPrefix,
            title: facts.title,
            conversationID: facts.conversationID,
            taken: KeepChangesNaming.takenNames(in: facts.history)
        )
    }

    var existingBranch: String? {
        facts.flatMap { KeepChangesPlanner.existingBranch(in: $0.history) }
    }

    var steps: [Step] {
        guard let facts else { return [] }
        return KeepChangesPlanner.steps(
            choice: choice,
            mode: mode,
            checkpoint: facts.checkpoint,
            history: facts.history
        )
    }

    var updatesPullRequest: Bool {
        facts.map { KeepChangesPlanner.hasOpenedPullRequest(in: $0.history) } ?? false
    }

    var branchIssue: String? {
        steps.contains(.branch) ? KeepChangesNaming.branchIssue(branchName) : nil
    }

    var remoteIssue: String? {
        steps.contains { $0 == .push || $0 == .draftPullRequest } ? KeepChangesNaming.remoteIssue(remote) : nil
    }

    var baseIssue: String? {
        steps.contains(.draftPullRequest) ? KeepChangesNaming.baseIssue(baseBranch) : nil
    }

    var showsOptionFields: Bool {
        steps.contains { $0 == .push || $0 == .draftPullRequest }
    }

    var emptyState: EmptyState? {
        guard let facts else { return nil }
        let needsChanges: Bool = switch mode {
        case .plan: true
        case let .single(choice): choice == .commit || choice == .draftPullRequest
        }
        if needsChanges, facts.fileCount == 0 || facts.checkpoint == nil { return .noChanges }
        if steps.isEmpty { return .alreadyKept }
        return nil
    }

    /// A single step that likely fails because an earlier step hasn't run.
    var singleStepWarning: String? {
        guard case let .single(choice) = mode, let facts else { return nil }
        switch choice {
        case .push where existingBranch == nil:
            return String(localized: "This task has no branch yet. Create one first.")
        case .draftPullRequest where !KeepChangesPlanner.hasCompletedPush(in: facts.history):
            return String(localized: "Push the branch first.")
        default:
            return nil
        }
    }

    var canConfirm: Bool {
        facts != nil && emptyState == nil && !steps.isEmpty
            && branchIssue == nil && remoteIssue == nil && baseIssue == nil
    }

    var sheetTitle: String {
        switch mode {
        case .plan: String(localized: "Keep Changes")
        case .single(.branch): String(localized: "Create Branch")
        case .single(.commit): String(localized: "Commit Changes")
        case .single(.push): String(localized: "Push Branch")
        case .single(.draftPullRequest): String(localized: "Open Draft Pull Request")
        }
    }

    var primaryTitle: String {
        switch mode {
        case .single(.branch): return String(localized: "Create Branch")
        case .single(.commit): return String(localized: "Commit")
        case .single(.push): return String(localized: "Push")
        case .single(.draftPullRequest): return String(localized: "Open Pull Request")
        case .plan: break
        }
        let last: Step = steps.last ?? {
            switch choice {
            case .saveToBranch: .commit
            case .saveAndPush: .push
            case .draftPullRequest: .draftPullRequest
            }
        }()
        switch last {
        case .branch, .commit: return String(localized: "Save to Branch")
        case .push: return String(localized: "Save and Push")
        case .draftPullRequest:
            return updatesPullRequest ? String(localized: "Update Pull Request") : String(localized: "Create Pull Request")
        }
    }

    /// The branch the plan works on: the new one, or the one created earlier.
    var planBranch: String {
        steps.contains(.branch) ? branchName : existingBranch ?? branchName
    }

    func makePlan() -> DeliveryCoordinator.Plan? {
        guard canConfirm, let facts else { return nil }
        return DeliveryCoordinator.Plan(
            steps: steps,
            branchName: planBranch,
            remote: remote,
            baseBranch: baseBranch.isEmpty ? nil : baseBranch,
            checkpoint: facts.checkpoint
        )
    }

    func stepTitle(_ step: Step) -> String {
        GitStepCopy.title(step, branch: planBranch, remote: remote, updatesPullRequest: updatesPullRequest)
    }
}

/// Keep Changes…: one review of the whole plan, then each Git step runs as its own
/// Command (design §6.9, decision 1). Confirming closes the sheet; progress shows
/// in the Keep status line.
struct KeepChangesSheet: View {
    @Bindable var session: DesktopSession
    let ref: ConversationRef
    let mode: KeepChangesMode

    @State private var model: KeepChangesModel

    init(
        session: DesktopSession,
        ref: ConversationRef,
        mode: KeepChangesMode,
        seed: (@MainActor (KeepChangesModel) -> Void)? = nil
    ) {
        self.session = session
        self.ref = ref
        self.mode = mode
        let model = KeepChangesModel(mode: mode)
        if let seed {
            seed(model)
            model.isSeeded = true
        }
        _model = State(initialValue: model)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            header
            if let progress = runningProgress {
                runningContent(progress)
            } else {
                notices
                content
            }
            footer
        }
        .font(.system(size: JetDesign.TextSize.navigation))
        .padding(20)
        .frame(width: 520, alignment: .leading)
        .task { await loadIfNeeded() }
    }

    // MARK: - Header and notices

    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(model.sheetTitle)
                .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
            if let subtitle = model.facts.flatMap(KeepChangesSheetCopy.subtitle) {
                Text(subtitle)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private var notices: some View {
        if let warning = model.singleStepWarning {
            InlineNotice(notice: ComposerNotice(kind: .warning, text: warning)) { _ in }
        }
        ForEach(blockingNotices, id: \.text) { notice in
            InlineNotice(notice: notice) { _ in
                session.dismissSheet()
                session.showDetails(.run)
            }
        }
    }

    /// Reasons Jet can't start Git steps now. Each disables the primary button.
    private var blockingNotices: [ComposerNotice] {
        var notices: [ComposerNotice] = []
        if ref.conversationID == session.selectedConversationID,
           let reason = session.keepChangesUnavailableReason
        {
            notices.append(ComposerNotice(kind: .warning, text: reason))
        }
        if session.deliveries.hasUncertainStep(for: session.deliveryKey(for: ref)) {
            notices.append(ComposerNotice(
                kind: .warning,
                text: String(localized: "Jet couldn't confirm it received the last Git request. Check its status first.")
            ))
        } else if session.uncheckedGitDelivery(for: ref) != nil {
            notices.append(ComposerNotice(
                kind: .warning,
                text: String(localized: "Jet paused Git steps for this task until you check the last one."),
                action: .reviewGitStep
            ))
        } else if session.deliveryHistory(for: ref).contains(where: { $0.outcome == .pending }) {
            notices.append(ComposerNotice(
                kind: .warning,
                text: String(localized: "Jet is still finishing a Git step for this task.")
            ))
        }
        return notices
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        switch model.load {
        case .loading:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Checking the changes…").foregroundStyle(.secondary)
            }
        case .failed:
            InlineNotice(
                notice: ComposerNotice(
                    kind: .error,
                    text: String(localized: "Couldn't check the changes."),
                    action: .tryAgainConnection
                )
            ) { _ in
                Task { await load() }
            }
        case let .loaded(facts):
            if model.emptyState == .noChanges {
                Text(KeepChangesModel.EmptyState.noChanges.text)
                    .foregroundStyle(.secondary)
            } else {
                loadedContent(facts)
            }
        }
    }

    @ViewBuilder
    private func loadedContent(_ facts: KeepChangesFacts) -> some View {
        if case .plan = mode {
            planPicker(facts)
        }
        if model.steps.contains(.draftPullRequest) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "key")
                    .foregroundStyle(.secondary)
                    .accessibilityHidden(true)
                Text("Needs a GitHub token saved for Jet in your Keychain.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 8)
                GitHubSetupButton(computer: facts.computerName)
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
            .font(.system(size: JetDesign.TextSize.control))
        }
        branchRow
        if model.showsOptionFields {
            options
        }
        if model.emptyState == .alreadyKept {
            Text(KeepChangesModel.EmptyState.alreadyKept.text)
                .foregroundStyle(.secondary)
        } else {
            stepList(model.steps)
        }
    }

    private func planPicker(_ facts: KeepChangesFacts) -> some View {
        SheetRow("Plan") {
            plans(facts)
        }
    }

    private func plans(_ facts: KeepChangesFacts) -> some View {
        Picker("Plan", selection: $model.choice) {
            ForEach(KeepChangesChoice.allCases) { choice in
                VStack(alignment: .leading, spacing: 2) {
                    Text(choice.title)
                    Text(KeepChangesSheetCopy.caption(
                        choice,
                        facts: facts,
                        existingBranch: model.existingBranch,
                        remote: model.remote
                    ))
                    .font(.system(size: JetDesign.TextSize.metadata))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.bottom, 4)
                .tag(choice)
            }
        }
#if os(macOS)
        .pickerStyle(.radioGroup)
#else
        .pickerStyle(.inline)
#endif
        .labelsHidden()
        .accessibilityIdentifier("keep-changes-plan")
    }

    @ViewBuilder
    private var branchRow: some View {
        if model.steps.contains(.branch) {
            SheetRow("Branch") {
                VStack(alignment: .leading, spacing: 4) {
                    TextField("Branch", text: $model.branchName)
                        .labelsHidden()
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: JetDesign.TextSize.control, design: .monospaced))
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("keep-changes-branch")
                    if let issue = model.branchIssue {
                        issueLine(issue)
                    }
                }
            }
        } else if let existing = model.existingBranch {
            SheetRow("Branch") {
                Text("Uses branch \(existing) created earlier.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var options: some View {
        DisclosureGroup("Options", isExpanded: optionsExpanded) {
            VStack(alignment: .leading, spacing: 8) {
                SheetRow("Remote") {
                    VStack(alignment: .leading, spacing: 4) {
                        TextField("Remote", text: $model.remote)
                            .labelsHidden()
                            .textFieldStyle(.roundedBorder)
                            .font(.system(size: JetDesign.TextSize.control, design: .monospaced))
                            .autocorrectionDisabled()
                        if let issue = model.remoteIssue { issueLine(issue) }
                    }
                }
                if model.steps.contains(.draftPullRequest) {
                    SheetRow("Base branch") {
                        VStack(alignment: .leading, spacing: 4) {
                            TextField("Base branch", text: $model.baseBranch, prompt: Text("Repository default"))
                                .labelsHidden()
                                .textFieldStyle(.roundedBorder)
                                .font(.system(size: JetDesign.TextSize.control, design: .monospaced))
                                .autocorrectionDisabled()
                            if let issue = model.baseIssue { issueLine(issue) }
                        }
                    }
                }
            }
            .padding(.top, 8)
        }
    }

    /// Options open by themselves while one of their fields has a problem.
    private var optionsExpanded: Binding<Bool> {
        Binding(
            get: { model.showsOptions || model.remoteIssue != nil || model.baseIssue != nil },
            set: { model.showsOptions = $0 }
        )
    }

    private func issueLine(_ text: String) -> some View {
        Label(text, systemImage: "exclamationmark.circle")
            .font(.system(size: JetDesign.TextSize.metadata))
            .foregroundStyle(.red)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func stepList(_ steps: [DeliveryCoordinator.Step]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Steps")
                .font(.system(size: JetDesign.TextSize.navigation, weight: .semibold))
            ForEach(Array(steps.enumerated()), id: \.element) { index, step in
                Text("\(index + 1). \(model.stepTitle(step))")
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("keep-changes-steps")
    }

    // MARK: - Running

    /// This task's plan while it runs: the sheet then only shows progress.
    private var runningProgress: DeliveryCoordinator.Progress? {
        guard let progress = session.deliveries.progress(for: session.deliveryKey(for: ref)),
              progress.isRunning
        else { return nil }
        return progress
    }

    @ViewBuilder
    private func runningContent(_ progress: DeliveryCoordinator.Progress) -> some View {
        InlineNotice(notice: ComposerNotice(
            kind: .info,
            text: String(localized: "Jet is still finishing a Git step for this task.")
        )) { _ in }
        VStack(alignment: .leading, spacing: 6) {
            Text("Steps")
                .font(.system(size: JetDesign.TextSize.navigation, weight: .semibold))
            ForEach(Array(progress.plan.steps.enumerated()), id: \.element) { index, step in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("\(index + 1). \(GitStepCopy.title(step, branch: progress.plan.branchName, remote: progress.plan.remote))")
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 8)
                    StepStateLabel(state: progress.state(of: step))
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("keep-changes-steps")
    }

    // MARK: - Footer

    private var footer: some View {
        HStack(spacing: 8) {
            Spacer()
            if runningProgress != nil {
                Button("Done") { session.dismissSheet() }
                    .keyboardShortcut(.defaultAction)
            } else {
                Button("Cancel", role: .cancel) { session.dismissSheet() }
                    .keyboardShortcut(.cancelAction)
                Button(model.primaryTitle) { confirm() }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!model.canConfirm || !blockingNotices.isEmpty)
                    .accessibilityIdentifier("git-delivery-review")
            }
        }
        .padding(.top, 4)
    }

    private func confirm() {
        guard blockingNotices.isEmpty, let plan = model.makePlan() else { return }
        session.startKeepChanges(plan, for: ref)
    }

    // MARK: - Loading

    private func loadIfNeeded() async {
        guard !model.isSeeded, case .loading = model.load else { return }
        await load()
    }

    private func load() async {
        model.load = .loading
        do {
            let facts = try await session.loadKeepChangesFacts(for: ref)
            model.apply(facts, lastPlan: session.deliveries.progress(for: session.deliveryKey(for: ref))?.plan)
        } catch {
            model.load = .failed
        }
    }
}

/// A field row with the sheet's shared label column, so every field lines up and
/// each label sits on its field's first baseline.
private struct SheetRow<Content: View>: View {
    let label: LocalizedStringKey
    let content: Content

    @ScaledMetric(relativeTo: .body) private var labelWidth: CGFloat = 88

    init(_ label: LocalizedStringKey, @ViewBuilder content: () -> Content) {
        self.label = label
        self.content = content()
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(label)
                .frame(width: labelWidth, alignment: .leading)
            content
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// A step's state while a plan runs: a symbol or spinner plus the word.
struct StepStateLabel: View {
    let state: DeliveryCoordinator.StepState

    var body: some View {
        HStack(spacing: 4) {
            switch state {
            case .submitting, .pending:
                ProgressView().controlSize(.mini)
            default:
                Image(systemName: symbol)
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(tint)
            }
            Text(GitStepCopy.stateLabel(state))
                .foregroundStyle(.secondary)
        }
        .font(.system(size: JetDesign.TextSize.metadata))
        .accessibilityElement(children: .combine)
    }

    private var symbol: String {
        switch state {
        case .notStarted: "circle.dashed"
        case .submitting, .pending: "circle"
        case .completed: "checkmark.circle"
        case .failed: "xmark.octagon.fill"
        case .unconfirmed, .admissionUncertain: "questionmark.circle"
        }
    }

    private var tint: Color {
        switch state {
        case .notStarted, .submitting, .pending: .secondary
        case .completed: .green
        case .failed: .red
        case .unconfirmed, .admissionUncertain: .orange
        }
    }
}

extension JetGitDeliveryOutcome {
    var isCompleted: Bool {
        if case .completed = self { return true }
        return false
    }
}
