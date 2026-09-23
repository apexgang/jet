import SwiftUI

enum JetSettingsPane: String, CaseIterable, Identifiable, Sendable {
    case general
    case agents
    case work
    case connections
    case safety

    var id: Self { self }

    var title: String {
        switch self {
        case .general: "General"
        case .agents: "Agents"
        case .work: "Work"
        case .connections: "Connections"
        case .safety: "Safety & System"
        }
    }

    var symbol: String {
        switch self {
        case .general: "gear"
        case .agents: "person.2"
        case .work: "hammer"
        case .connections: "network"
        case .safety: "checkmark.shield"
        }
    }

    static func resolving(_ error: JetPresentationError) -> JetSettingsPane? {
        let prefix = error.code.split(separator: ".").first.map(String.init)
        return switch prefix {
        case "notification": .general
        case "account", "credential", "extension", "craft", "usage": .agents
        case "schedule", "git", "retention", "autodelete": .work
        case "pairing", "remote", "ssh": .connections
        case "setting", "review", "audit", "storage", "energy", "recovery": .safety
        default: nil
        }
    }
}

struct JetSettingsView: View {
    let session: DesktopSession
    @State private var model: JetSettingsModel
    @State private var recoveryModel: JetRecoveryModel
    @AppStorage("jet.settings.last-pane") private var selectedPane = JetSettingsPane.general.rawValue
    @AppStorage("jet.settings.restore-last-task") private var restoresLastTask = true

    init(session: DesktopSession) {
        self.session = session
        _model = State(initialValue: JetSettingsModel(makeAccess: session.settingsAccess))
        _recoveryModel = State(initialValue: JetRecoveryModel(
            makeAccess: session.recoveryAccess(for:),
            onConversationChange: { staged in
                if staged { session.beginNewTask() }
                await session.loadConversations()
                if staged { session.beginNewTask() }
            }
        ))
    }

    var body: some View {
        TabView(selection: paneBinding) {
            GeneralSettingsPane(session: session, restoresLastTask: $restoresLastTask)
                .tabItem { Label(JetSettingsPane.general.title, systemImage: JetSettingsPane.general.symbol) }
                .tag(JetSettingsPane.general)

            AgentsSettingsPane(session: session, model: model)
                .tabItem { Label(JetSettingsPane.agents.title, systemImage: JetSettingsPane.agents.symbol) }
                .tag(JetSettingsPane.agents)

            WorkSettingsPane(session: session, model: model, recovery: recoveryModel)
                .tabItem { Label(JetSettingsPane.work.title, systemImage: JetSettingsPane.work.symbol) }
                .tag(JetSettingsPane.work)

            PlaneManagementView(session: session)
                .tabItem { Label(JetSettingsPane.connections.title, systemImage: JetSettingsPane.connections.symbol) }
                .tag(JetSettingsPane.connections)

            SafetySettingsPane(session: session, model: model, recovery: recoveryModel)
                .tabItem { Label(JetSettingsPane.safety.title, systemImage: JetSettingsPane.safety.symbol) }
                .tag(JetSettingsPane.safety)
        }
        .frame(minWidth: 760, idealWidth: 820, minHeight: 600, idealHeight: 680)
        .task(id: loadID) {
            await model.load(
                conversationID: session.selectedConversationID,
                projectID: selectedProjectID,
                crafts: session.selectedSetupSnapshot?.capabilities.crafts ?? [],
                authProviders: session.selectedSetupSnapshot?.capabilities.authProviders ?? []
            )
        }
        .task(id: loadID) {
            await recoveryModel.load(
                planeRegistryID: session.selectedPlaneRegistryID,
                conversationID: session.selectedConversationID
            )
        }
        .onAppear(perform: applyRequestedPane)
        .onChange(of: session.requestedSettingsPane) { _, _ in applyRequestedPane() }
    }

    private var selectedProjectID: UUID? {
        session.selectedConversation?.projectID ?? session.selectedProjectID
    }

    private var loadID: String {
        let craftIDs = session.selectedSetupSnapshot?.capabilities.crafts.map(\.id).joined(separator: ",") ?? ""
        return [
            session.selectedPlaneRegistryID.uuidString,
            session.selectedConversationID?.uuidString ?? "none",
            selectedProjectID?.uuidString ?? "none",
            craftIDs,
        ].joined(separator: "|")
    }

    private var paneBinding: Binding<JetSettingsPane> {
        Binding(
            get: { JetSettingsPane(rawValue: selectedPane) ?? .general },
            set: { selectedPane = $0.rawValue }
        )
    }

    private func applyRequestedPane() {
        guard let pane = session.requestedSettingsPane else { return }
        selectedPane = pane.rawValue
        session.requestedSettingsPane = nil
    }
}

private struct GeneralSettingsPane: View {
    let session: DesktopSession
    @Binding var restoresLastTask: Bool
    @AppStorage(JetNotificationPreferences.approvalsKey) private var approvalNotifications = false
    @AppStorage(JetNotificationPreferences.completionsKey) private var completionNotifications = false
    @AppStorage(JetNotificationPreferences.failuresKey) private var failureNotifications = false

    var body: some View {
        SettingsForm {
            Section("Workspace") {
                // ASVS 14.3.3: this is client-owned Boolean state. It contains
                // no Jet content, credential, path, or connection proof.
                Toggle("Restore the last task when Jet opens", isOn: $restoresLastTask)
            }

            Section("Appearance") {
                LabeledContent("Theme", value: "Use system setting")
                LabeledContent("Accent", value: "Jet Blue")
            }

            Section("Notification routing") {
                Toggle("Approval requests", isOn: $approvalNotifications)
                    .accessibilityIdentifier("notifications-approvals")
                Toggle("Completed tasks", isOn: $completionNotifications)
                    .accessibilityIdentifier("notifications-completions")
                Toggle("Failed tasks", isOn: $failureNotifications)
                    .accessibilityIdentifier("notifications-failures")
                LabeledContent("System permission", value: authorizationLabel)
                Text("Notifications omit task names, prompts, paths, and tool details. Jet requests alerts only, without sounds.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if let error = session.notificationError {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                }
            }
        }
        .task { await session.refreshNotificationAuthorization() }
        .onChange(of: approvalNotifications) { _, enabled in requestNotificationAccess(if: enabled) }
        .onChange(of: completionNotifications) { _, enabled in requestNotificationAccess(if: enabled) }
        .onChange(of: failureNotifications) { _, enabled in requestNotificationAccess(if: enabled) }
    }

    private var authorizationLabel: String {
        switch session.notificationAuthorization {
        case .notDetermined: "Not requested"
        case .denied: "Blocked in System Settings"
        case .authorized: "Allowed"
        case .provisional: "Delivered quietly"
        }
    }

    private func requestNotificationAccess(if enabled: Bool) {
        guard enabled, !session.notificationAuthorization.permitsDelivery else { return }
        Task { _ = await session.requestNotificationAuthorization() }
    }
}

private struct AgentsSettingsPane: View {
    let session: DesktopSession
    @Bindable var model: JetSettingsModel

    var body: some View {
        SettingsForm {
            Section("Accounts") {
                Text("Bindings contain non-secret metadata. Authentication stays with the Harness or secure credential store.")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                if let accounts = model.accounts, !accounts.bindings.isEmpty {
                    ForEach(accounts.bindings) { binding in
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(binding.label)
                                Text("\(binding.provider.capitalized) · \(binding.stateLabel)")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("Remove", role: .destructive) {
                                model.pendingAccountRemoval = binding
                            }
                            .disabled(model.operation != nil)
                        }
                    }
                } else if model.issues[.accounts] == nil {
                    Text("No Harness accounts connected")
                        .foregroundStyle(.secondary)
                }

                if credentialStoreUnavailable {
                    Label(
                        session.selectedSetupSnapshot?.capabilities.credentialStore.label
                            ?? "Secure storage unavailable",
                        systemImage: "lock.trianglebadge.exclamationmark"
                    )
                    .foregroundStyle(.orange)
                }

                HStack {
                    ForEach(model.authProviders) { provider in
                        Button("Use \(provider.harness) login") {
                            Task { await model.bindAccount(provider) }
                        }
                        .disabled(model.operation != nil || credentialStoreUnavailable)
                    }
                }
                SettingsIssueView(error: model.issues[.accounts])
            }

            Section("Usage") {
                if let usage = model.usage {
                    LabeledContent("Observed tokens", value: usage.tokens.total.formatted())
                    LabeledContent("Measurements", value: usage.measurements.formatted())
                    if usage.estimated > 0 || usage.interim > 0 {
                        Text("\(usage.estimated.formatted()) estimated · \(usage.interim.formatted()) still changing")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    ForEach(usage.quotaWindows) { window in
                        LabeledContent("\(window.provider.capitalized) · \(window.window)") {
                            Text(quotaLabel(window))
                        }
                    }
                } else if model.issues[.usage] == nil {
                    Text("No Usage has been observed on this Plane.")
                        .foregroundStyle(.secondary)
                }
                if let history = model.usageHistory, !history.series.isEmpty {
                    DisclosureGroup("Last 30 days") {
                        ForEach(history.series) { series in
                            LabeledContent(series.model ?? "Model not reported") {
                                Text("\(series.tokens.total.formatted()) tokens")
                            }
                        }
                    }
                }
                SettingsIssueView(error: model.issues[.usage])
            }

            Section("Harness extensions") {
                if model.crafts.isEmpty {
                    Text("Install a Craft that supports extensions before managing Harness-native skills, hooks, MCP servers, or plugins.")
                        .foregroundStyle(.secondary)
                } else {
                    Picker("Craft", selection: $model.selectedExtensionCraftID) {
                        ForEach(model.crafts) { craft in
                            Text("\(craft.id) \(craft.version)").tag(craft.id)
                        }
                    }
                    Picker("Change", selection: $model.extensionAction) {
                        ForEach(JetExtensionAction.allCases, id: \.self) { action in
                            Text(action.title).tag(action)
                        }
                    }
                    TextField("Native extension identity", text: $model.extensionID)
                        .textFieldStyle(.roundedBorder)
                    Button("Review Change…") {
                        Task { await model.inspectExtension() }
                    }
                    .disabled(
                        model.operation != nil
                            || model.selectedExtensionCraftID.isEmpty
                            || model.extensionID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    )
                }

                ForEach(model.extensionCatalogs) { catalog in
                    DisclosureGroup("\(catalog.harness) catalog") {
                        Text(catalog.nativeMetadata)
                            .font(.caption.monospaced())
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                if let change = model.latestExtensionChange {
                    LabeledContent("Last change") {
                        Text("\(change.action.title) · \(change.state.replacingOccurrences(of: "_", with: " "))")
                    }
                }
                SettingToggleRow(
                    model: model,
                    scope: .plane,
                    key: "craft.developer_mode",
                    title: "Developer Mode",
                    help: "Allow reviewed local or source-built third-party Crafts."
                )
                SettingsIssueView(error: model.issues[.extensions])
            }

            SettingsNotice(model: model)
        }
        .confirmationDialog(
            "Remove this Account binding?",
            isPresented: Binding(
                get: { model.pendingAccountRemoval != nil },
                set: { if !$0 { model.pendingAccountRemoval = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Remove Account Binding", role: .destructive) {
                Task { await model.confirmAccountRemoval() }
            }
            Button("Cancel", role: .cancel) { model.pendingAccountRemoval = nil }
        } message: {
            Text("New work can no longer use this binding. Jet does not delete the Provider account or copy its credential.")
        }
        .confirmationDialog(
            extensionConfirmationTitle,
            isPresented: Binding(
                get: { model.pendingExtensionProposal != nil },
                set: { if !$0 { model.pendingExtensionProposal = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button(extensionConfirmationButton, role: extensionConfirmationRole) {
                Task { await model.confirmExtensionChange() }
            }
            Button("Cancel", role: .cancel) { model.pendingExtensionProposal = nil }
        } message: {
            Text(extensionConfirmationMessage)
        }
    }

    private var credentialStoreUnavailable: Bool {
        session.selectedSetupSnapshot?.capabilities.credentialStore == .unavailable
    }

    private func quotaLabel(_ window: JetQuotaWindowSummary) -> String {
        let amount = window.limit.map { "\(window.used.formatted()) of \($0.formatted())" }
            ?? window.used.formatted()
        let freshness = switch window.freshness {
        case .fresh: "current"
        case .stale: "stale"
        case .unreachable: "unreachable"
        }
        return "\(amount) \(window.unit) · \(freshness)"
    }

    private var extensionConfirmationTitle: String {
        guard let proposal = model.pendingExtensionProposal else { return "Review extension change" }
        return "\(proposal.action.title) \(proposal.extensionID)?"
    }

    private var extensionConfirmationButton: String {
        model.pendingExtensionProposal?.action.title ?? "Continue"
    }

    private var extensionConfirmationRole: ButtonRole? {
        model.pendingExtensionProposal?.action == .remove ? .destructive : nil
    }

    private var extensionConfirmationMessage: String {
        guard let proposal = model.pendingExtensionProposal else { return "" }
        return "The \(proposal.catalog.harness) adapter will revalidate the inspected catalog. Skills can direct tools; hooks, MCP servers, and plugins can execute as your user. The change applies to subsequent Runs by default."
    }
}

private enum WorkSettingScope: String, CaseIterable, Identifiable {
    case project
    case conversation

    var id: Self { self }
    var title: String { self == .project ? "Project" : "Current task" }
}

private struct WorkSettingsPane: View {
    let session: DesktopSession
    @Bindable var model: JetSettingsModel
    @Bindable var recovery: JetRecoveryModel
    @State private var selectedScope = WorkSettingScope.project

    var body: some View {
        SettingsForm {
            Section("Defaults") {
                if availableScopes.count > 1 {
                    Picker("Scope", selection: $selectedScope) {
                        ForEach(availableScopes) { scope in Text(scope.title).tag(scope) }
                    }
                    .pickerStyle(.segmented)
                }
                if let scope = settingScope {
                    SettingToggleRow(
                        model: model,
                        scope: scope,
                        key: "utility.automatic_naming",
                        title: "Name tasks automatically",
                        help: "Use the Plane's Utility model when one is configured."
                    )
                    SettingToggleRow(model: model, scope: scope, key: "git.auto_branch", title: "Create a branch after successful work")
                    SettingToggleRow(model: model, scope: scope, key: "git.auto_commit", title: "Commit successful changes automatically")
                    SettingToggleRow(model: model, scope: scope, key: "git.auto_push", title: "Push successful changes automatically")
                    SettingToggleRow(model: model, scope: scope, key: "git.auto_draft_pull_request", title: "Open or update a draft pull request")
                    SettingTextRow(
                        model: model,
                        scope: scope,
                        key: "git.branch_prefix",
                        title: "Branch prefix",
                        help: "Jet appends a generated task identity to this prefix."
                    )
                    SnapshotFence(snapshot: snapshot)
                } else {
                    Text("Choose a Project or task in the main window to edit its defaults.")
                        .foregroundStyle(.secondary)
                }
                SettingsIssueView(error: model.issues[selectedScope == .project ? .project : .conversation])
            }

            Section("Schedules") {
                if model.conversationID == nil {
                    Text("Open a task in the main window to manage its schedules.")
                        .foregroundStyle(.secondary)
                } else {
                    if let tasks = model.schedules?.tasks, !tasks.isEmpty {
                        ForEach(tasks) { task in
                            VStack(alignment: .leading, spacing: 5) {
                                HStack {
                                    Text("Daily at \(task.localTime) · \(task.timeZone)")
                                        .font(.headline)
                                    Spacer()
                                    Button("Cancel", role: .destructive) {
                                        model.pendingScheduleCancellation = task
                                    }
                                    .disabled(model.operation != nil)
                                }
                                Text(task.prompt)
                                    .lineLimit(3)
                                Text("Next: \(Date(timeIntervalSince1970: Double(task.nextDueAtUnixMilliseconds) / 1_000).formatted(date: .abbreviated, time: .shortened))")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            .padding(.vertical, 4)
                        }
                    } else if model.issues[.schedules] == nil {
                        Text("No daily schedules for this task.")
                            .foregroundStyle(.secondary)
                    }

                    DatePicker("Local time", selection: $model.scheduleTime, displayedComponents: .hourAndMinute)
                    TextField("IANA time zone", text: $model.scheduleTimeZone)
                        .textFieldStyle(.roundedBorder)
                    TextField("Instructions for each scheduled turn", text: $model.schedulePrompt, axis: .vertical)
                        .lineLimit(2 ... 5)
                    Button("Create Daily Schedule") {
                        Task { await model.createSchedule() }
                    }
                    .disabled(
                        model.operation != nil
                            || model.schedulePrompt.isEmpty
                            || model.schedulePrompt.utf8.count > 8_192
                    )
                }
                SettingsIssueView(error: model.issues[.schedules])
            }

            Section("Retention") {
                SettingCountRow(
                    model: model,
                    scope: .plane,
                    key: "retention.trash_grace_days",
                    title: "Jet Trash grace period",
                    unit: "days",
                    minimum: 1
                )
                Text("This changes future Trash entries. Existing entries keep the grace period recorded when they were staged.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                SettingsIssueView(error: model.issues[.plane])
            }

            JetTrashSection(
                model: recovery,
                planeName: session.selectedPlane?.name ?? "Local Plane",
                selectedTitle: session.selectedConversation?.title
            ).selectedTask
            JetTrashSection(
                model: recovery,
                planeName: session.selectedPlane?.name ?? "Local Plane",
                selectedTitle: session.selectedConversation?.title
            )
            JetAutodeleteSection(
                model: recovery,
                planeName: session.selectedPlane?.name ?? "Local Plane"
            )

            Section {
                Button("Refresh Retention and Trash") { Task { await recovery.refresh() } }
                    .disabled(recovery.operation != nil)
                if let notice = recovery.notice { Text(notice).font(.caption).foregroundStyle(.secondary) }
            }

            SettingsNotice(model: model)
        }
        .onChange(of: availableScopes) { _, scopes in
            if !scopes.contains(selectedScope) { selectedScope = scopes.first ?? .project }
        }
        .confirmationDialog(
            "Cancel this schedule?",
            isPresented: Binding(
                get: { model.pendingScheduleCancellation != nil },
                set: { if !$0 { model.pendingScheduleCancellation = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Cancel Future Firings", role: .destructive) {
                Task { await model.confirmScheduleCancellation() }
            }
            Button("Keep Schedule", role: .cancel) { model.pendingScheduleCancellation = nil }
        } message: {
            Text("Future firings and pending scheduled input are removed. Active work continues.")
        }
    }

    private var availableScopes: [WorkSettingScope] {
        var values: [WorkSettingScope] = []
        if model.projectID != nil { values.append(.project) }
        if model.conversationID != nil { values.append(.conversation) }
        return values
    }

    private var settingScope: JetSettingScope? {
        switch selectedScope {
        case .project: model.projectID.map(JetSettingScope.project)
        case .conversation: model.conversationID.map(JetSettingScope.conversation)
        }
    }

    private var snapshot: JetSettingSnapshot? {
        selectedScope == .project ? model.projectSettings : model.conversationSettings
    }
}

private struct SafetySettingsPane: View {
    let session: DesktopSession
    @Bindable var model: JetSettingsModel
    @Bindable var recovery: JetRecoveryModel

    var body: some View {
        SettingsForm {
            Section("Execution defaults") {
                SettingCountRow(model: model, scope: .plane, key: "energy.concurrency", title: "Concurrent managed work", unit: "Runs", minimum: 1)
                SettingCountRow(model: model, scope: .plane, key: "energy.low_power_concurrency", title: "Low-power concurrency", unit: "Runs", minimum: 0)
                SettingToggleRow(model: model, scope: .plane, key: "energy.constrained", title: "Always use the constrained budget")
                SettingToggleRow(
                    model: model,
                    scope: .plane,
                    key: "energy.foreground_override",
                    title: "Allow foreground work above the budget",
                    help: "This admits explicit foreground work only. jetd still enforces the active limit."
                )
                SettingsIssueView(error: model.issues[.plane])
            }

            Section("Automatic reviews") {
                SettingToggleRow(
                    model: model,
                    scope: .plane,
                    key: "review.automatic",
                    title: "Review eligible approval requests automatically",
                    help: "The reviewer can allow once or deny. The Plane applies its own deny rules and authorization checks."
                )
                BindingSettingPicker(
                    model: model,
                    key: "review.account_binding",
                    title: "Reviewer",
                    defaultLabel: "Each Run's own account"
                )
                BindingSettingPicker(
                    model: model,
                    key: "review.cross_provider_consent",
                    title: "Cross-provider consent",
                    defaultLabel: "No cross-provider access"
                )
            }

            Section("Storage and retention") {
                SettingCountRow(model: model, scope: .plane, key: "storage.disposable_mib", title: "Disposable storage budget", unit: "MiB", minimum: 0)
                SettingCountRow(model: model, scope: .plane, key: "artifact.max_mib", title: "Maximum Artifact", unit: "MiB", minimum: 1)
                SettingCountRow(model: model, scope: .plane, key: "artifact.run_mib", title: "Artifact budget per Run", unit: "MiB", minimum: 1)
                SettingCountRow(model: model, scope: .plane, key: "security.audit_retention_days", title: "Security audit retention", unit: "days", minimum: 90)
                SnapshotFence(snapshot: model.planeSettings)
            }

            Section("System") {
                if model.isLoading {
                    ProgressView("Refreshing Plane settings")
                } else if let snapshot = model.planeSettings {
                    LabeledContent("Observed through event", value: snapshot.cursor.formatted())
                }
                Text("Policy, authorization, resource admission, and revision checks remain authoritative in jetd. This window sends typed Commands and reloads the resulting snapshot.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            JetSystemSection(
                model: recovery,
                planeName: session.selectedPlane?.name ?? "Local Plane",
                diskPressure: [session.actionError, session.workError, session.workNoticeError, session.gitDeliveryError, session.selectedPlane?.failure]
                    .contains(where: { $0?.code == "storage.disk_pressure" }),
                diagnosticErrors: [session.actionError, session.workError, session.workNoticeError, session.gitDeliveryError, session.selectedPlane?.failure]
                    .compactMap { $0 }
            )
            JetAuditSection(model: recovery)

            if let notice = recovery.notice {
                Section { Text(notice).font(.caption).foregroundStyle(.secondary) }
            }

            SettingsNotice(model: model)
        }
    }
}

private struct BindingSettingPicker: View {
    @Bindable var model: JetSettingsModel
    let key: String
    let title: String
    let defaultLabel: String

    var body: some View {
        if currentValue != nil {
            Picker(title, selection: selection) {
                Text(defaultLabel).tag("")
                ForEach(model.accounts?.bindings ?? []) { binding in
                    Text("\(binding.label) · \(binding.provider.capitalized)")
                        .tag(binding.id.uuidString.lowercased())
                }
            }
            .disabled(model.operation != nil)
        } else {
            UnavailableSettingValue(title: title, isLoading: model.isLoading)
        }
    }

    private var selection: Binding<String> {
        Binding(
            get: { currentValue ?? "" },
            set: { value in
                Task { await model.setSetting(key, value: .text(value), scope: .plane) }
            }
        )
    }

    private var currentValue: String? {
        guard case let .text(value) = model.settingValue(key, scope: .plane) else {
            return nil
        }
        return value
    }
}

private struct SettingToggleRow: View {
    @Bindable var model: JetSettingsModel
    let scope: JetSettingScope
    let key: String
    let title: String
    var help: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if currentValue != nil {
                Toggle(title, isOn: value)
                    .disabled(model.operation != nil)
            } else {
                UnavailableSettingValue(title: title, isLoading: model.isLoading)
            }
            if let help {
                Text(help)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if isExplicit {
                Button("Use Inherited Value") {
                    Task { await model.clearSetting(key, scope: scope) }
                }
                .buttonStyle(.plain)
                .foregroundStyle(.tint)
                .font(.caption)
                .disabled(model.operation != nil)
            }
        }
    }

    private var value: Binding<Bool> {
        Binding(
            get: { currentValue ?? false },
            set: { value in
                Task { await model.setSetting(key, value: .flag(value), scope: scope) }
            }
        )
    }

    private var currentValue: Bool? {
        guard case let .flag(value) = model.settingValue(key, scope: scope) else {
            return nil
        }
        return value
    }

    private var isExplicit: Bool {
        model.settingSource(key, scope: scope) == .scope(scope)
    }
}

private struct SettingTextRow: View {
    @Bindable var model: JetSettingsModel
    let scope: JetSettingScope
    let key: String
    let title: String
    var help: String?
    @State private var draft = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            if currentValue != nil {
                LabeledContent(title) {
                    TextField(title, text: $draft)
                        .textFieldStyle(.roundedBorder)
                        .frame(minWidth: 220)
                }
                if let help {
                    Text(help)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                HStack {
                    Button("Save") {
                        Task { await model.setSetting(key, value: .text(draft), scope: scope) }
                    }
                    .disabled(model.operation != nil || draft.utf8.count > 2_048)
                    if isExplicit {
                        Button("Use Inherited Value") {
                            Task { await model.clearSetting(key, scope: scope) }
                        }
                        .disabled(model.operation != nil)
                    }
                }
                .buttonStyle(.plain)
                .foregroundStyle(.tint)
                .font(.caption)
            } else {
                UnavailableSettingValue(title: title, isLoading: model.isLoading)
            }
        }
        .onAppear(perform: syncDraft)
        .onChange(of: currentValue) { _, _ in syncDraft() }
    }

    private var currentValue: String? {
        guard case let .text(value) = model.settingValue(key, scope: scope) else { return nil }
        return value
    }

    private var isExplicit: Bool {
        model.settingSource(key, scope: scope) == .scope(scope)
    }

    private func syncDraft() {
        if let currentValue { draft = currentValue }
    }
}

private struct SettingCountRow: View {
    @Bindable var model: JetSettingsModel
    let scope: JetSettingScope
    let key: String
    let title: String
    let unit: String
    let minimum: UInt32
    @State private var draft = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            if currentValue != nil {
                LabeledContent(title) {
                    HStack {
                        TextField(unit, text: $draft)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 90)
                        Text(unit)
                            .foregroundStyle(.secondary)
                    }
                }
                HStack {
                    Button("Save") {
                        guard let value = UInt32(draft), value >= minimum else { return }
                        Task { await model.setSetting(key, value: .count(value), scope: scope) }
                    }
                    .disabled(!draftIsValid || model.operation != nil)
                    if isExplicit {
                        Button("Use Inherited Value") {
                            Task { await model.clearSetting(key, scope: scope) }
                        }
                        .disabled(model.operation != nil)
                    }
                }
                .buttonStyle(.plain)
                .foregroundStyle(.tint)
                .font(.caption)
            } else {
                UnavailableSettingValue(title: title, isLoading: model.isLoading)
            }
        }
        .onAppear(perform: syncDraft)
        .onChange(of: currentValue) { _, _ in syncDraft() }
    }

    private var currentValue: UInt32? {
        guard case let .count(value) = model.settingValue(key, scope: scope) else { return nil }
        return value
    }

    private var draftIsValid: Bool {
        guard let value = UInt32(draft) else { return false }
        return value >= minimum
    }

    private var isExplicit: Bool {
        model.settingSource(key, scope: scope) == .scope(scope)
    }

    private func syncDraft() {
        if let currentValue { draft = String(currentValue) }
    }
}

private struct UnavailableSettingValue: View {
    let title: String
    let isLoading: Bool

    var body: some View {
        LabeledContent(title, value: isLoading ? "Loading…" : "Unavailable")
            .foregroundStyle(.secondary)
    }
}

private struct SnapshotFence: View {
    let snapshot: JetSettingSnapshot?

    var body: some View {
        if let snapshot {
            Text("Observed through Plane event \(snapshot.cursor.formatted())")
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.secondary)
        }
    }
}

private struct SettingsIssueView: View {
    let error: JetPresentationError?

    var body: some View {
        if let error {
            Label {
                VStack(alignment: .leading, spacing: 2) {
                    Text(error.message)
                    Text(error.code)
                        .font(.caption2.monospaced())
                }
            } icon: {
                Image(systemName: "exclamationmark.triangle")
            }
            .font(.caption)
            .foregroundStyle(.orange)
            .accessibilityElement(children: .combine)
        }
    }
}

private struct SettingsNotice: View {
    let model: JetSettingsModel

    var body: some View {
        if let notice = model.notice {
            Section {
                Text(notice)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("settings-notice")
            }
        }
    }
}

private struct SettingsForm<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        Form { content }
            .formStyle(.grouped)
            .scrollContentBackground(.hidden)
            .padding()
    }
}

struct JetSettingsRecoveryButton: View {
    let session: DesktopSession
    let error: JetPresentationError
#if os(macOS)
    @Environment(\.openSettings) private var openSettings
#endif

    var body: some View {
        if let pane = JetSettingsPane.resolving(error) {
#if os(macOS)
            Button("Open \(pane.title) Settings") {
                session.requestSettings(pane)
                openSettings()
            }
#else
            EmptyView()
#endif
        }
    }
}
