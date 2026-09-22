import Foundation
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

    @Test
    func authOptionsComeOnlyFromInstalledSupportedHarnesses() {
        let capabilities = JetCapabilitySummary(
            coreVersion: "test",
            platform: "macos",
            harnesses: ["codex", "unknown", "claude-code", "Codex CLI"],
            credentialStore: .available,
            degraded: []
        )

        #expect(capabilities.authProviders.map(\.provider) == ["openai", "anthropic"])
    }

    @Test
    func selectingAKnownProjectUpdatesClientOwnedSelection() {
        let project = JetProjectSummary(id: UUID(), root: "/tmp/jet-project")
        let session = DesktopSession()
        session.setupState = .ready(
            JetSetupSnapshot(
                status: JetPlaneStatus(
                    cursor: 1,
                    planeID: UUID(),
                    daemonStarts: 1,
                    startedAtUnixMilliseconds: 1,
                    coreVersion: "test",
                    security: nil,
                    recovery: nil
                ),
                capabilities: JetCapabilitySummary(
                    coreVersion: "test",
                    platform: "macos",
                    harnesses: [],
                    credentialStore: .available,
                    degraded: []
                ),
                projects: JetProjectList(cursor: 1, projects: [project]),
                accounts: JetAccountBindingList(cursor: 1, bindings: []),
                pairing: JetPairingSummary(
                    cursor: 1,
                    gate: "available",
                    pairedClients: 0,
                    hasPendingOffer: false
                )
            )
        )

        session.selectProject(project.id)

        #expect(session.selectedProjectID == project.id)
        #expect(session.sidebarSelection == .project)
        #expect(!session.isWorkPanelPresented)
    }

    @Test
    func liveTransportStateDrivesThePlaneFooter() {
        let session = DesktopSession()
        session.setupState = .loading
        session.connectionState = .reconnecting(attempt: 2)

        #expect(session.planeConnectionLabel == "Reconnecting")
        #expect(!session.planeIsConnected)

        session.connectionState = .connected(
            JetNegotiation(
                protocolVersion: 1,
                minorVersion: 43,
                codec: "json-v1",
                frameLimits: .protocolMaximum
            )
        )

        #expect(session.planeConnectionLabel == "Connected")
        #expect(session.planeIsConnected)
    }
}
