import SwiftUI

struct DesktopShellView: View {
    @Bindable var session: DesktopSession

    @SceneStorage("jet.shell.selection") private var storedSelection = SidebarDestination.conversation.rawValue
    @SceneStorage("jet.shell.work-panel") private var storedWorkPanel = WorkPanelTab.changes.rawValue
    @SceneStorage("jet.shell.work-panel-presented") private var storedPanelPresented = false
    @AppStorage("jet.settings.restore-last-task") private var restoresLastTask = true
    @AppStorage("jet.last-conversation") private var storedConversationID = ""
    @State private var columnVisibility: NavigationSplitViewVisibility = .all

    var body: some View {
#if os(macOS)
        GeometryReader { geometry in
            let compactWorkPanel = geometry.size.width < 1_100
            workspace(compactWorkPanel: compactWorkPanel)
                .onChange(of: compactWorkPanel) { _, isCompact in
                    if isCompact { session.isWorkPanelPresented = false }
                }
        }
        .frame(minWidth: 900, minHeight: 600)
#else
        workspace(compactWorkPanel: false)
#endif
    }

    private func workspace(compactWorkPanel: Bool) -> some View {
        NavigationSplitView(columnVisibility: $columnVisibility) {
            SidebarView(session: session)
#if os(macOS)
                .navigationSplitViewColumnWidth(min: 210, ideal: 244, max: 300)
#endif
        } detail: {
            switch session.sidebarSelection {
            case .project:
                ProjectPageView(session: session)
            case .trash:
                JetTrashPage(session: session)
            case .planes:
                PlaneManagementView(session: session)
            case .newTask, .search, .needsAttention, .conversation, .schedules:
                ConversationView(session: session)
            }
        }
#if os(macOS)
        .inspector(isPresented: panelBinding(compactWorkPanel: compactWorkPanel, sheet: false)) {
            WorkPanelView(session: session)
                .inspectorColumnWidth(min: 280, ideal: 340, max: 440)
        }
        .sheet(isPresented: panelBinding(compactWorkPanel: compactWorkPanel, sheet: true)) {
            VStack(spacing: 0) {
                HStack {
                    Text("Work details")
                        .font(.headline)
                    Spacer()
                    Button("Done") { session.isWorkPanelPresented = false }
                }
                .padding()
                Divider()
                WorkPanelView(session: session)
            }
            .frame(minWidth: 520, idealWidth: 720, minHeight: 500, idealHeight: 650)
            .accessibilityIdentifier("compact-work-panel")
        }
#endif
        .modifier(ShellPresentations(session: session))
        .tint(JetDesign.accent)
        .toolbar {
#if os(macOS)
            ToolbarItem(placement: .primaryAction) {
                Button {
                    session.isWorkPanelPresented.toggle()
                } label: {
                    Label(
                        session.isWorkPanelPresented ? "Hide Work Panel" : "Show Work Panel",
                        systemImage: "sidebar.right"
                    )
                }
                .help(session.isWorkPanelPresented ? "Hide Work Panel" : "Show Work Panel")
            }
#endif
        }
        .task {
            // Preview sessions arrive seeded; restoring scene state would replace the seed.
            guard !session.isPreviewSession else { return }
            await session.loadFoundationFixture()
            if restoresLastTask {
                session.restore(
                    selection: storedSelection,
                    workPanel: storedWorkPanel,
                    panelPresented: storedPanelPresented
                )
            } else {
                session.beginNewTask()
            }
            await session.loadSetup(openWhenIncomplete: true)
            await session.loadConversations(
                restoring: restoresLastTask ? UUID(uuidString: storedConversationID) : nil
            )
        }
        .onChange(of: session.sidebarSelection) { _, value in
            storedSelection = value.rawValue
        }
        .onChange(of: session.selectedWorkPanel) { _, value in
            storedWorkPanel = value.rawValue
        }
        .onChange(of: session.isWorkPanelPresented) { _, value in
            storedPanelPresented = value
        }
        .onChange(of: session.selectedConversationID) { _, value in
            storedConversationID = value?.uuidString.lowercased() ?? ""
        }
    }

#if os(macOS)
    private func panelBinding(compactWorkPanel: Bool, sheet: Bool) -> Binding<Bool> {
        Binding(
            get: { session.isWorkPanelPresented && compactWorkPanel == sheet },
            set: { presented in
                if compactWorkPanel == sheet { session.isWorkPanelPresented = presented }
            }
        )
    }
#endif
}

#if DEBUG
#Preview("Working") {
    DesktopPreviewScenes.view("task-working")
        .frame(width: 1280, height: 800)
}
#endif

#Preview("Fixture") {
    DesktopShellView(session: DesktopSession())
        .frame(width: 1280, height: 800)
}
