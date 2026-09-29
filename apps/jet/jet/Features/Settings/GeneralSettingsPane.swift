import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

/// Settings › General: opening, sending, text size and notifications. These are
/// this Mac's own preferences, so the pane has no computer picker.
struct GeneralSettingsPane: View {
    let session: DesktopSession

    // ASVS 14.3.3: these are client-owned preferences. They contain no Jet
    // content, credential, path, or connection proof.
    @AppStorage("jet.settings.restore-last-task") private var restoresLastTask = true
    // WP11: WP7 names this key `ComposerPreferences.returnSendsKey`; the lead
    // switches to the constant in wave 3.
    @AppStorage("jet.composer.return-sends") private var returnSends = false
    @AppStorage(JetDesign.transcriptScaleKey) private var transcriptScale = 1.0
    @AppStorage(JetNotificationPreferences.approvalsKey) private var approvalNotifications = false
    @AppStorage(JetNotificationPreferences.completionsKey) private var completionNotifications = false
    @AppStorage(JetNotificationPreferences.failuresKey) private var failureNotifications = false

    @Environment(\.openURL) private var openURL

    var body: some View {
        SettingsPaneForm {
            Section("When Jet Opens") {
                Toggle("Reopen the last task", isOn: $restoresLastTask)
            }

            Section("Messages") {
                Toggle(isOn: $returnSends) {
                    Text("Press Return to Send")
                    Text(returnSends
                        ? String(localized: "Return sends. Press Shift-Return for a new line.")
                        : String(localized: "Press ⌘↩ to send. Return starts a new line."))
                }
                Picker("Text size", selection: scaleBinding) {
                    ForEach(JetDesign.transcriptScaleSteps, id: \.self) { step in
                        Text(Self.scaleLabel(step)).tag(step)
                    }
                }
                .pickerStyle(.menu)
                .tint(JetDesign.accentText)
                Text("Jet keeps your project safe until you keep the changes.")
                    .font(.system(size: JetDesign.TextSize.content * Self.snapped(transcriptScale)))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityLabel(Text("Sample text"))
                    .accessibilityValue(Text("Jet keeps your project safe until you keep the changes."))
            }

            Section {
                Toggle("A task needs you", isOn: $approvalNotifications)
                    .accessibilityIdentifier("notifications-approvals")
                Toggle("A reply is ready", isOn: $completionNotifications)
                    .accessibilityIdentifier("notifications-completions")
                Toggle("A task stops with an error", isOn: $failureNotifications)
                    .accessibilityIdentifier("notifications-failures")
                permissionRow
                if let error = session.notificationError {
                    SettingsStatusLabel(text: error, systemImage: "exclamationmark.triangle.fill", tint: .orange)
                        .font(.system(size: JetDesign.TextSize.control))
                }
            } header: {
                Text("Notify Me When")
            } footer: {
                Text("Notifications never include what your tasks say.")
                    .settingsCaption()
            }
        }
        .task { await session.refreshNotificationAuthorization() }
        .onChange(of: approvalNotifications) { _, enabled in requestPermission(ifTurnedOn: enabled) }
        .onChange(of: completionNotifications) { _, enabled in requestPermission(ifTurnedOn: enabled) }
        .onChange(of: failureNotifications) { _, enabled in requestPermission(ifTurnedOn: enabled) }
    }

    @ViewBuilder
    private var permissionRow: some View {
        switch session.notificationAuthorization {
        case .denied:
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                SettingsStatusLabel(
                    text: String(localized: "Notifications for Jet are off in System Settings."),
                    systemImage: "bell.slash"
                )
                .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 8)
                Button("Open System Settings…") {
                    if let url = Self.notificationSettingsURL { openURL(url) }
                }
                .buttonStyle(.bordered)
            }
        case .provisional:
            SettingsStatusLabel(
                text: String(localized: "Notifications arrive quietly in Notification Center."),
                systemImage: "bell.badge"
            )
            .settingsCaption()
        case .notDetermined, .authorized:
            EmptyView()
        }
    }

    private var scaleBinding: Binding<Double> {
        Binding(
            get: { Self.snapped(transcriptScale) },
            set: { transcriptScale = $0 }
        )
    }

    private func requestPermission(ifTurnedOn enabled: Bool) {
        guard enabled, session.notificationAuthorization == .notDetermined else { return }
        Task { _ = await session.requestNotificationAuthorization() }
    }

    /// The step nearest to a stored scale, so older values still select a step.
    static func snapped(_ scale: Double) -> Double {
        JetDesign.transcriptScaleSteps.min { abs($0 - scale) < abs($1 - scale) } ?? 1
    }

    static func scaleLabel(_ step: Double) -> String {
        let percent = Int((step * 100).rounded())
        return step == 1 ? String(localized: "\(percent)% (Default)") : String(localized: "\(percent)%")
    }

    /// Jet's page in System Settings › Notifications.
    static var notificationSettingsURL: URL? {
#if os(macOS)
        // WP11: the lead switches to WP4's `session.openNotificationSettings()` in wave 3.
        URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=\(Bundle.main.bundleIdentifier ?? "")")
#elseif canImport(UIKit)
        URL(string: UIApplication.openNotificationSettingsURLString)
#else
        nil
#endif
    }
}
