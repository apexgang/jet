import SwiftUI

/// Details › Terminal: terminals that run in the task's working copy.
struct DetailsTerminalView: View {
    @Bindable var session: DesktopSession
    @Bindable var model: DetailsPanelModel

    @FocusState private var inputFocused: Bool

    var body: some View {
        if session.selectedRun == nil {
            ContentUnavailableView(
                "No Terminal Yet",
                systemImage: "terminal",
                description: Text("Terminals open in this task's working copy after \(assistant) starts.")
            )
        } else if session.detailsWorksInProjectFolder {
            ContentUnavailableView(
                "Terminal Unavailable",
                systemImage: "terminal",
                description: Text(projectFolderText)
            )
        } else if session.workTerminals.isEmpty {
            ContentUnavailableView {
                Label("No Terminals", systemImage: "terminal")
            } description: {
                Text("Terminals open in this task's working copy.")
            } actions: {
                Button("New Terminal") { Task { await session.createWorkspaceTerminal() } }
                    .disabled(!canCreate)
            }
        } else {
            terminal
        }
    }

    private var assistant: String {
        DetailsCopy.assistant(session.selectedAssistantName, position: .mid)
    }

    private var projectFolderText: String {
        session.detailsProjectName.map {
            String(localized: "This task works directly in your \($0) folder. Use Terminal on your Mac instead.")
        } ?? String(localized: "This task works directly in your project folder. Use Terminal on your Mac instead.")
    }

    private var canCreate: Bool {
        session.workOperation == nil && !session.detailsIsOffline && session.workDiff?.workspaceID != nil
    }

    private var selected: JetWorkspaceTerminal? { session.selectedDetailsTerminal }
    private var isAttached: Bool {
        session.attachedTerminalID != nil && session.attachedTerminalID == session.selectedTerminalID
    }

    private var isClosed: Bool {
        switch selected?.state {
        case .closed, .unavailable: true
        case .opening, .open, .closing, nil: false
        }
    }

    private var terminal: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
            output
            inputRow
                .padding(.horizontal, 12)
                .padding(.top, 8)
            controls
                .padding(12)
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Picker("Terminal", selection: terminalBinding) {
                ForEach(session.workTerminals) { terminal in
                    Text(verbatim: session.terminalTitles[terminal.id] ?? "")
                        .tag(Optional(terminal.id))
                }
            }
            .pickerStyle(.menu)
            .labelsHidden()
            .fixedSize()
            .help(pickerHelp)
            Spacer(minLength: 0)
            Button {
                Task { await session.createWorkspaceTerminal() }
            } label: {
                Label("New Terminal", systemImage: "plus")
                    .labelStyle(.iconOnly)
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.borderless)
            .tint(.secondary)
            .help("New Terminal")
            .disabled(!canCreate)
            .accessibilityIdentifier("terminal-new")
        }
    }

    /// "Terminal 1 · <id> · open": the ID and state live only in the tooltip.
    private var pickerHelp: String {
        guard let selected else { return "" }
        let title = session.terminalTitles[selected.id] ?? ""
        return "\(title) · \(selected.id.uuidString.lowercased()) · \(selected.state.rawValue)"
    }

    private var terminalBinding: Binding<UUID?> {
        Binding(
            get: { session.selectedTerminalID },
            set: { id in
                guard let id else { return }
                Task { await session.selectTerminal(id) }
            }
        )
    }

    private var outputText: String {
        if isClosed { return String(localized: "This terminal is closed.") }
        let text = session.selectedTerminalID.flatMap { session.terminalOutput[$0] } ?? ""
        if text.isEmpty, !isAttached { return String(localized: "Attach to see live output.") }
        return text
    }

    private var output: some View {
        ScrollView {
            Text(verbatim: outputText)
                .font(.system(size: JetDesign.TextSize.control, design: .monospaced))
                .foregroundStyle(showsPlaceholder ? .secondary : .primary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(12)
        }
        .defaultScrollAnchor(.bottom)
        .onGeometryChange(for: CGSize.self) { $0.size } action: { size in
            session.setTerminalGeometry(width: size.width - 24, height: size.height - 24)
        }
        .background(Color.primary.opacity(0.035))
        .overlay(alignment: .top) { Divider() }
        .overlay(alignment: .bottom) { Divider() }
    }

    private var showsPlaceholder: Bool {
        isClosed || (!isAttached && (session.selectedTerminalID.flatMap { session.terminalOutput[$0] } ?? "").isEmpty)
    }

    private var inputRow: some View {
        HStack(spacing: 8) {
            TextField("Type a command", text: $session.terminalInput)
                .textFieldStyle(.roundedBorder)
                .font(.system(size: JetDesign.TextSize.control, design: .monospaced))
                .focused($inputFocused)
                .reportsTextEditing(inputFocused)
                .onSubmit { Task { await session.sendTerminalLine() } }
                .disabled(!isAttached || session.detailsIsOffline)
                .accessibilityIdentifier("terminal-input")
            Button("Send") { Task { await session.sendTerminalLine() } }
                .disabled(!isAttached || session.detailsIsOffline || session.terminalInput.isEmpty)
        }
    }

    private var controls: some View {
        HStack(spacing: 8) {
            if isAttached {
                Button("Detach", action: session.detachSelectedTerminal)
                    .help("Stop showing live output. The terminal keeps running.")
            } else {
                Button("Attach") { Task { await session.attachSelectedTerminal() } }
                    .disabled(selected?.state != .open || session.detailsIsOffline)
            }
            Spacer(minLength: 0)
            Button("Close Terminal…") { model.terminalPendingClose = session.selectedTerminalID }
                .disabled(isClosed || selected == nil || session.workOperation != nil || session.detailsIsOffline)
                .confirmationDialog(
                    Text("Close \(pendingTitle)?"),
                    isPresented: closeConfirmationPresented,
                    titleVisibility: .visible,
                    presenting: model.terminalPendingClose
                ) { id in
                    Button("Close Terminal", role: .destructive) {
                        model.terminalPendingClose = nil
                        Task { await session.closeTerminal(id) }
                    }
                    Button("Cancel", role: .cancel) { model.terminalPendingClose = nil }
                } message: { _ in
                    Text("Commands running in it stop.")
                }
            // WP8: .focusedSceneValue(\.hasOpenDialog, model.terminalPendingClose != nil ? true : nil) once WP5 defines the key (critic 4.8).
        }
    }

    private var pendingTitle: String {
        model.terminalPendingClose.flatMap { session.terminalTitles[$0] } ?? String(localized: "Terminal")
    }

    private var closeConfirmationPresented: Binding<Bool> {
        Binding(
            get: { model.terminalPendingClose != nil },
            set: { presented in
                if !presented { model.terminalPendingClose = nil }
            }
        )
    }
}

#if DEBUG
#Preview { DesktopPreviewScenes.details[0].makeView() }
#endif
