import SwiftUI

struct TerminalWorkView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(spacing: 0) {
            WorkSectionHeader(
                title: "Workspace terminal",
                detail: session.workDiff?.workspaceID == nil
                    ? "Requires a managed Workspace"
                    : "Scoped to this managed Workspace"
            ) {
                Button("New") { Task { await session.createWorkspaceTerminal() } }
                    .disabled(session.workDiff?.workspaceID == nil || session.workOperation != nil)
            }
            Divider()

            if session.workDiff?.workspaceID == nil {
                WorkPanelEmptyState(
                    title: "No managed Workspace",
                    message: "This Run does not expose a terminal-capable Workspace.",
                    symbol: "terminal"
                )
            } else if session.workTerminals.isEmpty {
                WorkPanelEmptyState(
                    title: "No terminal open",
                    message: "Create a terminal owned by this Workspace. Jet does not launch an unrestricted host shell.",
                    symbol: "terminal"
                )
            } else {
                Picker("Session", selection: $session.selectedTerminalID) {
                    ForEach(session.workTerminals) { terminal in
                        Text("\(terminal.id.uuidString.prefix(8)) · \(terminal.state.rawValue)")
                            .tag(Optional(terminal.id))
                    }
                }
                .padding(12)

                GeometryReader { geometry in
                    ScrollView([.horizontal, .vertical]) {
                        Text(
                            session.selectedTerminalID.flatMap { session.terminalOutput[$0] }
                                ?? "Terminal output will appear here."
                        )
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .topLeading)
                        .padding(12)
                    }
                    .onAppear {
                        session.setTerminalGeometry(
                            width: geometry.size.width,
                            height: geometry.size.height
                        )
                    }
                    .onChange(of: geometry.size) { _, size in
                        session.setTerminalGeometry(width: size.width, height: size.height)
                    }
                }
                .defaultScrollAnchor(.bottom)
                .background(Color.primary.opacity(0.035))

                HStack(spacing: 8) {
                    TextField("Send input", text: $session.terminalInput)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(.caption, design: .monospaced))
                        .disabled(session.attachedTerminalID == nil)
                        .onSubmit { Task { await session.sendTerminalLine() } }
                    Button("Send") { Task { await session.sendTerminalLine() } }
                        .disabled(session.attachedTerminalID == nil || session.terminalInput.isEmpty)
                }
                .padding(12)

                HStack {
                    if session.attachedTerminalID == session.selectedTerminalID {
                        Button("Detach", action: session.detachSelectedTerminal)
                    } else {
                        Button("Attach") { Task { await session.attachSelectedTerminal() } }
                            .disabled(selectedTerminal?.state != .open)
                    }
                    Spacer()
                    Button("Close", role: .destructive) {
                        Task { await session.closeSelectedTerminal() }
                    }
                    .disabled(session.selectedTerminalID == nil || session.workOperation != nil)
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 12)
            }
        }
    }

    private var selectedTerminal: JetWorkspaceTerminal? {
        guard let selectedTerminalID = session.selectedTerminalID else { return nil }
        return session.workTerminals.first { $0.id == selectedTerminalID }
    }
}
