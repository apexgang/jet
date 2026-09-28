import SwiftUI

struct JetAutodeleteSection: View {
    @Bindable var model: JetRecoveryModel
    let planeName: String
    @State private var pendingRule: JetAutodeleteRule?
    @State private var pendingKind: RuleConfirmation?

    // ASVS 2.3.1: rule changes resume only after the Plane can serve writes.
    private var isReadOnly: Bool { model.health?.recoveryState == "read_only" }

    private enum RuleConfirmation { case approve, everywhere, delete }

    var body: some View {
        Section("Auto-delete review") {
            Text("Rules run on \(planeName) only after you approve an exact inactivity period. Protected tasks stay out of Trash.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if isReadOnly {
                Text("This Plane is read-only. Restore a verified snapshot before changing rules.")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
            if model.isLoading && model.autodelete == nil { ProgressView("Loading rules") }
            if model.autodelete != nil && model.issues[.autodelete] != nil {
                Text("Showing the last observed rules. Refresh before approving a change.")
                    .font(.caption).foregroundStyle(.orange)
            }
            if let rules = model.autodelete?.rules {
                if rules.isEmpty { Text("No auto-delete rules.").foregroundStyle(.secondary) }
                ForEach(rules) { rule in ruleRow(rule) }
            }
            TextField("Describe an inactivity rule", text: $model.rulePrompt, axis: .vertical)
                .lineLimit(2 ... 4)
            Text("\(model.rulePrompt.utf8.count.formatted()) / 4,096 bytes")
                .font(.caption).foregroundStyle(.secondary)
            Button("Compile Rule") { Task { await model.compileRule() } }
                .disabled(isReadOnly || model.operation != nil || model.issues[.autodelete] != nil || model.rulePrompt.isEmpty || model.rulePrompt.utf8.count > 4_096)
            RecoveryIssue(error: model.issues[.autodelete])
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
            Text(rule.prompt).font(.headline)
            Text(ruleState(rule)).foregroundStyle(.secondary)
            if !rule.candidates.isEmpty {
                Text("\(rule.candidates.count) candidates in this bounded preview")
                ForEach(rule.candidates) { candidate in
                    Text("Task \(candidate.id.uuidString.prefix(8)) · Last active \(candidate.lastActiveAt.formatted(date: .abbreviated, time: .omitted)) · \(candidate.protections.isEmpty ? "No protections" : candidate.protections.joined(separator: ", "))")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            HStack {
                TextField("", text: Binding(
                    get: { model.ruleDays[rule.id] ?? rule.state.days.map(String.init) ?? "" },
                    set: { model.ruleDays[rule.id] = $0 }
                ))
                .frame(width: 90)
                .accessibilityLabel("Idle days")
                Button("Set Days") { Task { await model.setRuleDays(rule) } }
                    .disabled(isReadOnly || model.operation != nil || model.issues[.autodelete] != nil || !validDays(rule))
                if case .draft = rule.state {
                    Button("Approve") { confirm(rule, .approve) }
                        .disabled(isReadOnly || model.operation != nil || model.issues[.autodelete] != nil)
                }
                if case .approved = rule.state, rule.scope == "forget" {
                    Button("Allow Everywhere", role: .destructive) { confirm(rule, .everywhere) }
                        .disabled(isReadOnly || model.operation != nil || model.issues[.autodelete] != nil)
                }
                Button("Remove Rule", role: .destructive) { confirm(rule, .delete) }
                    .disabled(isReadOnly || model.operation != nil || model.issues[.autodelete] != nil)
            }
        }
        .padding(.vertical, 5)
    }

    private func validDays(_ rule: JetAutodeleteRule) -> Bool {
        guard let days = UInt32(model.ruleDays[rule.id] ?? rule.state.days.map(String.init) ?? "") else { return false }
        return (1 ... 36_500).contains(days)
    }

    private func ruleState(_ rule: JetAutodeleteRule) -> String {
        switch rule.state {
        case .compiling: "Compiling. Nothing can be approved yet."
        case let .refused(reason): "Refused: \(reason). Edit the days or source before approval."
        case let .draft(days): "Draft: forget tasks idle for \(days) days. Awaiting approval."
        case let .approved(days, at): "Approved \(at.formatted(date: .abbreviated, time: .shortened)): \(days) idle days · \(rule.scope)"
        }
    }

    private func confirm(_ rule: JetAutodeleteRule, _ kind: RuleConfirmation) {
        pendingRule = rule
        pendingKind = kind
    }

    private var confirmationTitle: String {
        switch pendingKind {
        case .approve: "Approve this rule?"
        case .everywhere: "Allow deletion everywhere?"
        case .delete: "Remove this rule?"
        case nil: "Review rule"
        }
    }

    private var confirmationButton: String {
        switch pendingKind {
        case .approve: "Approve Exact Days"
        case .everywhere: "Authorize Everywhere"
        case .delete: "Remove Rule"
        case nil: "Continue"
        }
    }

    private var confirmationMessage: String {
        guard let rule = pendingRule else { return "" }
        switch pendingKind {
        case .approve:
            return "Approve forgetting tasks on \(planeName) after \(rule.state.days ?? 0) idle days. Matches first enter Jet Trash, where they can be restored before expiry."
        case .everywhere:
            return "The rule can stage native deletion too. Jet's data waits in Trash until expiry. This release has no Craft that can erase a Harness's native history."
        case .delete:
            return "The rule stops selecting new tasks. Tasks it already staged remain in Jet Trash until restored or expired."
        case nil: return ""
        }
    }
}
