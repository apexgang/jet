import SwiftUI

/// Settings › Tasks › Clean Up Idle Tasks. Jet manages one rule with a fixed
/// prompt; every other rule stays under Custom Rules with today's controls.
struct JetAutodeleteSection: View {
    @Bindable var model: JetRecoveryModel
    let computerName: String
    let graceDays: UInt32
    let taskTitle: (UUID) -> String

    @State private var sheet: CleanUpSheet.Start?
    @State private var confirmsTurnOff = false

    // ASVS 2.3.1: rule changes resume only after the computer can serve writes.
    private var isReadOnly: Bool { model.isReadOnly }

    var body: some View {
        Section {
            summary
            CustomCleanUpRules(model: model, computerName: computerName, isReadOnly: isReadOnly)
        } header: {
            Text("Clean Up Idle Tasks")
        }
        .sheet(item: $sheet) { start in
            CleanUpSheet(
                model: model,
                start: start,
                graceDays: graceDays,
                computerName: computerName,
                taskTitle: taskTitle
            )
        }
        .confirmationDialog(
            "Turn off clean up?",
            isPresented: $confirmsTurnOff,
            titleVisibility: .visible
        ) {
            Button("Turn Off", role: .destructive) {
                guard let rule = model.managedCleanUpRule else { return }
                Task { await model.deleteRule(rule) }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Tasks it already moved stay in Jet Trash until they're removed.")
        }
    }

    @ViewBuilder
    private var summary: some View {
        if model.issues[.autodelete]?.code == "protocol.feature_unavailable" {
            SettingsStatusLabel(
                text: String(localized: "Clean up isn't available on \(computerName). Update Jet there."),
                systemImage: "exclamationmark.circle"
            )
            .foregroundStyle(.secondary)
        } else if model.autodelete == nil {
            if model.isLoading {
                Text("Loading…").foregroundStyle(.secondary)
            } else if let issue = model.issues[.autodelete] {
                SettingsIssueRow(error: issue, sentence: String(localized: "Couldn't load clean up."))
            }
        } else if let rule = model.managedCleanUpRule {
            switch rule.state {
            case let .approved(days, _):
                row {
                    SettingsStatusLabel(
                        text: String(localized: "On · after \(Int(days)) days without activity"),
                        systemImage: "checkmark.circle.fill",
                        tint: .green
                    )
                } actions: {
                    Button("Change…") { sheet = .choose(days: days) }
                    Button("Turn Off…") { confirmsTurnOff = true }
                }
            case .draft, .refused, .compiling:
                row {
                    SettingsStatusLabel(
                        text: String(localized: "Clean up is paused until you review it."),
                        systemImage: "pause.circle",
                        tint: .orange
                    )
                } actions: {
                    Button("Review…") { sheet = CleanUpSheet.Start(reviewing: rule) }
                }
            }
            issueRow
        } else {
            row {
                Text("Move tasks you haven't used for a while to Jet Trash.")
                    .fixedSize(horizontal: false, vertical: true)
            } actions: {
                Button("Set Up…") { sheet = .choose(days: 30) }
                    .accessibilityIdentifier("cleanup-setup")
            }
            issueRow
        }
    }

    @ViewBuilder
    private var issueRow: some View {
        if let issue = model.issues[.autodelete] {
            SettingsIssueRow(error: issue, sentence: issue.category == .outcomeUnknown
                ? String(localized: "Jet couldn't confirm this change.")
                : String(localized: "Couldn't update clean up."))
        }
    }

    private func row<Content: View, Actions: View>(
        @ViewBuilder _ label: () -> Content,
        @ViewBuilder actions: () -> Actions
    ) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            label()
            Spacer(minLength: 8)
            actions()
                .disabled(isReadOnly || model.operation != nil)
        }
    }
}

/// Step 1 chooses the days and prepares the rule; step 2 shows which tasks
/// match and turns clean up on.
struct CleanUpSheet: View {
    enum Start: Identifiable, Equatable {
        case choose(days: UInt32)
        case review(JetAutodeleteRule)

        /// A paused rule opens on its matches when it is a draft, otherwise on
        /// choosing the days again.
        init(reviewing rule: JetAutodeleteRule) {
            if case .draft = rule.state {
                self = .review(rule)
            } else {
                self = .choose(days: rule.state.days ?? 30)
            }
        }

        var id: String {
            switch self {
            case let .choose(days): "choose-\(days)"
            case let .review(rule): "review-\(rule.id)"
            }
        }
    }

    private enum Phase: Equatable {
        case choosing
        case preparing
        case stillChecking
        case failed
        case review(JetAutodeleteRule)
    }

    @Bindable var model: JetRecoveryModel
    let start: Start
    let graceDays: UInt32
    let computerName: String
    let taskTitle: (UUID) -> String

    @Environment(\.dismiss) private var dismiss
    @State private var phase = Phase.choosing
    @State private var daysText = "30"
    @State private var didStart = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Clean Up Idle Tasks")
                .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
            switch phase {
            case .choosing, .preparing, .stillChecking, .failed:
                chooseStep
            case let .review(rule):
                reviewStep(rule)
            }
            Spacer(minLength: 0)
            buttons
        }
        .padding(20)
        .frame(minWidth: 480, idealWidth: 520, minHeight: 360, idealHeight: 460)
        .onAppear(perform: begin)
    }

    // MARK: Step 1

    private var chooseStep: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text("Move tasks to Jet Trash after")
                TextField("Days", text: $daysText)
                    .labelsHidden()
                    .textFieldStyle(.roundedBorder)
                    .multilineTextAlignment(.trailing)
                    .frame(width: 56)
                    .onSubmit(prepare)
                    .accessibilityLabel(Text("Days without activity"))
                Text("days without activity")
            }
            if isChangingApprovedRule {
                SettingsStatusLabel(
                    text: String(localized: "Changing the days pauses clean up until you turn it on again."),
                    systemImage: "exclamationmark.triangle.fill",
                    tint: .orange
                )
                .font(.system(size: JetDesign.TextSize.control))
            }
            if chosenDays == nil && !daysText.isEmpty {
                SettingValidationMessage(text: String(localized: "Enter a whole number of days from 1 to 36,500."))
            }
            switch phase {
            case .preparing:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Checking which tasks match…")
                }
                .foregroundStyle(.secondary)
            case .stillChecking:
                SettingsStatusLabel(
                    text: String(localized: "Jet is still checking. Try again in a minute."),
                    systemImage: "clock"
                )
                .foregroundStyle(.secondary)
            case .failed:
                if let issue = model.issues[.autodelete] {
                    SettingsIssueRow(error: issue, sentence: issue.category == .outcomeUnknown
                        ? String(localized: "Jet couldn't confirm this change.")
                        : String(localized: "Couldn't prepare clean up."))
                }
            case .choosing, .review:
                EmptyView()
            }
        }
    }

    // MARK: Step 2

    private func reviewStep(_ rule: JetAutodeleteRule) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Tasks idle for \(Int(rule.state.days ?? 0)) days move to Jet Trash. You can restore them for \(Int(graceDays)) days after that.")
                .fixedSize(horizontal: false, vertical: true)
            if rule.candidates.isEmpty {
                Text("No tasks match right now.")
                    .foregroundStyle(.secondary)
            } else {
                List(rule.candidates) { candidate in
                    CleanUpCandidateRow(candidate: candidate, title: taskTitle(candidate.conversationID))
                }
#if os(macOS)
                .listStyle(.bordered(alternatesRowBackgrounds: false))
#else
                .listStyle(.plain)
#endif
                .frame(minHeight: 120)
            }
        }
    }

    // MARK: Buttons

    private var buttons: some View {
        HStack {
            Spacer()
            Button("Cancel", role: .cancel) {
                Task {
                    await model.discardCleanUpDraft()
                    dismiss()
                }
            }
            .keyboardShortcut(.cancelAction)
            switch phase {
            case let .review(rule):
                if case .approved = rule.state {
                    Button("Done") { dismiss() }
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("Turn On Clean Up") {
                        Task {
                            await model.turnOnCleanUp(rule)
                            if model.issues[.autodelete] == nil { dismiss() }
                        }
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(model.operation != nil || model.isReadOnly)
                    .accessibilityIdentifier("cleanup-turn-on")
                }
            case .choosing, .preparing, .stillChecking, .failed:
                Button("Preview Matching Tasks", action: prepare)
                    .keyboardShortcut(.defaultAction)
                    .disabled(chosenDays == nil || phase == .preparing || model.operation != nil || model.isReadOnly)
            }
        }
    }

    private var chosenDays: UInt32? {
        guard let days = SettingNumberInput.parse(daysText), (1 ... 36_500).contains(days) else { return nil }
        return days
    }

    private var isChangingApprovedRule: Bool {
        guard case .approved = model.managedCleanUpRule?.state else { return false }
        return true
    }

    private func begin() {
        guard !didStart else { return }
        didStart = true
        switch start {
        case let .choose(days):
            daysText = String(days)
            phase = .choosing
        case let .review(rule):
            daysText = String(rule.state.days ?? 30)
            phase = .review(rule)
        }
    }

    private func prepare() {
        guard let days = chosenDays, phase != .preparing else { return }
        phase = .preparing
        Task {
            switch await model.prepareCleanUp(days: days) {
            case let .ready(rule): phase = .review(rule)
            case .stillChecking: phase = .stillChecking
            case .failed: phase = .failed
            }
        }
    }
}

private struct CleanUpCandidateRow: View {
    let candidate: JetAutodeleteCandidate
    let title: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .lineLimit(1)
            Text("Last used \(candidate.lastActiveAt.formatted(.dateTime.month(.abbreviated).day()))")
                .settingsCaption()
            if !candidate.protections.isEmpty {
                let sentences = candidate.protections.compactMap(CleanUpProtection.sentence)
                let others = candidate.protections.filter { CleanUpProtection.sentence($0) == nil }
                if !sentences.isEmpty {
                    SettingsStatusLabel(
                        text: String(localized: "Won't move yet: \(sentences.joined(separator: " "))"),
                        systemImage: "lock.fill",
                        tint: .orange
                    )
                    .font(.system(size: JetDesign.TextSize.metadata))
                }
                if !others.isEmpty {
                    DisclosureGroup("Details") {
                        Text(others.joined(separator: ", "))
                            .font(.system(size: JetDesign.TextSize.metadata, design: .monospaced))
                            .textSelection(.enabled)
                    }
                    .font(.system(size: JetDesign.TextSize.metadata))
                }
            }
        }
        .padding(.vertical, 2)
    }
}

/// Why a matching task isn't moved yet, as a sentence (design §6.11).
enum CleanUpProtection {
    static func sentence(_ code: String) -> String? {
        switch code {
        case "dirty_workspace": String(localized: "Its working copy has changes that aren't saved to a branch.")
        case "unpushed_work": String(localized: "It has commits that weren't pushed.")
        case "unresolved_effect": String(localized: "A Git step couldn't be confirmed.")
        case "enabled_schedule": String(localized: "It repeats daily.")
        case "pending_turn": String(localized: "Messages are still waiting to send.")
        case "active_run": String(localized: "An assistant is still working on it.")
        default: nil
        }
    }
}

/// Every rule that isn't the managed clean-up rule, with today's controls.
private struct CustomCleanUpRules: View {
    @Bindable var model: JetRecoveryModel
    let computerName: String
    let isReadOnly: Bool
    @State private var pendingRule: JetAutodeleteRule?
    @State private var pendingKind: RuleConfirmation?

    private enum RuleConfirmation { case approve, everywhere, delete }

    var body: some View {
        DisclosureGroup("Custom Rules") {
            VStack(alignment: .leading, spacing: 10) {
                if model.autodelete != nil && model.issues[.autodelete] != nil {
                    Text("Showing the last rules Jet saw. Check again before turning one on.")
                        .settingsCaption()
                }
                if model.customCleanUpRules.isEmpty {
                    Text("No custom rules.").foregroundStyle(.secondary)
                }
                ForEach(model.customCleanUpRules) { rule in ruleRow(rule) }
                TextField("Describe a rule", text: $model.rulePrompt, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .lineLimit(2 ... 4)
                HStack {
                    Text("\(model.rulePrompt.utf8.count.formatted()) / 4,096 bytes")
                        .settingsCaption()
                    Spacer()
                    Button("Check Rule") { Task { await model.compileRule() } }
                        .disabled(isReadOnly || model.operation != nil || model.rulePrompt.isEmpty || model.rulePrompt.utf8.count > 4_096)
                }
            }
            .padding(.top, 6)
        }
        .confirmationDialog(
            confirmationTitle,
            isPresented: Binding(get: { pendingRule != nil }, set: { if !$0 { pendingRule = nil; pendingKind = nil } }),
            titleVisibility: .visible
        ) {
            if let rule = pendingRule, let kind = pendingKind {
                Button(confirmationButton, role: kind == .approve ? nil : .destructive) {
                    pendingRule = nil
                    pendingKind = nil
                    Task {
                        switch kind {
                        case .approve: await model.approveRule(rule)
                        case .everywhere: await model.authorizeEverywhere(rule)
                        case .delete: await model.deleteRule(rule)
                        }
                    }
                }
            }
            Button("Cancel", role: .cancel) { pendingRule = nil; pendingKind = nil }
        } message: {
            Text(confirmationMessage)
        }
    }

    private func ruleRow(_ rule: JetAutodeleteRule) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(rule.prompt)
                .fixedSize(horizontal: false, vertical: true)
            Text(ruleState(rule)).settingsCaption()
            if !rule.candidates.isEmpty {
                Text("\(rule.candidates.count) matching tasks").settingsCaption()
            }
            HStack {
                TextField("Days", text: Binding(
                    get: { model.ruleDays[rule.id] ?? rule.state.days.map(String.init) ?? "" },
                    set: { model.ruleDays[rule.id] = $0 }
                ))
                .labelsHidden()
                .textFieldStyle(.roundedBorder)
                .frame(width: 64)
                .accessibilityLabel(Text("Idle days"))
                Button("Set Days") { Task { await model.setRuleDays(rule) } }
                    .disabled(isReadOnly || model.operation != nil || model.issues[.autodelete] != nil || !validDays(rule))
                if case .draft = rule.state {
                    Button("Turn On…") { confirm(rule, .approve) }
                        .disabled(isReadOnly || model.operation != nil || model.issues[.autodelete] != nil)
                }
                if case .approved = rule.state, rule.scope == "forget" {
                    Button("Allow Everywhere…", role: .destructive) { confirm(rule, .everywhere) }
                        .disabled(isReadOnly || model.operation != nil || model.issues[.autodelete] != nil)
                }
                Button("Remove Rule…", role: .destructive) { confirm(rule, .delete) }
                    .disabled(isReadOnly || model.operation != nil || model.issues[.autodelete] != nil)
            }
            .controlSize(.small)
        }
        .padding(.vertical, 4)
    }

    private func validDays(_ rule: JetAutodeleteRule) -> Bool {
        guard let days = UInt32(model.ruleDays[rule.id] ?? rule.state.days.map(String.init) ?? "") else { return false }
        return (1 ... 36_500).contains(days)
    }

    private func ruleState(_ rule: JetAutodeleteRule) -> String {
        switch rule.state {
        case .compiling: String(localized: "Jet is still checking this rule.")
        case .refused: String(localized: "Jet couldn't use this rule. Set the days to turn it into a draft.")
        case let .draft(days): String(localized: "Draft · tasks idle for \(Int(days)) days · Off")
        case let .approved(days, _): String(localized: "On · tasks idle for \(Int(days)) days")
        }
    }

    private func confirm(_ rule: JetAutodeleteRule, _ kind: RuleConfirmation) {
        pendingRule = rule
        pendingKind = kind
    }

    private var confirmationTitle: String {
        switch pendingKind {
        case .approve: String(localized: "Turn on this rule?")
        case .everywhere: String(localized: "Allow deleting everywhere?")
        case .delete: String(localized: "Remove this rule?")
        case nil: String(localized: "Review rule")
        }
    }

    private var confirmationButton: String {
        switch pendingKind {
        case .approve: String(localized: "Turn On")
        case .everywhere: String(localized: "Allow Everywhere")
        case .delete: String(localized: "Remove Rule")
        case nil: String(localized: "Continue")
        }
    }

    private var confirmationMessage: String {
        guard let rule = pendingRule else { return "" }
        switch pendingKind {
        case .approve:
            return String(localized: "Tasks on \(computerName) idle for \(Int(rule.state.days ?? 0)) days move to Jet Trash, where you can restore them before they're removed.")
        case .everywhere:
            return String(localized: "The rule can also delete the assistant's own history. Jet's data waits in Jet Trash until it's removed.")
        case .delete:
            return String(localized: "The rule stops choosing tasks. Tasks it already moved stay in Jet Trash until they're removed.")
        case nil:
            return ""
        }
    }
}
