#if DEBUG
import SwiftUI

extension DesktopPreviewScenes {
    /// Task status driven by live Events (WP4). The window scenes show the rows
    /// the script sets; the board scenes list what each row derives, so the
    /// status store can be checked on its own.
    @MainActor static var status: [DesktopPreviewScene] {
        [
            statusWindow("status-events", seed: seedNewTask),
            statusWindow("status-events-selected", seed: seedOpenTask),
            DesktopPreviewScene(id: "status-board", size: CGSize(width: 760, height: 420)) {
                AnyView(StatusBoardPreview(session: .preview(configure: seedNewTask)))
            },
            DesktopPreviewScene(id: "status-board-selected", size: CGSize(width: 760, height: 420)) {
                AnyView(StatusBoardPreview(session: .preview(configure: seedOpenTask)))
            },
        ]
    }

    /// New Task is open while the script updates every row.
    @MainActor private static func seedNewTask(_ session: DesktopSession) {
        DesktopPreviewData.connect(session)
        session.open(.newTask)
        StatusPreviewScript.play(on: session, rows: DesktopPreviewData.conversations.map(\.id))
    }

    /// A task is open and plays the reply row: its new reply counts as seen.
    @MainActor private static func seedOpenTask(_ session: DesktopSession) {
        DesktopPreviewData.waitingWithChanges(session)
        var rows = DesktopPreviewData.conversations.map(\.id)
        if let selected = session.selectedConversationID, let index = rows.firstIndex(of: selected) {
            rows.swapAt(index, StatusPreviewScript.replyRow)
        }
        StatusPreviewScript.play(on: session, rows: rows)
        session.statusStore.noteSelected(session.selectedConversationID)
    }

    @MainActor private static func statusWindow(
        _ id: String,
        seed: @escaping @MainActor (DesktopSession) -> Void
    ) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id) {
            AnyView(ContentView(session: .preview(configure: seed)))
        }
    }
}

/// Feeds synthetic Events for seven tasks through the status store, the way
/// the Plane's Event stream does.
enum StatusPreviewScript {
    static let sequenceBase: UInt64 = 1_000_000
    /// The row that ends with a reply the person hasn't read.
    static let replyRow = 3

    /// `rows` are seven task IDs:
    /// 0 working and editing files; 1 needs permission; 2 needs sign-in;
    /// 3 waiting for your reply, with a new reply; 4 stopped with an error;
    /// 5 stopped after a restart (no glyph, not in Needs You); 6 unknown.
    @MainActor static func play(on session: DesktopSession, rows: [UUID]) {
        guard rows.count >= 7 else { return }
        let planeRegistryID = session.localPlaneRegistryID
        var sequence = sequenceBase

        func send(_ row: Int, _ kind: String, _ payload: String) {
            sequence += 1
            let event = JetEvent(
                sequence: sequence,
                eventID: UUID(),
                actor: JetRawJSON(source: #"{"type":"harness"}"#),
                origin: nil,
                recordedAtUnixMilliseconds: DesktopPreviewData.now,
                conversationID: rows[row],
                runID: DesktopPreviewData.runID(20 + row),
                kind: kind,
                payloadVersion: 1,
                payload: JetRawJSON(source: payload)
            )
            session.statusStore.record(event, planeRegistryID: planeRegistryID)
        }

        send(0, "run.created", "{}")
        send(0, "run.lifecycle_changed", #"{"from":"created","to":"active"}"#)
        send(0, "run.activity_changed", #"{"activity":"working"}"#)
        send(0, "run.output", output(kind: "text", text: "Edit"))

        send(1, "run.activity_changed", #"{"activity":"working"}"#)
        send(1, "approval.requested", #"{"request":{}}"#)
        send(1, "run.activity_changed", #"{"activity":"waiting_for_approval"}"#)

        send(2, "run.activity_changed", #"{"activity":"waiting_for_auth"}"#)

        send(replyRow, "run.activity_changed", #"{"activity":"working"}"#)
        send(replyRow, "run.output", output(kind: "markdown", text: "The redirect loop is fixed."))
        send(replyRow, "run.activity_changed", #"{"activity":"waiting_for_user"}"#)

        send(4, "run.lifecycle_changed", #"{"from":"starting","to":"active"}"#)
        send(4, "run.lifecycle_changed", #"{"from":"active","to":"failed"}"#)

        send(5, "run.lifecycle_changed", #"{"from":"starting","to":"active"}"#)
        send(5, "run.lifecycle_changed", #"{"from":"active","to":"lost"}"#)

        session.statusStore.remove(rows[6])
    }

    /// A `run.output` payload with one presentation block.
    private static func output(kind: String, text: String) -> String {
        let block = ["kind": kind, "text": text]
        let payload: [String: Any] = [
            "native_json": "{}",
            "presentation_json": [json(block)],
        ]
        return json(payload)
    }

    private static func json(_ object: Any) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else {
            return "{}"
        }
        return String(decoding: data, as: UTF8.self)
    }
}

/// What each task row derives after the script: its glyph, its status, the
/// Needs You list and the Dock badge.
private struct StatusBoardPreview: View {
    let session: DesktopSession

    var body: some View {
        VStack(alignment: .leading, spacing: JetDesign.gap) {
            VStack(spacing: 0) {
                ForEach(session.conversations) { task in
                    row(task)
                    if task.id != session.conversations.last?.id {
                        Divider().padding(.leading, 34)
                    }
                }
            }
            .background(.background, in: RoundedRectangle(cornerRadius: JetDesign.fieldRadius))
            .overlay {
                RoundedRectangle(cornerRadius: JetDesign.fieldRadius)
                    .strokeBorder(.separator)
            }

            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 6) {
                GridRow(alignment: .firstTextBaseline) {
                    Text("Needs You").foregroundStyle(.secondary)
                    Text(needsYouTitles)
                }
                GridRow(alignment: .firstTextBaseline) {
                    Text("Dock badge").foregroundStyle(.secondary)
                    Text(dockBadge)
                }
            }
            .font(.system(size: JetDesign.TextSize.control))
        }
        .padding(JetDesign.sectionGap)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(.background.secondary)
        .tint(JetDesign.accent)
    }

    private func row(_ task: JetConversationSummary) -> some View {
        let status = session.taskStatus(for: task.id)
        let isOpen = task.id == session.selectedConversationID
        return HStack(spacing: 10) {
            TaskStatusGlyph(status: status, isUnread: session.isUnread(task.id))
            Text(task.title)
                .font(.system(size: JetDesign.TextSize.navigation, weight: isOpen ? .semibold : .regular))
                .lineLimit(1)
            Spacer(minLength: 12)
            TaskStatusLabel(status: status)
        }
        .padding(.horizontal, 10)
        .frame(height: 34)
        .background {
            if isOpen {
                RoundedRectangle(cornerRadius: JetDesign.controlRadius)
                    .fill(.quaternary)
                    .padding(2)
            }
        }
    }

    private var needsYouTitles: String {
        let titles = session.needsYouConversationIDs.compactMap { id in
            session.conversations.first { $0.id == id }?.title
        }
        return titles.isEmpty ? String(localized: "None") : titles.joined(separator: "\n")
    }

    private var dockBadge: String {
#if os(macOS)
        JetDockBadge.label(for: session.needsYouConversationIDs.count) ?? String(localized: "None")
#else
        String(localized: "None")
#endif
    }
}
#endif
