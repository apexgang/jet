import Testing
@testable import jet

@MainActor
struct DesktopSessionTests {
    @Test
    func fixtureBackedShellMovesBetweenTaskStatesWithoutInventingDomainState() async {
        let session = DesktopSession()

        await session.loadFoundationFixture()
        #expect(session.scenario?.state == .active)
        #expect(session.sidebarSelection == .conversation)
        #expect(session.isWorkPanelPresented)

        session.beginNewTask()
        #expect(session.scenario?.state == .ready)
        #expect(session.sidebarSelection == .newTask)
        #expect(!session.isWorkPanelPresented)

        session.draft = "Keep this draft"
        session.submitDraft()
        #expect(session.draft == "Keep this draft")
        #expect(session.actionNotice != nil)
    }

    @Test
    func needsAttentionUsesTheSharedApprovalFixture() async {
        let session = DesktopSession()

        await session.loadFoundationFixture()
        session.sidebarSelection = .needsAttention
        session.applySidebarSelection()

        #expect(session.scenario?.state == .approval)
        #expect(session.isWorkPanelPresented)
    }

    @Test
    func everySharedFixtureStateCanDriveTheShell() async {
        let session = DesktopSession()
        await session.loadFoundationFixture()

        for state in DesktopFixtureState.allCases {
            session.showFixture(state)
            #expect(session.scenario?.state == state)
        }
    }
}
