import SwiftUI

struct RenameTaskSheet: View {
    @Bindable var session: DesktopSession
    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var commandID = UUID()
    @FocusState private var nameFocused: Bool
    @State private var revision: UInt64
    @State private var conversationID: UUID?

    init(session: DesktopSession) {
        self.session = session
        _name = State(initialValue: session.selectedConversationTitle)
        _revision = State(initialValue: session.selectedConversation?.revision ?? 0)
        _conversationID = State(initialValue: session.selectedConversationID)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Rename task").font(.title3.weight(.semibold))
            TextField("Name", text: $name).textFieldStyle(.roundedBorder)
                .focused($nameFocused).disabled(session.conversationOperation != nil)
                .onChange(of: name) { _, _ in commandID = UUID() }
            if let notice = session.actionNotice { Text(notice).font(.callout).foregroundStyle(.secondary) }
            if let latest = session.selectedConversation, let latestRevision = latest.revision, latestRevision != revision {
                Text("Current name: \(session.selectedConversationTitle)").font(.callout)
                Button("Use latest version") {
                    revision = latestRevision
                    commandID = UUID()
                    session.actionNotice = nil
                }
                .disabled(session.conversationOperation != nil)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(session.conversationOperation == nil ? "Save" : "Saving…") {
                    Task { if await session.renameTask(name, revision: revision, commandID: commandID) { dismiss() } }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || name.utf8.count > 256)
            }
            .disabled(session.conversationOperation != nil)
        }
        .padding(24).frame(width: 360)
        .interactiveDismissDisabled(session.conversationOperation != nil)
        .onAppear { session.actionNotice = nil; nameFocused = true }
        .onChange(of: session.selectedConversationID) { _, selected in
            if selected != conversationID { dismiss() }
        }
    }
}
