import SwiftUI

/// Shown first in Settings › Advanced while the store is read-only: the backups
/// Jet can restore from.
struct JetRecoverySection: View {
    @Bindable var model: JetRecoveryModel
    let computerName: String
    @State private var pendingSnapshot: JetRecoverySnapshot?

    var body: some View {
        if let health = model.health, model.isReadOnly {
            Section {
                VStack(alignment: .leading, spacing: 4) {
                    SettingsStatusLabel(
                        text: String(localized: "Jet paused changes to protect your data."),
                        systemImage: "exclamationmark.triangle.fill",
                        tint: .orange
                    )
                    .fontWeight(.medium)
                    Text("Restore a backup to continue. Your tasks stay readable.")
                        .settingsCaption()
                }
                if health.snapshots.isEmpty {
                    Text("No backups to restore from.")
                        .foregroundStyle(.secondary)
                }
                ForEach(health.snapshots) { snapshot in
                    HStack {
                        Text(BackupPresentation.line(snapshot))
                        Spacer()
                        Button("Restore…") { pendingSnapshot = snapshot }
                            .disabled(!canRestore(health))
                    }
                }
                if health.deletionLedger == "corrupt" {
                    SettingsStatusLabel(
                        text: String(localized: "Restoring is unavailable because Jet's record of deleted tasks is damaged."),
                        systemImage: "exclamationmark.triangle.fill",
                        tint: .orange
                    )
                    .font(.system(size: JetDesign.TextSize.control))
                }
                if let issue = model.issues[.health] {
                    SettingsIssueRow(error: issue, sentence: String(localized: "Couldn't restore the backup."))
                }
            } header: {
                Text("Data Protection")
            }
            .confirmationDialog(
                pendingTitle,
                isPresented: Binding(get: { pendingSnapshot != nil }, set: { if !$0 { pendingSnapshot = nil } }),
                titleVisibility: .visible
            ) {
                if let snapshot = pendingSnapshot {
                    Button("Restore", role: .destructive) {
                        pendingSnapshot = nil
                        Task { await model.restoreSnapshot(snapshot) }
                    }
                }
                Button("Cancel", role: .cancel) { pendingSnapshot = nil }
            } message: {
                Text("Jet replaces its data on \(computerName) with this backup. Changes made after it may be lost. The damaged data is kept aside.")
            }
        }
    }

    private func canRestore(_ health: JetSystemHealth) -> Bool {
        model.operation == nil && model.issues[.health] == nil && health.deletionLedger != "corrupt"
    }

    private var pendingTitle: String {
        guard let pendingSnapshot else { return String(localized: "Restore the backup?") }
        return String(localized: "Restore the backup from \(BackupPresentation.date(pendingSnapshot.takenAt))?")
    }
}

/// The background service on one computer: versions, tools, sign-in storage,
/// assistants, and backups while the store is serving.
struct JetServiceSection: View {
    @Bindable var model: JetRecoveryModel
    let computerName: String
    let isLocal: Bool
    let diskPressure: Bool
    @State private var confirmsPurge = false

    var body: some View {
        Section("Background Service") {
            if let health = model.health {
                LabeledContent("Jet service", value: health.daemonVersion)
                LabeledContent("Platform", value: health.platform)
                LabeledContent("Running since", value: JetCopy.dateTime(health.daemonStartedAt))
                LabeledContent("Git", value: gitVersion(health))
                LabeledContent("Keychain", value: keychainLabel(health.credentialStore))
                ForEach(health.crafts) { craft in
                    LabeledContent("Craft \(craft.id)", value: craft.version)
                }
                if !health.degradedCapabilities.isEmpty {
                    SettingsStatusLabel(
                        text: String(localized: "Limited: \(health.degradedCapabilities.joined(separator: ", "))"),
                        systemImage: "exclamationmark.triangle.fill",
                        tint: .orange
                    )
                }
            } else if model.isLoading {
                Text("Loading…").foregroundStyle(.secondary)
            }
            if diskPressure {
                SettingsStatusLabel(
                    text: isLocal
                        ? String(localized: "Your Mac is almost out of disk space. Jet paused new work.")
                        : String(localized: "\(computerName) is almost out of disk space. Jet paused new work."),
                    systemImage: "externaldrive.badge.exclamationmark",
                    tint: .orange
                )
            }
            if let issue = model.issues[.health], !model.isReadOnly {
                SettingsIssueRow(error: issue, sentence: model.health == nil
                    ? String(localized: "Couldn't check the background service.")
                    : String(localized: "Showing what Jet saw last."))
            }
        }

        if let health = model.health, health.recoveryState == "serving" {
            Section("Backups") {
                if let latest = health.snapshots.first {
                    LabeledContent("Latest backup", value: BackupPresentation.line(latest))
                }
                LabeledContent("Backups", value: JetCopy.number(health.snapshots.count))
                if health.auditIntegrity == .trusted, !health.snapshots.isEmpty {
                    Button("Remove Older Backups…") { confirmsPurge = true }
                        .disabled(model.operation != nil || model.issues[.health] != nil)
                }
            }
            .confirmationDialog(
                "Remove older backups?",
                isPresented: $confirmsPurge,
                titleVisibility: .visible
            ) {
                Button("Remove", role: .destructive) {
                    Task { await model.purgeSnapshots() }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Jet makes a fresh backup first. Older backups from before tasks were deleted can't be restored later.")
            }
        }
    }

    private func gitVersion(_ health: JetSystemHealth) -> String {
        guard let git = health.externalTools.first(where: { $0.tool == "git" }) else {
            return String(localized: "Not reported")
        }
        switch git.availability {
        case let .present(version): return version
        case .missing: return String(localized: "Not installed")
        }
    }

    private func keychainLabel(_ state: JetCredentialStoreState) -> String {
        switch state {
        case .available: String(localized: "Ready")
        case .locked: String(localized: "Locked")
        case .unavailable: String(localized: "Can't reach it")
        }
    }
}

/// Security Audit: how long records are kept, whether they are trusted, and
/// the records themselves.
struct JetAuditSection: View {
    @Bindable var model: JetRecoveryModel
    let settings: JetSettingsModel
    let computerName: String
    @State private var showsReferences = false

    var body: some View {
        Section("Security Audit") {
            SettingCountRow(
                settings: settings,
                key: "security.audit_retention_days",
                title: String(localized: "Keep audit records for"),
                unit: String(localized: "days"),
                minimum: 90,
                computerName: computerName
            )
            LabeledContent("Status") {
                if let health = model.health {
                    switch health.auditIntegrity {
                    case .trusted:
                        SettingsStatusLabel(text: String(localized: "Trusted"), systemImage: "checkmark.shield.fill", tint: .green)
                    case .degraded:
                        SettingsStatusLabel(text: String(localized: "Needs review"), systemImage: "exclamationmark.shield.fill", tint: .orange)
                    case .unavailable:
                        Text("Not reported").foregroundStyle(.secondary)
                    }
                } else {
                    Text(model.isLoading ? String(localized: "Loading…") : String(localized: "Not reported"))
                        .foregroundStyle(.secondary)
                }
            }
            if case let .degraded(reason)? = model.health?.auditIntegrity {
                Text("Some audit records don't add up (\(reason)). Keep the evidence and review it before relying on the audit.")
                    .settingsCaption()
            }
            DisclosureGroup("Audit Records") {
                VStack(alignment: .leading, spacing: 8) {
                    Toggle("Show what each record is about", isOn: $showsReferences)
                        .font(.system(size: JetDesign.TextSize.control))
                    if model.isLoadingAudit && model.auditEntries.isEmpty {
                        Text("Loading…").foregroundStyle(.secondary)
                    }
                    if model.auditEntries.isEmpty && model.issues[.audit] == nil && !model.isLoadingAudit {
                        Text("No audit records yet.").foregroundStyle(.secondary)
                    }
                    ForEach(model.auditEntries) { entry in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(entry.decision.replacingOccurrences(of: "_", with: " "))
                            Text("\(JetCopy.dateTime(entry.recordedAt)) · \(entry.actor) · \(entry.outcome) · \(entry.risk)")
                                .settingsCaption()
                            if showsReferences {
                                Text("\(entry.targetKind): \(entry.targetReference)")
                                    .font(.system(size: JetDesign.TextSize.metadata, design: .monospaced))
                                    .textSelection(.enabled)
                            }
                        }
                    }
                    if model.hasMoreAudit {
                        Button("Load More") { Task { await model.loadMoreAudit() } }
                            .disabled(model.isLoadingAudit)
                    }
                    if let issue = model.issues[.audit] {
                        SettingsIssueRow(error: issue, sentence: String(localized: "Couldn't load audit records."))
                    }
                }
                .padding(.top, 6)
            }
        }
    }
}

/// Backup rows: "Sep 26, 09:14 · Daily · 412 KB".
enum BackupPresentation {
    static func line(_ snapshot: JetRecoverySnapshot) -> String {
        let size = JetCopy.byteCount(Int64(clamping: snapshot.bytes))
        return [date(snapshot.takenAt), reason(snapshot.reason), size].joined(separator: " · ")
    }

    static func date(_ date: Date) -> String {
        date.formatted(.dateTime.month(.abbreviated).day().hour().minute().locale(JetCopy.uiLocale))
    }

    static func reason(_ reason: String) -> String {
        switch reason {
        case "daily": String(localized: "Daily")
        case "migration": String(localized: "Before an update")
        case "maintenance": String(localized: "Before maintenance")
        default: reason.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }
}

/// A summary for bug reports, built from versions, states and stable codes only.
/// It never includes computer names, SSH addresses, task titles, paths or
/// error messages.
enum JetDiagnosticSummary {
    static func make(
        appVersion: String?,
        isLocal: Bool,
        connection: JetConnectionState,
        health: JetSystemHealth?,
        capabilities: JetCapabilitySummary?,
        errors: [JetPresentationError]
    ) -> String {
        var lines = ["Jet diagnostic summary"]
        lines.append("App: \(appVersion.map(token) ?? "unknown")")
        lines.append("Computer: \(isLocal ? "this Mac" : "another computer")")
        lines.append("Connection: \(connectionLabel(connection))")
        if let health {
            lines.append("Service: \(token(health.daemonVersion)) · \(token(health.platform)) · \(health.daemonStarts) starts")
            lines.append("Store: \(token(health.recoveryState ?? "not reported"))\(health.recoveryReason.map { " (\(token($0)))" } ?? "")")
            lines.append("Deletion ledger: \(token(health.deletionLedger ?? "not reported"))")
            lines.append("Security audit: \(auditLabel(health.auditIntegrity))")
            lines.append("Backups: \(health.snapshots.count)")
            lines.append("Keychain: \(health.credentialStore.rawValue)")
            let tools = health.externalTools.map { tool in
                switch tool.availability {
                case let .present(version): "\(token(tool.tool)) \(token(version))"
                case .missing: "\(token(tool.tool)) missing"
                }
            }
            lines.append("Tools: \(tools.isEmpty ? "none" : tools.joined(separator: ", "))")
            let crafts = health.crafts.map { "\(token($0.id)) \(token($0.version))" }
            lines.append("Crafts: \(crafts.isEmpty ? "none" : crafts.joined(separator: ", "))")
            let degraded = health.degradedCapabilities.map(token)
            lines.append("Degraded: \(degraded.isEmpty ? "none" : degraded.joined(separator: ", "))")
        } else if let capabilities {
            lines.append("Service: \(token(capabilities.coreVersion)) · \(token(capabilities.platform))")
            lines.append("Keychain: \(capabilities.credentialStore.rawValue)")
        } else {
            lines.append("Service: not reported")
        }
        let codes = Set(errors.map(\.code).filter(isStableCode)).sorted()
        lines.append("Recent error codes: \(codes.isEmpty ? "none" : codes.joined(separator: ", "))")
        return lines.joined(separator: "\n")
    }

    /// Stable codes look like `storage.disk_pressure`.
    static func isStableCode(_ code: String) -> Bool {
        guard !code.isEmpty, code.count <= 80 else { return false }
        let allowed = Set("abcdefghijklmnopqrstuvwxyz0123456789_.")
        return code.allSatisfy { allowed.contains($0) } && !code.hasPrefix(".") && !code.hasSuffix(".")
    }

    /// Keeps only the leading version-like text, so nothing else can slip into
    /// the summary: "2.0 (alex@host)" becomes "2.0".
    private static func token(_ value: String) -> String {
        let allowed = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._+-")
        let kept = String(value.prefix(64).prefix { allowed.contains($0) })
        return kept.isEmpty ? "unknown" : kept
    }

    private static func connectionLabel(_ state: JetConnectionState) -> String {
        switch state {
        case .disconnected: "disconnected"
        case .connecting: "connecting"
        case let .connected(negotiation): "connected (protocol \(negotiation.protocolVersion).\(negotiation.minorVersion))"
        case let .reconnecting(attempt): "reconnecting (attempt \(attempt))"
        case let .failed(error): "failed (\(isStableCode(error.code) ? error.code : "unknown"))"
        }
    }

    private static func auditLabel(_ integrity: JetAuditIntegrity) -> String {
        switch integrity {
        case .trusted: "trusted"
        case let .degraded(reason): "degraded (\(token(reason)))"
        case .unavailable: "not reported"
        }
    }
}
