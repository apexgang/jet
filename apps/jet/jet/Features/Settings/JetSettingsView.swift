import SwiftUI

struct JetSettingsView: View {
    let session: DesktopSession
    enum Pane: String, CaseIterable, Identifiable {
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
    }

    @AppStorage("jet.settings.last-pane") private var selectedPane = Pane.general.rawValue
    @AppStorage("jet.settings.restore-last-task") private var restoresLastTask = true

    var body: some View {
        TabView(selection: paneBinding) {
            GeneralSettingsPane(session: session, restoresLastTask: $restoresLastTask)
                .tabItem { Label(Pane.general.title, systemImage: Pane.general.symbol) }
                .tag(Pane.general)

            SetupSettingsPane(session: session, kind: .agents)
            .tabItem { Label(Pane.agents.title, systemImage: Pane.agents.symbol) }
            .tag(Pane.agents)

            SettingsPlaceholder(
                title: "Work",
                message: "Project defaults, delivery, schedules, and retention arrive in later slices.",
                symbol: Pane.work.symbol
            )
            .tabItem { Label(Pane.work.title, systemImage: Pane.work.symbol) }
            .tag(Pane.work)

            SetupSettingsPane(session: session, kind: .connections)
            .tabItem { Label(Pane.connections.title, systemImage: Pane.connections.symbol) }
            .tag(Pane.connections)

            SettingsPlaceholder(
                title: "Safety & System",
                message: "Execution policy, recovery, diagnostics, and storage controls arrive in Wave 3.",
                symbol: Pane.safety.symbol
            )
            .tabItem { Label(Pane.safety.title, systemImage: Pane.safety.symbol) }
            .tag(Pane.safety)
        }
        .frame(width: 620, height: 430)
    }

    private var paneBinding: Binding<Pane> {
        Binding(
            get: { Pane(rawValue: selectedPane) ?? .general },
            set: { selectedPane = $0.rawValue }
        )
    }
}

private struct SetupSettingsPane: View {
    enum Kind {
        case agents
        case connections
    }

    let session: DesktopSession
    let kind: Kind

    var body: some View {
        Form {
            switch kind {
            case .agents:
                Section("Harness access") {
                    if let snapshot = session.setupSnapshot {
                        if snapshot.accounts.bindings.isEmpty {
                            Text("No Harness accounts connected")
                                .foregroundStyle(.secondary)
                        } else {
                            ForEach(snapshot.accounts.bindings) { binding in
                                LabeledContent(binding.label, value: binding.stateLabel)
                            }
                        }
                        Text("Add Harness access from Projects in the main window.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    } else {
                        Text(session.planeConnectionLabel)
                            .foregroundStyle(.secondary)
                    }
                }
            case .connections:
                Section("Local Plane") {
                    LabeledContent("Status", value: session.planeConnectionLabel)
                    if let snapshot = session.setupSnapshot {
                        LabeledContent("Core", value: snapshot.capabilities.coreVersion)
                        LabeledContent("Platform", value: snapshot.capabilities.platform)
                    }
                }
                Section("Remote pairing") {
                    let count = session.setupSnapshot?.pairing.pairedClients ?? 0
                    LabeledContent("Paired clients", value: count.formatted())
                    Text("Pairing controls arrive in Wave 3. Remote setup is optional for local work.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
        .padding()
        .task { await session.loadSetup() }
    }
}

private struct GeneralSettingsPane: View {
    let session: DesktopSession
    @Binding var restoresLastTask: Bool
    @AppStorage(JetNotificationPreferences.approvalsKey) private var approvalNotifications = false
    @AppStorage(JetNotificationPreferences.completionsKey) private var completionNotifications = false
    @AppStorage(JetNotificationPreferences.failuresKey) private var failureNotifications = false

    var body: some View {
        Form {
            Section("Workspace") {
                // ASVS 14.3.3: this preference contains no Jet content or
                // credentials. SwiftUI stores only the client-owned Boolean.
                Toggle("Restore the last task when Jet opens", isOn: $restoresLastTask)
            }

            Section("Appearance") {
                LabeledContent("Theme", value: "Use system setting")
                LabeledContent("Accent", value: "Jet Blue")
            }

            Section("Notifications") {
                Toggle("Approval requests", isOn: $approvalNotifications)
                    .accessibilityIdentifier("notifications-approvals")
                Toggle("Completed tasks", isOn: $completionNotifications)
                    .accessibilityIdentifier("notifications-completions")
                Toggle("Failed tasks", isOn: $failureNotifications)
                    .accessibilityIdentifier("notifications-failures")
                LabeledContent("System permission", value: authorizationLabel)
                Text("Jet notifications omit task names, prompts, paths, and tool details. Jet does not request notification sounds.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if let error = session.notificationError {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                }
            }
        }
        .formStyle(.grouped)
        .padding()
        .task { await session.refreshNotificationAuthorization() }
        .onChange(of: approvalNotifications) { _, enabled in
            requestNotificationAccess(if: enabled)
        }
        .onChange(of: completionNotifications) { _, enabled in
            requestNotificationAccess(if: enabled)
        }
        .onChange(of: failureNotifications) { _, enabled in
            requestNotificationAccess(if: enabled)
        }
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

private struct SettingsPlaceholder: View {
    let title: String
    let message: String
    let symbol: String

    var body: some View {
        ContentUnavailableView {
            Label(title, systemImage: symbol)
        } description: {
            Text(message)
        }
        .padding()
    }
}
