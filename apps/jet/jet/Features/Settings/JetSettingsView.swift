import SwiftUI

/// The Settings window: six tabs, each a grouped form. Settings never follows the
/// main window's selection; panes that belong to one computer use the computer
/// chosen here (`jet.settings.computer`), This Mac by default.
struct JetSettingsView: View {
    let session: DesktopSession
    @State private var settings: JetSettingsModel
    @State private var recovery: JetRecoveryModel
    @State private var pairing: ComputerPairingModel
    @AppStorage("jet.settings.last-pane") private var selectedPane = JetSettingsPane.general.rawValue
    @AppStorage("jet.settings.computer") private var storedComputer = ""

    init(session: DesktopSession) {
        self.session = session
        _settings = State(initialValue: JetSettingsModel(
            planeAccess: { try await session.settingsAccess(for: $0) }
        ))
        _recovery = State(initialValue: JetRecoveryModel(
            makeAccess: session.recoveryAccess(for:),
            onConversationChange: { _ in await session.loadConversations() },
            onSnapshotRestored: {
                // Clears the main window's data-protection banner.
                await session.loadSetup()
                await session.loadConversations()
            }
        ))
        _pairing = State(initialValue: ComputerPairingModel(
            makeAccess: { try await session.pairingAccess(for: $0) }
        ))
    }

    var body: some View {
        TabView(selection: paneBinding) {
            Tab(JetSettingsPane.general.title, systemImage: JetSettingsPane.general.symbol, value: JetSettingsPane.general) {
                GeneralSettingsPane(session: session)
                    .settingsPaneFrame()
            }
            Tab(JetSettingsPane.agents.title, systemImage: JetSettingsPane.agents.symbol, value: JetSettingsPane.agents) {
                AssistantsSettingsPane(session: session, settings: settings, recovery: recovery, computer: computerBinding)
                    .settingsPaneFrame()
            }
            Tab(JetSettingsPane.work.title, systemImage: JetSettingsPane.work.symbol, value: JetSettingsPane.work) {
                TasksSettingsPane(session: session, settings: settings, recovery: recovery, computer: computerBinding)
                    .settingsPaneFrame()
            }
            Tab(JetSettingsPane.connections.title, systemImage: JetSettingsPane.connections.symbol, value: JetSettingsPane.connections) {
                ComputersSettingsPane(session: session, pairing: pairing)
                    .settingsPaneFrame()
            }
            Tab(JetSettingsPane.safety.title, systemImage: JetSettingsPane.safety.symbol, value: JetSettingsPane.safety) {
                SafetySettingsPane(session: session, settings: settings, recovery: recovery, computer: computerBinding)
                    .settingsPaneFrame()
            }
            Tab(JetSettingsPane.advanced.title, systemImage: JetSettingsPane.advanced.symbol, value: JetSettingsPane.advanced) {
                AdvancedSettingsPane(session: session, settings: settings, recovery: recovery, computer: computerBinding)
                    .settingsPaneFrame()
            }
        }
        .tint(JetDesign.accent)
        .task(id: loadKey) { await load() }
        .onAppear(perform: applyRequestedPane)
        .onChange(of: session.requestedSettingsPane) { _, _ in applyRequestedPane() }
    }

    /// The chosen computer, or This Mac when it is gone.
    private var computer: UUID {
        if let id = UUID(uuidString: storedComputer), session.planes.contains(where: { $0.id == id }) {
            return id
        }
        return session.localPlaneRegistryID
    }

    private var computerBinding: Binding<UUID> {
        Binding(
            get: { computer },
            set: { storedComputer = $0.uuidString }
        )
    }

    /// Reloads when the computer, its connection or its assistants change.
    private var loadKey: String {
        let crafts = session.planeSetupSnapshot(for: computer)?.capabilities.crafts
            .map(\.id).joined(separator: ",") ?? ""
        return [
            computer.uuidString,
            Self.connectionToken(session.computerConnection(computer)),
            crafts,
        ].joined(separator: "|")
    }

    private static func connectionToken(_ state: JetConnectionState) -> String {
        switch state {
        case .disconnected: "disconnected"
        case .connecting: "connecting"
        case .connected: "connected"
        case .reconnecting: "reconnecting"
        case .failed: "failed"
        }
    }

    private func load() async {
        guard !session.isPreviewSession else { return }
        let computer = computer
        let capabilities = session.planeSetupSnapshot(for: computer)?.capabilities
        await settings.load(
            planeRegistryID: computer,
            crafts: capabilities?.crafts ?? [],
            authProviders: capabilities?.authProviders ?? []
        )
        await recovery.load(planeRegistryID: computer, conversationID: nil)
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

extension View {
    /// Settings panes share one width range: 640 pt, never wider than 760.
    func settingsPaneFrame() -> some View {
        frame(minWidth: 600, idealWidth: 640, maxWidth: 760, minHeight: 460)
    }

    /// Secondary metadata under a row, at the 11-pt metadata size.
    func settingsCaption() -> some View {
        font(.system(size: JetDesign.TextSize.metadata))
            .foregroundStyle(.secondary)
    }
}

// MARK: - Shared rows

/// A settings tab: a grouped form without an in-pane title.
struct SettingsPaneForm<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        Form { content }
            .formStyle(.grouped)
    }
}

/// The first section of a pane that belongs to one computer: "Settings for" when
/// there are two or more computers, and the offline banner.
struct SettingsScopeHeader: View {
    let session: DesktopSession
    @Binding var computer: UUID
    let settings: JetSettingsModel
    let recovery: JetRecoveryModel

    var body: some View {
        if session.planes.count >= 2 || settings.isUnreachable {
            Section {
                if session.planes.count >= 2 {
                    Picker("Settings for", selection: $computer) {
                        ForEach(session.planes) { plane in
                            Text(plane.name).tag(plane.id)
                        }
                    }
                    .pickerStyle(.menu)
                    .tint(JetDesign.accentText)
                    .accessibilityIdentifier("settings-computer-picker")
                }
                if settings.isUnreachable {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Label {
                            Text("Can't reach \(session.computerName(computer)). Settings can't be changed right now.")
                                .fixedSize(horizontal: false, vertical: true)
                        } icon: {
                            Image(systemName: "wifi.slash")
                                .foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 8)
                        Button("Try Again") {
                            Task {
                                await session.reconnectComputer(computer)
                                await settings.refresh()
                                await recovery.refresh()
                            }
                        }
                        .buttonStyle(.bordered)
                    }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("settings-offline-banner")
                }
            }
        }
    }
}

/// A switch that saves as soon as it changes.
struct SettingToggleRow: View {
    let settings: JetSettingsModel
    let key: String
    let title: String
    var caption: String?
    let computerName: String
    var scope: JetSettingScope = .plane

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let current {
                Toggle(isOn: Binding(
                    get: { current },
                    set: { value in
                        Task { await settings.setSetting(key, value: .flag(value), scope: scope) }
                    }
                )) {
                    Text(title)
                    if let caption { Text(caption) }
                }
                .disabled(settings.isSaving(key, scope: scope))
            } else {
                SettingValueUnavailable(title: title, isLoading: settings.isLoading, computerName: computerName)
            }
            SettingRowFooter(settings: settings, key: key, scope: scope)
        }
    }

    private var current: Bool? {
        guard case let .flag(value) = settings.settingValue(key, scope: scope) else { return nil }
        return value
    }
}

/// A whole number that saves on Return or when the field loses focus. An invalid
/// number shows why and sends nothing.
struct SettingCountRow: View {
    let settings: JetSettingsModel
    let key: String
    let title: String
    let unit: String
    let minimum: UInt32
    var caption: String?
    let computerName: String
    var scope: JetSettingScope = .plane

    @State private var draft = ""
    @State private var isInvalid = false
    @FocusState private var isFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let current {
                LabeledContent {
                    HStack(spacing: 6) {
                        TextField(title, text: $draft)
                            .labelsHidden()
                            .textFieldStyle(.roundedBorder)
                            .multilineTextAlignment(.trailing)
                            .frame(width: 64)
                            .focused($isFocused)
                            .onSubmit { commit(current: current) }
                            .disabled(settings.isSaving(key, scope: scope))
                            // The unit goes in the label, so VoiceOver still reads the number.
                            .accessibilityLabel(Text(verbatim: "\(title) (\(unit))"))
                        Text(unit)
                            .foregroundStyle(.secondary)
                            .accessibilityHidden(true)
                    }
                } label: {
                    Text(title)
                    if let caption { Text(caption) }
                }
                if isInvalid {
                    SettingValidationMessage(text: String(localized: "Enter a whole number of at least \(Int(minimum))."))
                }
            } else {
                SettingValueUnavailable(title: title, isLoading: settings.isLoading, computerName: computerName)
            }
            SettingRowFooter(settings: settings, key: key, scope: scope)
        }
        .onAppear(perform: syncDraft)
        .onChange(of: current) { _, _ in syncDraft() }
        .onChange(of: isFocused) { _, focused in
            if !focused, let current { commit(current: current) }
        }
    }

    private var current: UInt32? {
        guard case let .count(value) = settings.settingValue(key, scope: scope) else { return nil }
        return value
    }

    private func syncDraft() {
        guard let current else { return }
        draft = String(current)
        isInvalid = false
    }

    private func commit(current: UInt32) {
        guard let value = SettingNumberInput.parse(draft), value >= minimum else {
            isInvalid = true
            return
        }
        isInvalid = false
        draft = String(value)
        guard value != current else { return }
        Task { await settings.setSetting(key, value: .count(value), scope: scope) }
    }
}

/// A size in MiB, shown as a number with an MB or GB menu.
struct SettingSizeRow: View {
    let settings: JetSettingsModel
    let key: String
    let title: String
    let minimum: UInt32
    let computerName: String
    var scope: JetSettingScope = .plane

    @State private var draft = ""
    @State private var unit = StorageSize.Unit.megabytes
    @State private var isInvalid = false
    @FocusState private var isFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let current {
                LabeledContent(title) {
                    HStack(spacing: 6) {
                        TextField(title, text: $draft)
                            .labelsHidden()
                            .textFieldStyle(.roundedBorder)
                            .multilineTextAlignment(.trailing)
                            .frame(width: 64)
                            .focused($isFocused)
                            .onSubmit { commit(current: current) }
                            .accessibilityLabel(Text(title))
                        Picker("Unit", selection: Binding(
                            get: { unit },
                            set: { newUnit in
                                unit = newUnit
                                commit(current: current)
                            }
                        )) {
                            ForEach(StorageSize.Unit.allCases) { unit in
                                Text(unit.title).tag(unit)
                            }
                        }
                        .labelsHidden()
                        .pickerStyle(.menu)
                        .tint(JetDesign.accentText)
                        .fixedSize()
                    }
                    .disabled(settings.isSaving(key, scope: scope))
                }
                if isInvalid {
                    SettingValidationMessage(text: minimum == 0
                        ? String(localized: "Enter a whole number.")
                        : String(localized: "Enter a whole number of at least \(Int(minimum)) MB."))
                }
            } else {
                SettingValueUnavailable(title: title, isLoading: settings.isLoading, computerName: computerName)
            }
            SettingRowFooter(settings: settings, key: key, scope: scope)
        }
        .onAppear(perform: syncDraft)
        .onChange(of: current) { _, _ in syncDraft() }
        .onChange(of: isFocused) { _, focused in
            if !focused, let current { commit(current: current) }
        }
    }

    private var current: UInt32? {
        guard case let .count(value) = settings.settingValue(key, scope: scope) else { return nil }
        return value
    }

    private func syncDraft() {
        guard let current else { return }
        let size = StorageSize(mebibytes: current)
        draft = String(size.amount)
        unit = size.unit
        isInvalid = false
    }

    private func commit(current: UInt32) {
        guard let amount = SettingNumberInput.parse(draft),
              let value = StorageSize(amount: amount, unit: unit).mebibytes,
              value >= minimum
        else {
            isInvalid = true
            return
        }
        isInvalid = false
        guard value != current else { return }
        Task { await settings.setSetting(key, value: .count(value), scope: scope) }
    }
}

/// Parses what a person typed into a number field.
enum SettingNumberInput {
    static func parse(_ text: String, locale: Locale = JetCopy.uiLocale) -> UInt32? {
        var trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if let separator = locale.groupingSeparator, !separator.isEmpty {
            trimmed = trimmed.replacingOccurrences(of: separator, with: "")
        }
        trimmed = trimmed.replacingOccurrences(of: "\u{00A0}", with: "")
        guard !trimmed.isEmpty, trimmed.allSatisfy({ $0.isASCII && $0.isNumber }) else { return nil }
        return UInt32(trimmed)
    }
}

/// Saving…, Saved, a failure, or an unconfirmed change, then Reset to Default.
struct SettingRowFooter: View {
    let settings: JetSettingsModel
    let key: String
    let scope: JetSettingScope

    var body: some View {
        if settings.isSaving(key, scope: scope) {
            SettingProgressLine(text: String(localized: "Saving…"))
        } else if settings.isChecking(key, scope: scope) {
            SettingProgressLine(text: String(localized: "Checking…"))
        } else {
            switch settings.rowStatus(key, scope: scope) {
            case .saved?:
                Label("Saved", systemImage: "checkmark")
                    .settingsCaption()
                    .task {
                        try? await Task.sleep(for: .seconds(3))
                        guard !Task.isCancelled,
                              settings.rowStatus(key, scope: scope) == .saved
                        else { return }
                        settings.clearRowStatus(key, scope: scope)
                    }
            case let .failed(error)?:
                SettingsIssueRow(error: error)
            case .unconfirmed?:
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Label {
                        Text("Jet couldn't confirm this change.")
                    } icon: {
                        Image(systemName: "questionmark.circle")
                            .symbolRenderingMode(.hierarchical)
                            .foregroundStyle(.orange)
                    }
                    Button("Check Again") {
                        Task { await settings.checkAgain(key, scope: scope) }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                }
                .font(.system(size: JetDesign.TextSize.control))
            case nil:
                EmptyView()
            }
            if settings.isExplicit(key, scope: scope), let label = JetSettingDefaults.label(key) {
                Button("Reset to Default (\(label))") {
                    Task { await settings.clearSetting(key, scope: scope) }
                }
                .buttonStyle(.plain)
                .foregroundStyle(JetDesign.accentText)
                .font(.system(size: JetDesign.TextSize.metadata))
            }
        }
    }
}

private struct SettingProgressLine: View {
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            ProgressView()
                .controlSize(.small)
            Text(text)
        }
        .settingsCaption()
    }
}

struct SettingValidationMessage: View {
    let text: String

    var body: some View {
        Label {
            Text(text)
        } icon: {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
        }
        .font(.system(size: JetDesign.TextSize.metadata))
    }
}

/// "Loading…" while the computer answers, then "Not available on Studio Mac".
struct SettingValueUnavailable: View {
    let title: String
    let isLoading: Bool
    let computerName: String

    var body: some View {
        LabeledContent(title) {
            Text(isLoading ? String(localized: "Loading…") : String(localized: "Not available on \(computerName)"))
                .foregroundStyle(.secondary)
        }
    }
}

/// One sentence for a failure, with the message and code under Details.
struct SettingsIssueRow: View {
    let error: JetPresentationError
    var sentence: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Label {
                Text(sentence ?? SettingsErrorCopy.sentence(error))
                    .fixedSize(horizontal: false, vertical: true)
            } icon: {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.orange)
            }
            DisclosureGroup("Details") {
                VStack(alignment: .leading, spacing: 2) {
                    Text(error.message)
                    Text(error.code)
                        .font(.system(size: JetDesign.TextSize.metadata, design: .monospaced))
                }
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .font(.system(size: JetDesign.TextSize.control))
    }
}

/// A notice at the bottom of a pane, such as "Repeats daily."
struct SettingsNoticeSection: View {
    let notice: String?
    var identifier = "settings-notice"

    var body: some View {
        if let notice {
            Section {
                Label {
                    Text(notice)
                } icon: {
                    Image(systemName: "info.circle")
                        .foregroundStyle(.secondary)
                }
                .font(.system(size: JetDesign.TextSize.control))
                .accessibilityIdentifier(identifier)
            }
        }
    }
}

/// A status as a symbol and text; colour is never the only signal.
struct SettingsStatusLabel: View {
    let text: String
    let systemImage: String
    var tint: Color = .secondary

    var body: some View {
        Label {
            Text(text)
        } icon: {
            Image(systemName: systemImage)
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(tint)
        }
    }
}
