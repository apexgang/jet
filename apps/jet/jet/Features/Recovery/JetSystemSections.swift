import SwiftUI

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
