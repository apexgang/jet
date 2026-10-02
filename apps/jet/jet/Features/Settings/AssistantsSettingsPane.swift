import SwiftUI

/// Settings › Assistants: Claude Code and Codex on one computer, their sign-ins
/// and how much of each usage limit is left.
struct AssistantsSettingsPane: View {
    let session: DesktopSession
    let settings: JetSettingsModel
    let recovery: JetRecoveryModel
    @Binding var computer: UUID

    var body: some View {
        SettingsPaneForm {
            SettingsScopeHeader(session: session, computer: $computer, settings: settings, recovery: recovery)

            Group {
                Section {
                    ForEach(AssistantKind.allCases) { assistant in
                        AssistantSettingsRow(
                            settings: settings,
                            assistant: assistant,
                            state: state(of: assistant),
                            computerName: computerName,
                            canAddSignIn: !keychainUnavailable
                        )
                    }
                    if keychainUnavailable {
                        SettingsStatusLabel(
                            text: String(localized: "Jet can't reach the keychain on \(computerName), so sign-ins can't be added."),
                            systemImage: "lock.trianglebadge.exclamationmark",
                            tint: .orange
                        )
                        .font(.system(size: JetDesign.TextSize.control))
                    }
                    if let issue = settings.issues[.accounts], !settings.isUnreachable {
                        SettingsIssueRow(error: issue, sentence: accountsSentence(issue))
                    }
                } footer: {
                    Text("Jet uses each assistant's own sign-in. It never sees your password.")
                        .settingsCaption()
                }

                Section("Usage") {
                    if windows.isEmpty {
                        if settings.usage == nil && settings.isLoading {
                            Text("Loading…").foregroundStyle(.secondary)
                        } else if let issue = settings.issues[.usage], settings.usage == nil, !settings.isUnreachable {
                            SettingsIssueRow(error: issue, sentence: String(localized: "Couldn't check usage."))
                        } else {
                            Text("Usage appears after an assistant finishes a reply.")
                                .foregroundStyle(.secondary)
                        }
                    } else {
                        ForEach(windows) { window in
                            UsageWindowRow(
                                assistant: AssistantKind(provider: window.provider)?.name ?? window.provider,
                                window: window
                            )
                        }
                    }
                }
            }
            .disabled(settings.isUnreachable)

            SettingsNoticeSection(notice: settings.notice)
        }
        .confirmationDialog(
            removalTitle,
            isPresented: Binding(
                get: { settings.pendingAccountRemoval != nil },
                set: { if !$0 { settings.pendingAccountRemoval = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Remove Sign-In", role: .destructive) {
                Task { await settings.confirmAccountRemoval() }
            }
            Button("Cancel", role: .cancel) { settings.pendingAccountRemoval = nil }
        } message: {
            Text(removalMessage)
        }
    }

    private var computerName: String { session.computerName(computer) }

    private var capabilities: JetCapabilitySummary? {
        session.planeSetupSnapshot(for: computer)?.capabilities
    }

    private var keychainUnavailable: Bool {
        capabilities?.credentialStore == .unavailable
    }

    private var windows: [JetQuotaWindowSummary] {
        settings.usage?.quotaWindows ?? []
    }

    private func state(of assistant: AssistantKind) -> AssistantSignInState {
        let harnesses = capabilities?.harnesses ?? []
        let installed = harnesses.contains { $0.lowercased().contains(assistant.harnessMatch) }
        guard installed else { return .missing }
        let bindings = (settings.accounts?.bindings ?? []).filter { $0.provider == assistant.provider }
        let binding = bindings.first { AssistantSignInState.isUsable($0.state) } ?? bindings.first
        guard let binding else { return .notSignedIn }
        switch binding.state {
        case "resolvable", "resolved_at_use": return .usable(binding)
        case "waiting_for_unlock": return .waitingForUnlock(binding)
        default: return .unavailable(binding)
        }
    }

    private func accountsSentence(_ error: JetPresentationError) -> String {
        error.category == .outcomeUnknown
            ? String(localized: "Jet couldn't confirm this change.")
            : String(localized: "Couldn't update sign-ins.")
    }

    private var removalName: String {
        settings.pendingAccountRemoval.flatMap { AssistantKind(provider: $0.provider)?.name }
            ?? String(localized: "assistant")
    }

    private var removalTitle: String {
        String(localized: "Remove the \(removalName) sign-in from Jet?")
    }

    private var removalMessage: String {
        String(localized: "Jet stops using it for usage and safety review on \(computerName). \(removalName) stays signed in and your account doesn't change.")
    }
}

/// The two assistants Jet knows by name.
enum AssistantKind: String, CaseIterable, Identifiable {
    case claudeCode
    case codex

    var id: Self { self }

    init?(provider: String) {
        switch provider {
        case "anthropic": self = .claudeCode
        case "openai": self = .codex
        default: return nil
        }
    }

    var name: String {
        switch self {
        case .claudeCode: "Claude Code"
        case .codex: "Codex"
        }
    }

    /// The short name used when it reviews another assistant's requests.
    var reviewerName: String {
        switch self {
        case .claudeCode: "Claude"
        case .codex: "Codex"
        }
    }

    var provider: String {
        switch self {
        case .claudeCode: "anthropic"
        case .codex: "openai"
        }
    }

    var company: String {
        switch self {
        case .claudeCode: "Anthropic"
        case .codex: "OpenAI"
        }
    }

    var harnessMatch: String {
        switch self {
        case .claudeCode: "claude"
        case .codex: "codex"
        }
    }

    var other: AssistantKind {
        self == .claudeCode ? .codex : .claudeCode
    }
}

enum AssistantSignInState {
    case usable(JetAccountBindingSummary)
    case notSignedIn
    case waitingForUnlock(JetAccountBindingSummary)
    case unavailable(JetAccountBindingSummary)
    case missing

    static func isUsable(_ state: String) -> Bool {
        state == "resolvable" || state == "resolved_at_use"
    }
}

private struct AssistantSettingsRow: View {
    let settings: JetSettingsModel
    let assistant: AssistantKind
    let state: AssistantSignInState
    let computerName: String
    let canAddSignIn: Bool

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(assistant.name)
                statusLine
                    .font(.system(size: JetDesign.TextSize.control))
            }
            Spacer(minLength: 8)
            if isWorking {
                ProgressView().controlSize(.small)
            }
            actionButton
        }
        .padding(.vertical, 2)
    }

    private var isWorking: Bool {
        settings.operation?.hasSuffix(assistant.provider) == true
    }

    @ViewBuilder
    private var statusLine: some View {
        switch state {
        case .usable:
            SettingsStatusLabel(
                text: String(localized: "Installed · Using your existing sign-in"),
                systemImage: "checkmark.circle.fill",
                tint: .green
            )
        case .notSignedIn:
            SettingsStatusLabel(
                text: String(localized: "Installed · Not signed in through Jet"),
                systemImage: "person.crop.circle"
            )
            .foregroundStyle(.secondary)
        case .waitingForUnlock:
            SettingsStatusLabel(
                text: String(localized: "Unlock your keychain to use this sign-in."),
                systemImage: "lock.fill",
                tint: .orange
            )
        case .unavailable:
            SettingsStatusLabel(
                text: String(localized: "Jet can't use this sign-in right now."),
                systemImage: "exclamationmark.triangle.fill",
                tint: .orange
            )
        case .missing:
            SettingsStatusLabel(
                text: String(localized: "Not installed on \(computerName)"),
                systemImage: "minus.circle"
            )
            .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private var actionButton: some View {
        switch state {
        case let .usable(binding), let .waitingForUnlock(binding):
            Button("Remove Sign-In…") { settings.pendingAccountRemoval = binding }
                .disabled(settings.operation != nil)
        case .notSignedIn, .unavailable:
            if let provider = settings.authProviders.first(where: { $0.provider == assistant.provider }) {
                Button("Use Existing Sign-In") {
                    Task { await settings.bindAccount(provider) }
                }
                .disabled(settings.operation != nil || !canAddSignIn)
            }
        case .missing:
            EmptyView()
        }
    }
}

private struct UsageWindowRow: View {
    let assistant: String
    let window: JetQuotaWindowSummary

    var body: some View {
        let presentation = UsageWindowPresentation(window)
        VStack(alignment: .leading, spacing: 4) {
            if let fraction = presentation.fraction {
                Text(assistant)
                    .accessibilityHidden(true)
                // The name is the row's title above; the gauge carries it only for VoiceOver.
                Gauge(value: fraction) {
                    EmptyView()
                }
                .gaugeStyle(.linearCapacity)
                .accessibilityLabel(Text(assistant))
                .tint(presentation.isNearLimit ? .orange : JetDesign.accent)
                .accessibilityValue(Text(presentation.text))
                Label {
                    Text(presentation.text)
                } icon: {
                    if presentation.isNearLimit {
                        Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                    }
                }
                .labelStyle(UsageLabelStyle(showsIcon: presentation.isNearLimit))
                .settingsCaption()
            } else {
                LabeledContent(assistant) {
                    Text(presentation.text)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(.vertical, 2)
    }
}

/// Shows the warning symbol only near the limit, so the text stays aligned.
private struct UsageLabelStyle: LabelStyle {
    let showsIcon: Bool

    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 4) {
            if showsIcon { configuration.icon }
            configuration.title
        }
    }
}
