import SwiftUI

/// Whether a proposed task name can be sent.
enum RenameValidation: Equatable {
    case valid
    case empty
    case unchanged
    case tooLong

    static let maximumBytes = 256

    static func evaluate(name: String, currentTitle: String) -> RenameValidation {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return .empty }
        if trimmed.utf8.count > maximumBytes { return .tooLong }
        if trimmed == currentTitle { return .unchanged }
        return .valid
    }
}

/// Rename Task. The rename is bound to the revision the person saw: when the task
/// is renamed elsewhere, they choose which name to keep, and every choice sends a
/// new Command.
struct RenameTaskSheet: View {
    @Bindable var session: DesktopSession
    private let conversationID: UUID?

    @State private var name: String
    @State private var revision: UInt64?
    @State private var commandID = UUID()
    @State private var failure: String?
    @FocusState private var nameFocused: Bool

    /// Without a captured revision, the sheet takes the revision of the task's
    /// snapshot the first time it is loaded, and waits until then.
    init(session: DesktopSession, capturedRevision: UInt64? = nil, initialName: String? = nil) {
        self.session = session
        conversationID = session.selectedConversationID
        _name = State(initialValue: initialName ?? session.selectedConversationTitle)
        _revision = State(initialValue: capturedRevision ?? Self.snapshotRevision(of: session))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Rename Task")
                .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
            Form {
                TextField("Name", text: $name)
                    .focused($nameFocused)
                    .disabled(isRenaming)
                    .onSubmit(rename)
            }
            notices
            HStack {
                Spacer()
                Button("Cancel", action: session.dismissSheet)
                    .keyboardShortcut(.cancelAction)
                Button(isRenaming ? String(localized: "Renaming…") : String(localized: "Rename"), action: rename)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canRename)
            }
        }
        .padding(20)
        .frame(width: 420)
        .interactiveDismissDisabled(isRenaming)
        .onAppear { nameFocused = true }
        .onChange(of: name) { _, _ in commandID = UUID() }
        .onChange(of: session.conversationSnapshot) { _, _ in
            if revision == nil { revision = Self.snapshotRevision(of: session) }
        }
        .onChange(of: session.selectedConversationID) { _, selected in
            if selected != conversationID { session.dismissSheet() }
        }
    }

    @ViewBuilder
    private var notices: some View {
        if revision == nil {
            Text("Loading…")
                .foregroundStyle(.secondary)
        }
        if validation == .tooLong {
            Text("This name is too long.")
                .foregroundStyle(.secondary)
        }
        if let latest = staleLatest {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange)
                        .accessibilityHidden(true)
                    Text("This task was renamed to “\(latest.title)” elsewhere.")
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                HStack(spacing: 8) {
                    Button("Use Latest Name") {
                        name = latest.title
                        adopt(latest)
                    }
                    Button("Keep Mine") { adopt(latest) }
                }
                .buttonStyle(.bordered)
                .disabled(isRenaming)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        if !session.planeIsConnected {
            Text("You can rename this task when Jet reconnects.")
                .foregroundStyle(.secondary)
        }
        if let failure {
            Label {
                Text(failure)
            } icon: {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.orange)
                    .accessibilityHidden(true)
            }
        }
    }

    // MARK: - State

    private var isRenaming: Bool { session.userOperation == .renaming }

    /// The newest known summary of this task, from the list or its snapshot.
    private var latest: JetConversationSummary? {
        let listed = session.conversations.first { $0.id == conversationID }
        let loaded = session.conversationSnapshot?.conversation
        guard let loaded, loaded.id == conversationID else { return listed }
        guard let listed else { return loaded }
        return (listed.revision ?? 0) > (loaded.revision ?? 0) ? listed : loaded
    }

    /// The newer summary when the task changed since the sheet captured its revision.
    private var staleLatest: JetConversationSummary? {
        guard let revision, let latest, let latestRevision = latest.revision,
              latestRevision != revision
        else { return nil }
        return latest
    }

    private var validation: RenameValidation {
        RenameValidation.evaluate(name: name, currentTitle: latest?.title ?? session.selectedConversationTitle)
    }

    private var canRename: Bool {
        validation == .valid && revision != nil && staleLatest == nil
            && session.planeIsConnected && session.userOperation == nil
    }

    private static func snapshotRevision(of session: DesktopSession) -> UInt64? {
        guard let snapshot = session.conversationSnapshot,
              snapshot.conversation.id == session.selectedConversationID
        else { return nil }
        return snapshot.conversation.revision
    }

    private func adopt(_ latest: JetConversationSummary) {
        revision = latest.revision
        commandID = UUID()
        failure = nil
    }

    private func rename() {
        guard canRename, let revision else { return }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let noticeBefore = session.composerNotice
        failure = nil
        Task {
            if await session.renameTask(trimmed, revision: revision, commandID: commandID) {
                session.dismissSheet()
            } else if session.composerNotice != noticeBefore, let notice = session.composerNotice {
                failure = notice.text
                session.composerNotice = nil
            }
        }
    }
}
