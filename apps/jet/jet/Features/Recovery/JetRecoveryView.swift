import SwiftUI

struct JetTrashSection: View {
    @Bindable var model: JetRecoveryModel
    let planeName: String
    let selectedTitle: String?
    @State private var pendingAction: JetRetentionAction?
    @State private var pendingRestore: JetTrashEntry?

    var body: some View {
        Section("Jet Trash") {
            Text("Entries are held on \(planeName) until their shown expiry. Restore before expiry to keep a task.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if model.isLoading && model.trash == nil { ProgressView("Loading Jet Trash") }
            if model.trash != nil && model.issues[.trash] != nil {
                Text("Showing the last observed Trash state.").font(.caption).foregroundStyle(.orange)
            }
            if let entries = model.trash?.entries {
                if entries.isEmpty { Text("Jet Trash is empty.").foregroundStyle(.secondary) }
                ForEach(entries) { entry in
                    VStack(alignment: .leading, spacing: 5) {
                        HStack {
                            Text(title(for: entry.conversationID)).font(.headline)
                            Spacer()
                            if entry.canRestore {
                                Button("Restore") { pendingRestore = entry }
                                    .disabled(model.operation != nil || model.issues[.trash] != nil)
                                    .accessibilityIdentifier("trash-restore-\(entry.id.uuidString)")
                            }
                        }
                        Text("Reason: \(entry.reason.replacingOccurrences(of: "_", with: " "))")
                        Text("Expires \(entry.expiresAt.formatted(date: .abbreviated, time: .shortened))")
                        if !entry.canRestore { Text("This transfer entry cannot be restored here.") }
                    }
                    .font(.caption)
                    .padding(.vertical, 3)
                }
            }
            RecoveryIssue(error: model.issues[.trash])
        }
        .confirmationDialog(
            "Restore this task?",
            isPresented: Binding(get: { pendingRestore != nil }, set: { if !$0 { pendingRestore = nil } }),
            titleVisibility: .visible
        ) {
            if let entry = pendingRestore {
                Button("Restore on \(planeName)") {
                    pendingRestore = nil
                    Task { await model.restore(entry.conversationID) }
                }
            }
            Button("Cancel", role: .cancel) { pendingRestore = nil }
        } message: {
            Text("Jet will remove this task from Trash on \(planeName).")
        }
    }

    @ViewBuilder
    var selectedTask: some View {
        Section("Remove current task") {
            if let conversationID = model.selectedConversationID {
                Text(selectedTitle ?? conversationID.uuidString).font(.headline)
                if let preview = model.preview, preview.conversationID == conversationID {
                    if let trash = preview.trash {
                        Text("Already in Jet Trash until \(trash.expiresAt.formatted(date: .abbreviated, time: .shortened)).")
                    } else {
                        if !preview.protections.isEmpty {
                            Text("Current protections: \(preview.protections.map { $0.replacingOccurrences(of: "_", with: " ") }.joined(separator: ", ")).")
                                .foregroundStyle(.secondary)
                        }
                        Text("\(preview.auditRecords.formatted()) content-free audit records will remain until audit retention ends.")
                            .font(.caption)
                        HStack {
                            Button("Forget on this Plane") { pendingAction = .forget }
                                .disabled(model.operation != nil || model.issues[.preview] != nil || preview.blocksForget)
                                .accessibilityIdentifier("retention-forget")
                            Button("Delete everywhere", role: .destructive) { pendingAction = .deleteEverywhere }
                                .disabled(model.operation != nil || model.issues[.preview] != nil)
                                .accessibilityIdentifier("retention-delete-everywhere")
                        }
                        if preview.blocksForget {
                            Text("Finish the active Run and queued turns before forgetting. Delete everywhere can stop and cancel them.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                } else if model.isLoading { ProgressView("Checking task protections") }
                RecoveryIssue(error: model.issues[.preview])
            } else {
                Text("Open a task in the main window to review its removal.")
                    .foregroundStyle(.secondary)
            }
        }
        .confirmationDialog(
            pendingAction == .forget ? "Forget this task?" : "Delete this task everywhere?",
            isPresented: Binding(get: { pendingAction != nil }, set: { if !$0 { pendingAction = nil } }),
            titleVisibility: .visible
        ) {
            if let action = pendingAction {
                Button(action == .forget ? "Move to Jet Trash" : "Stage deletion everywhere", role: .destructive) {
                    pendingAction = nil
                    Task { await model.stage(action) }
                }
            }
            Button("Cancel", role: .cancel) { pendingAction = nil }
        } message: {
            Text(pendingAction == .forget
                 ? "Jet will stage this task on \(planeName). At expiry, its Jet history, schedules, and managed Workspace are deleted unless you restore it. Its Harness history remains."
                 : "Jet will stage this task on \(planeName), stop its active Run, and cancel queued turns. At expiry, its Jet history and managed Workspace are deleted unless restored. This release cannot delete the Harness's native history.")
        }
    }

    private func title(for id: UUID) -> String {
        if model.selectedConversationID == id { return selectedTitle ?? id.uuidString }
        return "Task \(id.uuidString.prefix(8))"
    }
}

struct JetAutodeleteSection: View {
    @Bindable var model: JetRecoveryModel
    let planeName: String
    @State private var pendingRule: JetAutodeleteRule?
    @State private var pendingKind: RuleConfirmation?

    private enum RuleConfirmation { case approve, everywhere, delete }

    var body: some View {
        Section("Auto-delete review") {
            Text("Rules run on \(planeName) only after you approve an exact inactivity period. Protected tasks stay out of Trash.")
                .font(.caption)
                .foregroundStyle(.secondary)
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
                .disabled(model.operation != nil || model.issues[.autodelete] != nil || model.rulePrompt.isEmpty || model.rulePrompt.utf8.count > 4_096)
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
                TextField("Idle days", text: Binding(
                    get: { model.ruleDays[rule.id] ?? rule.state.days.map(String.init) ?? "" },
                    set: { model.ruleDays[rule.id] = $0 }
                ))
                .frame(width: 90)
                Button("Set Days") { Task { await model.setRuleDays(rule) } }
                    .disabled(model.operation != nil || model.issues[.autodelete] != nil || !validDays(rule))
                if case .draft = rule.state {
                    Button("Approve") { confirm(rule, .approve) }
                        .disabled(model.operation != nil || model.issues[.autodelete] != nil)
                }
                if case .approved = rule.state, rule.scope == "forget" {
                    Button("Allow Everywhere", role: .destructive) { confirm(rule, .everywhere) }
                        .disabled(model.operation != nil || model.issues[.autodelete] != nil)
                }
                Button("Remove Rule", role: .destructive) { confirm(rule, .delete) }
                    .disabled(model.operation != nil || model.issues[.autodelete] != nil)
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

struct JetSystemSection: View {
    @Bindable var model: JetRecoveryModel
    let planeName: String
    let diskPressure: Bool
    let diagnosticErrors: [JetPresentationError]
    @State private var pendingSnapshot: JetRecoverySnapshot?
    @State private var confirmPurge = false
    @State private var showDiagnosticCodes = false

    var body: some View {
        Section("Service health") {
            if model.isLoading && model.health == nil { ProgressView("Checking Plane health") }
            if model.health != nil && model.issues[.health] != nil {
                Text("Showing the last observed health state. Repair controls are unavailable until refresh succeeds.")
                    .font(.caption).foregroundStyle(.orange)
            }
            if let health = model.health {
                LabeledContent("Plane", value: planeName)
                LabeledContent("Jet core", value: health.daemonVersion)
                LabeledContent("Platform", value: health.platform)
                if !health.capabilitiesAvailable {
                    Text("Capabilities could not be refreshed. Recovery status below is from the Plane status query.")
                        .font(.caption).foregroundStyle(.orange)
                }
                LabeledContent("Started", value: health.daemonStartedAt.formatted(date: .abbreviated, time: .shortened))
                LabeledContent("Daemon starts", value: health.daemonStarts.formatted())
                LabeledContent("Credential store", value: health.credentialStore.label)
                ForEach(health.crafts) { craft in
                    LabeledContent("Craft \(craft.id)", value: craft.version)
                }
                ForEach(health.externalTools) { tool in
                    LabeledContent(tool.tool, value: toolVersion(tool))
                }
                if !health.degradedCapabilities.isEmpty {
                    Text("Degraded: \(health.degradedCapabilities.joined(separator: ", "))")
                        .foregroundStyle(.orange)
                }
                if diskPressure {
                    Text("Disk pressure blocked a recent write. Free space on the Plane and refresh before retrying. Existing reads remain available.")
                        .foregroundStyle(.orange)
                }
                LabeledContent("Store", value: health.recoveryState ?? "Not reported by this Plane")
                LabeledContent("Deletion ledger", value: health.deletionLedger ?? "Not reported")
                LabeledContent("Security audit", value: auditLabel(health.auditIntegrity))
                if case let .degraded(reason) = health.auditIntegrity {
                    Text("Audit integrity failed: \(reason). Export the evidence and review the gap before beginning a new audit epoch outside this view.")
                        .foregroundStyle(.orange)
                }
                Text("Runner version and live free disk space are not reported by this protocol. A storage.disk_pressure refusal identifies admission pressure when it occurs.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Button("Refresh Health") { Task { await model.refresh() } }
                .disabled(model.operation != nil)
            RecoveryIssue(error: model.issues[.health])
        }

        Section("Recovery snapshots") {
            if let health = model.health {
                if let reason = health.recoveryReason {
                    Text("The Plane is read-only: \(reason.replacingOccurrences(of: "_", with: " ")).")
                        .foregroundStyle(.orange)
                }
                if health.snapshots.isEmpty { Text("No verified Recovery snapshots.").foregroundStyle(.secondary) }
                ForEach(health.snapshots) { snapshot in
                    HStack {
                        VStack(alignment: .leading) {
                            Text(snapshot.takenAt.formatted(date: .abbreviated, time: .shortened))
                            Text("\(snapshot.reason) · \(ByteCountFormatter.string(fromByteCount: Int64(clamping: snapshot.bytes), countStyle: .file))")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if health.recoveryState == "read_only" {
                            Button("Restore") { pendingSnapshot = snapshot }
                                .disabled(model.operation != nil || model.issues[.health] != nil || health.deletionLedger == "corrupt")
                        }
                    }
                }
                if health.recoveryState == "serving" {
                    Button("Purge Older Snapshots", role: .destructive) { confirmPurge = true }
                        .disabled(model.operation != nil || model.issues[.health] != nil || health.auditIntegrity != .trusted)
                }
            } else {
                Text("Connect to this Plane to inspect verified snapshots.")
                    .foregroundStyle(.secondary)
            }
        }
        .confirmationDialog(
            "Restore this Recovery snapshot?",
            isPresented: Binding(get: { pendingSnapshot != nil }, set: { if !$0 { pendingSnapshot = nil } }),
            titleVisibility: .visible
        ) {
            if let snapshot = pendingSnapshot {
                Button("Replace Plane Store", role: .destructive) {
                    pendingSnapshot = nil
                    Task { await model.restoreSnapshot(snapshot) }
                }
            }
            Button("Cancel", role: .cancel) { pendingSnapshot = nil }
        } message: {
            Text("Jet will replace the read-only store on \(planeName) with this snapshot. Later state may be lost; the damaged store is kept aside. The Deletion ledger is reapplied, and the audit may need review afterward.")
        }
        .confirmationDialog(
            "Purge older Recovery snapshots?",
            isPresented: $confirmPurge,
            titleVisibility: .visible
        ) {
            Button("Create New Snapshot and Purge", role: .destructive) {
                Task { await model.purgeSnapshots() }
            }
            Button("Cancel", role: .cancel) { confirmPurge = false }
        } message: {
            Text("Jet will create a verified snapshot, then remove older snapshots that predate recorded deletions. Those older recovery points cannot be restored later.")
        }

        Section("Diagnostics") {
            Text("Status and stable error codes stay on this device. Jet does not upload diagnostics. This view omits prompts, file content, terminal output, credentials, and raw native errors.")
                .font(.caption).foregroundStyle(.secondary)
            Toggle("Show recent error codes", isOn: $showDiagnosticCodes)
            if showDiagnosticCodes {
                if diagnosticErrors.isEmpty && model.issues.isEmpty {
                    Text("No recent errors in this view.").foregroundStyle(.secondary)
                }
                ForEach(Array(Set((diagnosticErrors + Array(model.issues.values)).map(\.code))).sorted(), id: \.self) { code in
                    Text(code).font(.caption.monospaced())
                }
            }
        }
    }

    private func toolVersion(_ tool: JetExternalToolSummary) -> String {
        switch tool.availability {
        case let .present(version): version
        case .missing: "Unavailable"
        }
    }

    private func auditLabel(_ integrity: JetAuditIntegrity) -> String {
        switch integrity {
        case .trusted: "Trusted"
        case .degraded: "Degraded"
        case .unavailable: "Not reported"
        }
    }
}

struct JetAuditSection: View {
    @Bindable var model: JetRecoveryModel
    @State private var showReferences = false

    var body: some View {
        Section("Security audit") {
            Text("Structured decisions only. Prompts, files, terminal output, credentials, and target identities are hidden here by default.")
                .font(.caption).foregroundStyle(.secondary)
            Toggle("Show target references", isOn: $showReferences)
            if model.isLoadingAudit && model.auditEntries.isEmpty { ProgressView("Loading audit") }
            if !model.auditEntries.isEmpty && model.issues[.audit] != nil {
                Text("Showing the last observed audit page.").font(.caption).foregroundStyle(.orange)
            }
            if model.auditEntries.isEmpty && model.issues[.audit] == nil && !model.isLoadingAudit {
                Text("No audit records in this page.").foregroundStyle(.secondary)
            }
            ForEach(model.auditEntries) { entry in
                VStack(alignment: .leading, spacing: 4) {
                    Text(entry.decision).font(.headline)
                    Text("\(entry.recordedAt.formatted(date: .abbreviated, time: .shortened)) · \(entry.actor) · \(entry.outcome) · \(entry.risk)")
                    Text(showReferences ? "\(entry.targetKind): \(entry.targetReference)" : "Target reference hidden")
                }
                .font(.caption)
                .padding(.vertical, 3)
            }
            if model.hasMoreAudit {
                Button("Load More Audit Records") { Task { await model.loadMoreAudit() } }
                    .disabled(model.isLoadingAudit)
            }
            RecoveryIssue(error: model.issues[.audit])
        }
    }
}

private struct RecoveryIssue: View {
    let error: JetPresentationError?

    var body: some View {
        if let error {
            VStack(alignment: .leading, spacing: 3) {
                Text(error.message)
                Text(error.code).font(.caption.monospaced())
            }
            .foregroundStyle(.orange)
        }
    }
}

#Preview("Recovery offline") {
    let model = JetRecoveryModel(makeAccess: { _ in
        throw JetClientFailure.presentation(.offline)
    })
    model.issues[.health] = .offline
    return Form {
        JetSystemSection(model: model, planeName: "This Mac", diskPressure: false, diagnosticErrors: [])
        JetAuditSection(model: model)
    }
    .formStyle(.grouped)
    .frame(width: 820, height: 680)
}

#Preview("Recovery ready") {
    let model = JetRecoveryModel(makeAccess: { _ in
        throw JetClientFailure.presentation(.offline)
    })
    model.health = JetSystemHealth(
        planeID: UUID(), daemonVersion: "1.40", daemonStarts: 12,
        daemonStartedAt: Date(timeIntervalSince1970: 1_750_000_000),
        platform: "macOS · arm64", capabilitiesAvailable: true,
        externalTools: [], crafts: [], degradedCapabilities: [],
        credentialStore: .available, recoveryState: "serving", recoveryReason: nil,
        snapshots: [JetRecoverySnapshot(
            name: "plane-1750000000000-daily.sqlite3", reason: "daily",
            takenAt: Date(timeIntervalSince1970: 1_750_000_000), bytes: 405_504
        )], deletionLedger: "verified", auditIntegrity: .trusted
    )
    return Form {
        JetSystemSection(model: model, planeName: "This Mac", diskPressure: false, diagnosticErrors: [])
        JetAuditSection(model: model)
    }
    .formStyle(.grouped)
    .frame(width: 820, height: 680)
}
