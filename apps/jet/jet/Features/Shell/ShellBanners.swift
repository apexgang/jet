import SwiftUI

/// The one banner the detail column shows at a time (design §6.4, §9).
enum ShellBanner: Equatable, Sendable {
    case notConnected(computer: String)
    case dataProtection
    /// `computer` is nil for this Mac.
    case lowDiskSpace(computer: String?)

    static let diskPressureCode = "storage.disk_pressure"
    static let recoveryReadOnlyCode = "recovery.read_only"
    /// Storage and recovery live in Settings › Advanced.
    static let advancedPane = JetSettingsPane.advanced

    var text: String {
        switch self {
        case let .notConnected(computer):
            String(localized: "Not connected to \(computer). Showing the last saved view.")
        case .dataProtection:
            String(localized: "Jet paused changes to protect your data.")
        case .lowDiskSpace(nil):
            String(localized: "Your Mac is almost out of disk space. Jet paused new work.")
        case let .lowDiskSpace(computer?):
            String(localized: "\(computer) is almost out of disk space. Jet paused new work.")
        }
    }

    var systemImage: String {
        switch self {
        case .notConnected: "wifi.slash"
        case .dataProtection: "exclamationmark.triangle.fill"
        case .lowDiskSpace: "externaldrive.badge.exclamationmark"
        }
    }

    var tint: Color {
        switch self {
        case .notConnected: .secondary
        case .dataProtection, .lowDiskSpace: .orange
        }
    }

    var actionTitle: String {
        switch self {
        case .notConnected: String(localized: "Try Again")
        case .dataProtection: String(localized: "Review…")
        case .lowDiskSpace: String(localized: "Storage Settings…")
        }
    }

    /// Review… and Storage Settings… open the Settings window, which only the Mac has.
    var showsAction: Bool {
#if os(macOS)
        true
#else
        if case .notConnected = self { return true }
        return false
#endif
    }

    /// The banner for these inputs. Not connected comes first, then data protection,
    /// then low disk space. New Task has its own checklist and composer blocker for a
    /// missing connection, and Jet Trash shows both states in its own rows.
    static func resolve(_ inputs: ShellBannerInputs) -> ShellBanner? {
        let ownsConnectionState = inputs.destination != .newTask && inputs.destination != .trash
        if ownsConnectionState, inputs.connection != .starting,
           inputs.connection == .notConnected || inputs.showsSavedView
        {
            return .notConnected(computer: inputs.computerName)
        }
        if inputs.destination != .trash,
           inputs.recoveryReadOnly || inputs.errorCodes.contains(recoveryReadOnlyCode)
        {
            return .dataProtection
        }
        if inputs.errorCodes.contains(diskPressureCode) {
            return .lowDiskSpace(computer: inputs.isLocal ? nil : inputs.computerName)
        }
        return nil
    }

    /// The core reports read-only recovery as `{"state":"read_only", …}`. Anything
    /// else, including malformed JSON, is not read-only.
    static func isReadOnly(recoveryJSON: String?) -> Bool {
        guard let data = recoveryJSON?.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return false }
        return object["state"] as? String == "read_only"
    }
}

/// Everything the banner depends on, so the choice stays a pure function.
struct ShellBannerInputs: Equatable {
    enum Connection: Equatable {
        case starting
        case connected
        case notConnected
    }

    var destination: ShellDestination
    var computerName: String
    var isLocal: Bool
    var connection: Connection
    var showsSavedView: Bool
    var recoveryReadOnly: Bool
    var errorCodes: [String]
}

extension ShellBannerInputs {
    init(session: DesktopSession) {
        let destination = ShellDestination(session: session)
        let isLocal = session.isLocalPlane(session.selectedPlaneRegistryID)
        var errors = [
            session.actionError,
            session.workError,
            session.workNoticeError,
            session.gitDeliveryError,
            session.selectedPlane?.failure,
        ]
        if isLocal, case let .failed(failure) = session.setupState { errors.append(failure) }
        self.init(
            destination: destination,
            computerName: session.selectedPlaneName,
            isLocal: isLocal,
            connection: isLocal
                ? Self.localConnection(setupState: session.setupState, connectionState: session.connectionState)
                : Self.remoteConnection(session.selectedPlane, localSetupState: session.setupState),
            showsSavedView: destination == .task && session.conversationFreshness == .cached,
            recoveryReadOnly: ShellBanner.isReadOnly(
                recoveryJSON: session.selectedSetupSnapshot?.status.recovery?.source
            ),
            errorCodes: errors.compactMap { $0?.code }
        )
    }

    /// This Mac is starting while its first setup load runs and nothing failed yet.
    static func localConnection(
        setupState: DesktopSession.SetupState,
        connectionState: JetConnectionState
    ) -> Connection {
        switch connectionState {
        case .connected:
            return .connected
        case .disconnected, .connecting:
            switch setupState {
            case .idle, .loading: return .starting
            case .ready, .failed: return .notConnected
            }
        case .reconnecting, .failed:
            return .notConnected
        }
    }

    /// Another computer is starting while it connects. Before its first connection
    /// attempt (This Mac still starting, no recorded failure) it counts as starting
    /// too, so a restored task shows no banner for a moment at launch.
    static func remoteConnection(
        _ plane: JetPlanePresentation?,
        localSetupState: DesktopSession.SetupState
    ) -> Connection {
        switch plane?.connection {
        case .connected:
            return .connected
        case .connecting:
            return .starting
        case .disconnected where plane?.failure == nil:
            switch localSetupState {
            case .idle, .loading: return .starting
            case .ready, .failed: return .notConnected
            }
        case .disconnected, .reconnecting, .failed, nil:
            return .notConnected
        }
    }
}

/// Shows the detail column's banner in its top bar, below the toolbar.
struct ShellBannerBar: ViewModifier {
    @Bindable var session: DesktopSession
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var isRetrying = false

    func body(content: Content) -> some View {
        let banner = session.usesLivePlane ? ShellBanner.resolve(ShellBannerInputs(session: session)) : nil
        content
            .safeAreaBar(edge: .top, spacing: 0) {
                if let banner {
                    ShellBannerRow(banner: banner, isRetrying: isRetrying) { perform(banner) }
                        .transition(.opacity)
                }
            }
            .scrollEdgeEffectStyle(banner == nil ? .automatic : .hard, for: .top)
            .animation(reduceMotion ? nil : .default, value: banner)
            .onChange(of: banner, initial: true) { _, banner in
                if let banner { AccessibilityNotification.Announcement(banner.text).post() }
            }
    }

    private func perform(_ banner: ShellBanner) {
        switch banner {
        case .notConnected:
            guard !isRetrying else { return }
            isRetrying = true
            Task {
                await session.retryConnection()
                isRetrying = false
            }
        case .dataProtection, .lowDiskSpace:
            session.perform(.openSettings(ShellBanner.advancedPane))
        }
    }
}

private struct ShellBannerRow: View {
    let banner: ShellBanner
    let isRetrying: Bool
    let perform: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: banner.systemImage)
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(banner.tint)
                    .accessibilityHidden(true)
                Text(banner.text)
                    .font(.body)
                    .foregroundStyle(.primary)
                    .layoutPriority(1)
                Spacer(minLength: 8)
                if isRetrying {
                    ProgressView()
                        .controlSize(.small)
                        .accessibilityLabel(Text("Trying again"))
                }
                if banner.showsAction {
                    Button(banner.actionTitle, action: perform)
                        .buttonStyle(.bordered)
                        .controlSize(.regular)
                        // Copper text on a faint fill: the text colour keeps its contrast.
                        .tint(JetDesign.accentText)
                        .disabled(isRetrying)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            Divider()
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("shell-banner")
    }
}
