import SwiftUI

/// Settings › Tasks: how long Jet Trash keeps tasks, and cleaning up idle tasks.
struct TasksSettingsPane: View {
    let session: DesktopSession
    let settings: JetSettingsModel
    let recovery: JetRecoveryModel
    @Binding var computer: UUID
#if os(macOS)
    @Environment(\.openWindow) private var openWindow
#endif

    var body: some View {
        SettingsPaneForm {
            SettingsScopeHeader(session: session, computer: $computer, settings: settings, recovery: recovery)

            Group {
                Section("Jet Trash") {
                    SettingCountRow(
                        settings: settings,
                        key: "retention.trash_grace_days",
                        title: String(localized: "Keep deleted tasks in Jet Trash for"),
                        unit: String(localized: "days"),
                        minimum: 1,
                        caption: String(localized: "Applies to tasks you delete from now on."),
                        computerName: computerName
                    )
                    Button("Show Jet Trash") {
                        session.showTrash()
#if os(macOS)
                        openWindow(id: "main")
#endif
                    }
                }

                JetAutodeleteSection(
                    model: recovery,
                    computerName: computerName,
                    graceDays: graceDays,
                    taskTitle: taskTitle(for:)
                )
            }
            .disabled(settings.isUnreachable)

            SettingsNoticeSection(notice: recovery.notice, identifier: "recovery-notice")
            SettingsNoticeSection(notice: settings.notice)
        }
    }

    private var computerName: String { session.computerName(computer) }

    private var graceDays: UInt32 {
        if case let .count(days) = settings.settingValue("retention.trash_grace_days", scope: .plane) {
            return days
        }
        return 30
    }

    private func taskTitle(for conversationID: UUID) -> String {
        let title = session.conversations.first { $0.id == conversationID }?.title
            ?? session.memory.trashedTitle(conversationID)
        guard let title, !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return String(localized: "Untitled task")
        }
        return title
    }
}
