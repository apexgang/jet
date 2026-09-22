import SwiftUI

struct JetSettingsView: View {
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
            GeneralSettingsPane(restoresLastTask: $restoresLastTask)
                .tabItem { Label(Pane.general.title, systemImage: Pane.general.symbol) }
                .tag(Pane.general)

            SettingsPlaceholder(
                title: "Agents",
                message: "Accounts, models, and extensions arrive with setup in Wave 1.2.",
                symbol: Pane.agents.symbol
            )
            .tabItem { Label(Pane.agents.title, systemImage: Pane.agents.symbol) }
            .tag(Pane.agents)

            SettingsPlaceholder(
                title: "Work",
                message: "Project defaults, delivery, schedules, and retention arrive in later slices.",
                symbol: Pane.work.symbol
            )
            .tabItem { Label(Pane.work.title, systemImage: Pane.work.symbol) }
            .tag(Pane.work)

            SettingsPlaceholder(
                title: "Connections",
                message: "Local service and remote Plane controls arrive in Wave 3.",
                symbol: Pane.connections.symbol
            )
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

private struct GeneralSettingsPane: View {
    @Binding var restoresLastTask: Bool

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
        }
        .formStyle(.grouped)
        .padding()
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
