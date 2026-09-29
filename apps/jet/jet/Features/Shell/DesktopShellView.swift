import SwiftUI

/// The main window: sidebar, detail and the Details inspector (design §6.1).
struct DesktopShellView: View {
    @Bindable var session: DesktopSession

    @SceneStorage("jet.shell.selection") private var storedSelection = SidebarDestination.conversation.rawValue
    @SceneStorage("jet.shell.work-panel") private var storedWorkPanel = WorkPanelTab.changes.rawValue
    @SceneStorage("jet.shell.work-panel-presented") private var storedPanelPresented = false
    @SceneStorage("jet.shell.sidebar-visibility") private var storedSidebarVisibility = ShellColumnLayout.showsSidebarValue
    @AppStorage("jet.settings.restore-last-task") private var restoresLastTask = true
    @AppStorage("jet.last-conversation") private var storedConversationID = ""
    @State private var layout = ShellColumnLayout()
    /// Zero until the first layout reports the real width, so a narrow window with
    /// Details never lays out the sidebar, the detail and Details together.
    @State private var windowWidth: CGFloat = 0
    @State private var detailWidth: CGFloat = 820
    @State private var announcer = TaskStatusAnnouncer()
#if os(macOS)
    @Environment(\.openSettings) private var openSettings
#endif

    var body: some View {
        NavigationSplitView(columnVisibility: sidebarBinding) {
            SidebarView(session: session)
#if os(macOS)
                .focusSection()
                .navigationSplitViewColumnWidth(min: 220, ideal: 260, max: 320)
#endif
        } detail: {
            detail
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { detailWidth = $0 }
                .modifier(ShellBannerBar(session: session))
                .modifier(ShellToolbar(session: session, detailWidth: detailWidth))
#if os(macOS)
                .focusSection()
#endif
        }
#if os(macOS)
        .inspector(isPresented: $session.isWorkPanelPresented) {
            WorkPanelView(session: session)
                .focusSection()
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier(currentLayout.isSidebarAutoHidden ? "compact-work-panel" : "details-column")
                .inspectorColumnWidth(min: 300, ideal: 360, max: inspectorMaxWidth)
        }
#endif
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width in
            windowWidth = width
            layout.update(width: width, detailsPresented: session.isWorkPanelPresented)
        }
#if os(macOS)
        .frame(minWidth: 900, minHeight: 600)
        .focusedSceneValue(\.isMainWindow, true)
        .onChange(of: session.settingsOpenRequest) { openSettings() }
#endif
        .modifier(ShellPresentations(session: session))
        // WP9: .keepChangesSupport(session: session) hosts the Keep Changes and Git
        // step confirmations here (lead, wave 3).
        // Inside the tint, so sheets and dialogs keep the copper accent too.
        .tint(JetDesign.accent)
        .task { await bootstrap() }
        .onChange(of: layout.persisted) { _, value in storedSidebarVisibility = value }
        .onChange(of: session.revealSidebarRequest) { showSidebar() }
        .onChange(of: session.sidebarSelection) { _, value in storedSelection = value.rawValue }
        .onChange(of: session.selectedWorkPanel) { _, value in storedWorkPanel = value.rawValue }
        .onChange(of: session.isWorkPanelPresented) { _, value in
            storedPanelPresented = value
            layout.update(width: windowWidth, detailsPresented: value)
        }
        .onChange(of: session.selectedConversationID) { _, value in
            storedConversationID = value?.uuidString.lowercased() ?? ""
        }
        .onChange(of: announcedStatus) { _, value in
            announcer.statusChanged(value.status, conversationID: value.conversationID)
        }
        .onDisappear { announcer.cancel() }
    }

    @ViewBuilder
    private var detail: some View {
        switch session.sidebarSelection {
        case .project:
            ProjectPageView(session: session)
        case .trash:
            JetTrashPage(session: session)
        case .newTask, .conversation, .search, .needsAttention, .schedules, .planes:
            ConversationView(session: session)
        }
    }

    /// The person's sidebar choice, except that narrow windows hide it while Details is open.
    private var sidebarBinding: Binding<NavigationSplitViewVisibility> {
        Binding(
            // Derived in the same update that opens Details, so the three columns
            // never have to fit a narrow window at once.
            get: { currentLayout.visibility },
            set: { personSet($0) }
        )
    }

    /// Details may widen only while the detail keeps 460 pt. A minimum width on the
    /// detail itself makes the split view count the hidden sidebar's minimum too, so
    /// the columns overflow a narrow window (and AppKit can loop on constraints).
    private var inspectorMaxWidth: CGFloat {
        let sidebar: CGFloat = currentLayout.visibility == .detailOnly ? 0 : 320
        return min(640, max(300, windowWidth - sidebar - 460))
    }

    /// The person showed or hid the sidebar; narrow windows then close Details.
    private func personSet(_ value: NavigationSplitViewVisibility) {
        layout = currentLayout
        if layout.personSet(value, width: windowWidth, detailsPresented: session.isWorkPanelPresented) {
            session.isWorkPanelPresented = false
        }
    }

    private var currentLayout: ShellColumnLayout {
        var current = layout
        current.update(width: windowWidth, detailsPresented: session.isWorkPanelPresented)
        return current
    }

    /// Find Task (⌘K) reveals the sidebar; the sidebar then focuses its search field.
    private func showSidebar() {
        personSet(.all)
    }

    private var announcedStatus: AnnouncedStatus {
        AnnouncedStatus(conversationID: session.selectedConversationID, status: session.selectedTaskStatus)
    }

    /// Restores the scene, then connects. There are no alerts at launch.
    private func bootstrap() async {
        layout = ShellColumnLayout(persisted: storedSidebarVisibility)
        layout.update(width: windowWidth, detailsPresented: session.isWorkPanelPresented)
        // Preview sessions arrive seeded; restoring scene state would replace the seed.
        guard !session.isPreviewSession else { return }
        await session.loadFoundationFixture()
        session.restore(
            selection: storedSelection,
            workPanel: storedWorkPanel,
            panelPresented: storedPanelPresented
        )
        if !restoresLastTask, session.sidebarSelection == .conversation {
            session.beginNewTask()
        }
        await session.loadSetup(openWhenIncomplete: true)
        await session.loadConversations(
            restoring: restoresLastTask ? UUID(uuidString: storedConversationID) : nil
        )
    }
}

private struct AnnouncedStatus: Equatable {
    let conversationID: UUID?
    let status: TaskStatus
}

/// Which columns show. Narrow windows (below 1100 pt) hide the sidebar while
/// Details is open and bring it back when Details closes or the window widens.
/// Showing the sidebar there closes Details instead: sidebar or Details, never both.
struct ShellColumnLayout: Equatable {
    static let narrowWidth: CGFloat = 1_100
    static let showsSidebarValue = "all"
    static let hidesSidebarValue = "detailOnly"

    /// The person's choice.
    var prefersSidebar = true
    /// Hidden for Details in a narrow window, not by the person.
    var isSidebarAutoHidden = false

    init() {}

    init(persisted: String) {
        prefersSidebar = persisted != Self.hidesSidebarValue
    }

    var persisted: String {
        prefersSidebar ? Self.showsSidebarValue : Self.hidesSidebarValue
    }

    var visibility: NavigationSplitViewVisibility {
        prefersSidebar && !isSidebarAutoHidden ? .all : .detailOnly
    }

    mutating func update(width: CGFloat, detailsPresented: Bool) {
        isSidebarAutoHidden = prefersSidebar && detailsPresented && width < Self.narrowWidth
    }

    /// The person showed or hid the sidebar. Returns true when Details must close
    /// to make room. The split view's echoes of the current visibility are ignored.
    mutating func personSet(_ value: NavigationSplitViewVisibility, width: CGFloat, detailsPresented: Bool) -> Bool {
        let showsSidebar = value != .detailOnly
        guard showsSidebar != (visibility != .detailOnly) else { return false }
        prefersSidebar = showsSidebar
        if showsSidebar, detailsPresented, width < Self.narrowWidth {
            isSidebarAutoHidden = false
            return true
        }
        update(width: width, detailsPresented: detailsPresented)
        return false
    }
}

#if DEBUG
#Preview("Working") {
    DesktopShellView(session: .preview { DesktopPreviewData.working($0) })
        .frame(width: 1280, height: 800)
}
#endif

#Preview("Fixture") {
    DesktopShellView(session: DesktopSession())
        .frame(width: 1280, height: 800)
}
