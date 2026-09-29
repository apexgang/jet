import SwiftUI

/// Settings › Safety: automatic safety review and how many tasks run at once.
struct SafetySettingsPane: View {
    let session: DesktopSession
    let settings: JetSettingsModel
    let recovery: JetRecoveryModel
    @Binding var computer: UUID

    @State private var pendingConsent: AssistantKind?

    private static let reviewerKey = "review.account_binding"
    private static let consentKey = "review.cross_provider_consent"

    var body: some View {
        SettingsPaneForm {
            SettingsScopeHeader(session: session, computer: $computer, settings: settings, recovery: recovery)

            Group {
                Section("Safety Review") {
                    SettingToggleRow(
                        settings: settings,
                        key: "review.automatic",
                        title: String(localized: "Review permission requests automatically"),
                        caption: String(localized: "When an assistant asks for permission, a safety reviewer can allow the request once or block it. Anything it can't review waits for you."),
                        computerName: computerName
                    )
                    reviewerRow
                    consentRow
                }

                Section {
                    SettingCountRow(
                        settings: settings,
                        key: "energy.concurrency",
                        title: String(localized: "Run up to"),
                        unit: String(localized: "tasks at once"),
                        minimum: 1,
                        computerName: computerName
                    )
                    SettingCountRow(
                        settings: settings,
                        key: "energy.low_power_concurrency",
                        title: String(localized: "On battery or in Low Power Mode, run up to"),
                        unit: String(localized: "tasks"),
                        minimum: 0,
                        computerName: computerName
                    )
                } header: {
                    Text("Work Limits")
                } footer: {
                    Text("Tasks over the limit wait their turn. Nothing running is stopped.")
                        .settingsCaption()
                }
            }
            .disabled(settings.isUnreachable)

            SettingsNoticeSection(notice: settings.notice)
        }
        .confirmationDialog(
            consentDialogTitle,
            isPresented: Binding(
                get: { pendingConsent != nil },
                set: { if !$0 { pendingConsent = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Turn On") {
                guard let bindingID = reviewerBindingID else { return }
                pendingConsent = nil
                Task { await settings.setSetting(Self.consentKey, value: .text(bindingID), scope: .plane) }
            }
            Button("Cancel", role: .cancel) { pendingConsent = nil }
        } message: {
            Text(consentDialogMessage)
        }
    }

    private var computerName: String { session.computerName(computer) }

    // MARK: Reviewer

    @ViewBuilder
    private var reviewerRow: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let current = text(Self.reviewerKey) {
                Picker("Reviewer", selection: Binding(
                    get: { current },
                    set: { value in
                        Task { await settings.setSetting(Self.reviewerKey, value: .text(value), scope: .plane) }
                    }
                )) {
                    Text("Each task's own assistant").tag("")
                    ForEach(reviewerBindings) { binding in
                        Text(reviewerTitle(binding)).tag(Self.bindingValue(binding.id))
                    }
                    if !current.isEmpty, !reviewerBindings.contains(where: { Self.bindingValue($0.id) == current }) {
                        Text("A removed sign-in").tag(current)
                    }
                }
                .pickerStyle(.menu)
                .tint(JetDesign.accentText)
                .disabled(settings.isSaving(Self.reviewerKey, scope: .plane))
            } else {
                SettingValueUnavailable(
                    title: String(localized: "Reviewer"),
                    isLoading: settings.isLoading,
                    computerName: computerName
                )
            }
            SettingRowFooter(settings: settings, key: Self.reviewerKey, scope: .plane)
        }
    }

    // MARK: Cross-provider consent

    @ViewBuilder
    private var consentRow: some View {
        if let reviewer, let bindingID = reviewerBindingID, let consent = text(Self.consentKey) {
            VStack(alignment: .leading, spacing: 4) {
                Toggle(isOn: Binding(
                    get: { consent == bindingID },
                    set: { isOn in
                        if isOn {
                            // Sending request details to another company is confirmed first.
                            pendingConsent = reviewer
                        } else {
                            Task { await settings.setSetting(Self.consentKey, value: .text(""), scope: .plane) }
                        }
                    }
                )) {
                    Text(consentTitle(for: reviewer))
                }
                .disabled(settings.isSaving(Self.consentKey, scope: .plane))
                SettingRowFooter(settings: settings, key: Self.consentKey, scope: .plane)
            }
        }
    }

    private func consentTitle(for reviewer: AssistantKind) -> String {
        switch reviewer {
        case .claudeCode:
            String(localized: "Let Claude review requests from Codex tasks (request details are sent to Anthropic)")
        case .codex:
            String(localized: "Let Codex review requests from Claude Code tasks (request details are sent to OpenAI)")
        }
    }

    private var consentDialogTitle: String {
        switch pendingConsent {
        case .codex: String(localized: "Send Claude Code request details to OpenAI?")
        case .claudeCode, nil: String(localized: "Send Codex request details to Anthropic?")
        }
    }

    private var consentDialogMessage: String {
        switch pendingConsent {
        case .codex:
            String(localized: "When a Claude Code task asks for permission, Jet sends the request and recent messages from that task to OpenAI so Codex can review it.")
        case .claudeCode, nil:
            String(localized: "When a Codex task asks for permission, Jet sends the request and recent messages from that task to Anthropic so Claude can review it.")
        }
    }

    // MARK: Values

    private func text(_ key: String) -> String? {
        guard case let .text(value) = settings.settingValue(key, scope: .plane) else { return nil }
        return value
    }

    private var reviewerBindings: [JetAccountBindingSummary] {
        (settings.accounts?.bindings ?? []).filter { AssistantKind(provider: $0.provider) != nil }
    }

    /// The chosen reviewer's sign-in, when a reviewer is chosen.
    private var reviewerBindingID: String? {
        guard let value = text(Self.reviewerKey), !value.isEmpty else { return nil }
        return value
    }

    private var reviewer: AssistantKind? {
        guard let bindingID = reviewerBindingID,
              let binding = reviewerBindings.first(where: { Self.bindingValue($0.id) == bindingID })
        else { return nil }
        return AssistantKind(provider: binding.provider)
    }

    private func reviewerTitle(_ binding: JetAccountBindingSummary) -> String {
        let name = AssistantKind(provider: binding.provider)?.name ?? binding.label
        return String(localized: "\(name) sign-in")
    }

    /// The core spells binding IDs in lowercase.
    static func bindingValue(_ id: UUID) -> String {
        id.uuidString.lowercased()
    }
}
