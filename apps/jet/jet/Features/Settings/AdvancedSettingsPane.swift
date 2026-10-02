import SwiftUI

/// Settings › Advanced: data protection, the background service, storage,
/// extensions, diagnostics and the security audit. Jet's own terms (Craft, Run)
/// may appear here.
struct AdvancedSettingsPane: View {
    let session: DesktopSession
    let settings: JetSettingsModel
    let recovery: JetRecoveryModel
    @Binding var computer: UUID

    @State private var copiedDiagnostics = false

    var body: some View {
        SettingsPaneForm {
            SettingsScopeHeader(session: session, computer: $computer, settings: settings, recovery: recovery)

            // Restoring a backup is the way out of read-only mode, so it comes first
            // and stays available even while other changes are paused.
            JetRecoverySection(model: recovery, computerName: computerName)

            Group {
                JetServiceSection(
                    model: recovery,
                    computerName: computerName,
                    isLocal: session.isLocalPlane(computer),
                    diskPressure: diskPressure
                )

                Group {
                    Section {
                        SettingSizeRow(
                            settings: settings,
                            key: "storage.disposable_mib",
                            title: String(localized: "Temporary files"),
                            minimum: 0,
                            computerName: computerName
                        )
                        SettingSizeRow(
                            settings: settings,
                            key: "artifact.max_mib",
                            title: String(localized: "Largest file a task can save"),
                            minimum: 1,
                            computerName: computerName
                        )
                        SettingSizeRow(
                            settings: settings,
                            key: "artifact.run_mib",
                            title: String(localized: "Files per assistant session"),
                            minimum: 1,
                            computerName: computerName
                        )
                    } header: {
                        Text("Storage")
                    } footer: {
                        Text("Jet stops adding files at these sizes. Nothing you keep is deleted.")
                            .settingsCaption()
                    }

                    Section("Limits") {
                        SettingToggleRow(
                            settings: settings,
                            key: "energy.constrained",
                            title: String(localized: "Always use the low-power limit"),
                            computerName: computerName
                        )
                        SettingToggleRow(
                            settings: settings,
                            key: "energy.foreground_override",
                            title: String(localized: "Let messages you send go past the limit"),
                            computerName: computerName
                        )
                    }

                    ExtensionsSection(settings: settings, computerName: computerName)
                }
                // Every write fails while Jet protects its data; only Restore helps.
                .disabled(recovery.isReadOnly)

                Section("Diagnostics") {
                    HStack(spacing: 12) {
                        Button("Copy Diagnostic Summary", action: copyDiagnostics)
                            .accessibilityIdentifier("copy-diagnostics")
                        if copiedDiagnostics {
                            SettingsStatusLabel(text: String(localized: "Copied."), systemImage: "checkmark")
                                .settingsCaption()
                                .task {
                                    try? await Task.sleep(for: .seconds(3))
                                    copiedDiagnostics = false
                                }
                        }
                    }
                    Text("Versions, states and error codes only. It never includes task names, messages, files or addresses.")
                        .settingsCaption()
                }

                JetAuditSection(model: recovery, settings: settings, computerName: computerName)
                    .disabled(recovery.isReadOnly)
            }
            .disabled(settings.isUnreachable)

            SettingsNoticeSection(notice: recovery.notice, identifier: "recovery-notice")
            SettingsNoticeSection(notice: settings.notice)
        }
    }

    private var computerName: String { session.computerName(computer) }

    private var plane: JetPlanePresentation? {
        session.planes.first { $0.id == computer }
    }

    /// Errors seen for this computer. The main window's errors count only when
    /// its open task is on the same computer.
    private var recentErrors: [JetPresentationError] {
        var errors: [JetPresentationError] = []
        if let failure = plane?.failure { errors.append(failure) }
        errors += settings.issues.values
        errors += recovery.issues.values
        if session.selectedPlaneRegistryID == computer {
            errors += [session.actionError, session.workError, session.workNoticeError, session.gitDeliveryError]
                .compactMap { $0 }
        }
        return errors
    }

    private var diskPressure: Bool {
        recentErrors.contains { $0.code == "storage.disk_pressure" }
    }

    private func copyDiagnostics() {
        let summary = JetDiagnosticSummary.make(
            appVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
            isLocal: session.isLocalPlane(computer),
            connection: session.computerConnection(computer),
            health: recovery.health,
            capabilities: session.planeSetupSnapshot(for: computer)?.capabilities,
            errors: recentErrors
        )
        DesktopSession.copyToPasteboard(summary)
        copiedDiagnostics = true
    }
}

/// Assistant extensions (skills, hooks, MCP servers and plugins) and Developer
/// Mode. Every change is inspected first and confirmed.
private struct ExtensionsSection: View {
    @Bindable var settings: JetSettingsModel
    let computerName: String

    var body: some View {
        Section("Extensions") {
            if settings.crafts.isEmpty {
                Text("No installed Craft supports extensions on \(computerName).")
                    .foregroundStyle(.secondary)
            } else {
                Picker("Craft", selection: $settings.selectedExtensionCraftID) {
                    ForEach(settings.crafts) { craft in
                        Text("\(craft.id) \(craft.version)").tag(craft.id)
                    }
                }
                .tint(JetDesign.accentText)
                Picker("Change", selection: $settings.extensionAction) {
                    ForEach(JetExtensionAction.allCases, id: \.self) { action in
                        Text(action.title).tag(action)
                    }
                }
                .tint(JetDesign.accentText)
                TextField("Extension", text: $settings.extensionID, prompt: Text("Name of the skill, hook, MCP server or plugin"))
                HStack {
                    Spacer()
                    Button("Review Change…") {
                        Task { await settings.inspectExtension() }
                    }
                    .disabled(
                        settings.operation != nil
                            || settings.selectedExtensionCraftID.isEmpty
                            || settings.extensionID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    )
                }
            }
            ForEach(settings.extensionCatalogs) { catalog in
                DisclosureGroup("\(catalog.harness) catalog") {
                    Text(catalog.nativeMetadata)
                        .font(.system(size: JetDesign.TextSize.metadata, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            if let change = settings.latestExtensionChange {
                LabeledContent("Last change") {
                    Text("\(change.action.title) \(change.extensionID) · \(change.state.replacingOccurrences(of: "_", with: " "))")
                }
            }
            if let issue = settings.issues[.extensions] {
                SettingsIssueRow(error: issue, sentence: issue.category == .outcomeUnknown
                    ? String(localized: "Jet couldn't confirm this change.")
                    : String(localized: "Couldn't change extensions."))
            }
            SettingToggleRow(
                settings: settings,
                key: "craft.developer_mode",
                title: String(localized: "Developer Mode"),
                caption: String(localized: "Allow reviewed local or source-built Crafts."),
                computerName: computerName
            )
        }
        .confirmationDialog(
            confirmationTitle,
            isPresented: Binding(
                get: { settings.pendingExtensionProposal != nil },
                set: { if !$0 { settings.pendingExtensionProposal = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button(settings.pendingExtensionProposal?.action.title ?? String(localized: "Continue"),
                   role: settings.pendingExtensionProposal?.action == .remove ? .destructive : nil) {
                Task { await settings.confirmExtensionChange() }
            }
            Button("Cancel", role: .cancel) { settings.pendingExtensionProposal = nil }
        } message: {
            Text("Skills can direct tools. Hooks, MCP servers and plugins run as your user. The change applies the next time the assistant starts.")
        }
    }

    private var confirmationTitle: String {
        guard let proposal = settings.pendingExtensionProposal else {
            return String(localized: "Review the extension change")
        }
        return String(localized: "\(proposal.action.title) \(proposal.extensionID)?")
    }
}
