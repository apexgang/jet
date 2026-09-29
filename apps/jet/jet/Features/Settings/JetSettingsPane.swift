import SwiftUI

/// The Settings tabs. Raw values are persisted in `jet.settings.last-pane`, so
/// they never change; new tabs are added only.
enum JetSettingsPane: String, CaseIterable, Identifiable, Sendable {
    case general
    case agents
    case work
    case connections
    case safety
    case advanced

    var id: Self { self }

    var title: String {
        switch self {
        case .general: String(localized: "General")
        case .agents: String(localized: "Assistants")
        case .work: String(localized: "Tasks")
        case .connections: String(localized: "Computers")
        case .safety: String(localized: "Safety")
        case .advanced: String(localized: "Advanced")
        }
    }

    var symbol: String {
        switch self {
        case .general: "gearshape"
        case .agents: "sparkles"
        case .work: "checklist"
        case .connections: "desktopcomputer"
        case .safety: "checkmark.shield"
        case .advanced: "gearshape.2"
        }
    }

    /// The tab that can fix a stable error, chosen by its code's prefix.
    static func resolving(_ error: JetPresentationError) -> JetSettingsPane? {
        let prefix = error.code.split(separator: ".").first.map(String.init)
        return switch prefix {
        case "notification": .general
        case "account", "credential", "extension", "craft", "usage": .agents
        case "schedule", "git", "retention", "autodelete": .work
        case "pairing", "remote", "ssh": .connections
        case "setting", "review", "energy": .safety
        case "storage", "audit", "recovery": .advanced
        default: nil
        }
    }

    /// The button title that opens this tab from elsewhere in Jet. It matches the
    /// composer's notice actions: "Assistant Settings…" or "Open Settings…".
    var openTitle: String {
        switch self {
        case .agents: String(localized: "Assistant Settings…")
        default: String(localized: "Open Settings…")
        }
    }
}

/// Opens the Settings tab that can fix `error`, when there is one.
struct JetSettingsRecoveryButton: View {
    let session: DesktopSession
    let error: JetPresentationError
#if os(macOS)
    @Environment(\.openSettings) private var openSettings
#endif

    var body: some View {
        if let pane = JetSettingsPane.resolving(error) {
#if os(macOS)
            Button(pane.openTitle) {
                session.requestSettings(pane)
                openSettings()
            }
#else
            EmptyView()
#endif
        }
    }
}
