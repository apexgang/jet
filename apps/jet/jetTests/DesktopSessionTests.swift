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
        #expect(session.scenario?.state == .active)
        #expect(session.sidebarSelection == .newTask)
        #expect(session.selectedConversationID == nil)
        #expect(!session.isWorkPanelPresented)

        session.draft = "Keep this draft"
        await session.submitDraft()
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
            crafts: [],
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
                    crafts: [],
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

    @Test
    func timelineProjectsOnlyPortableInertPresentation() throws {
        let turnID = UUID()
        let conversationID = UUID()
        let input = event(
            sequence: 7,
            conversationID: conversationID,
            kind: "turn.input",
            payload: #"{"turn_id":"\#(turnID.uuidString)","text":"Ship Wave 1.3"}"#
        )
        let output = event(
            sequence: 8,
            conversationID: conversationID,
            kind: "run.output",
            payload: #"{"native_json":"{\"secret\":true}","presentation_json":["{\"kind\":\"markdown\",\"text\":\"**Done**\"}","{\"kind\":\"future\",\"private\":\"hidden\"}"]}"#
        )

        #expect(input.timelineProjections() == [
            JetTimelineEntry(
                id: turnID.uuidString.lowercased(),
                kind: .user,
                text: "Ship Wave 1.3",
                sequence: 7,
                rawCount: 0
            ),
        ])
        let projected = output.timelineProjections()
        #expect(projected.count == 2)
        #expect(projected[0].kind == .agent)
        #expect(projected[0].text == "**Done**")
        #expect(projected[1].kind == .activity)
        #expect(!projected.map(\.text).joined().contains("secret"))
        #expect(!projected.map(\.text).joined().contains("private"))
    }

    @Test
    func selectingAnotherConversationClearsThePreviousTimeline() {
        let first = JetConversationSummary(
            id: UUID(), title: "First", createdAtUnixMilliseconds: 1, projectID: nil
        )
        let second = JetConversationSummary(
            id: UUID(), title: "Second", createdAtUnixMilliseconds: 2, projectID: nil
        )
        let session = DesktopSession()
        session.conversations = [first, second]
        session.selectedConversationID = first.id
        session.timeline = [
            JetTimelineEntry(
                id: "old", kind: .agent, text: "Old task", sequence: 1, rawCount: 0
            ),
        ]

        session.selectConversation(second.id)

        #expect(session.selectedConversationID == second.id)
        #expect(session.timeline.isEmpty)
    }

    @Test
    func approvalProjectionKeepsTheActionInertAndRetryBoundToTheReview() throws {
        let runID = UUID()
        let reviewID = UUID()
        let requested = event(
            sequence: 10,
            conversationID: UUID(),
            runID: runID,
            kind: "approval.requested",
            payload: #"{"request":{"request_id":"req-1","tool":"shell","action":"{\"cwd\":\"/tmp/project\",\"command\":\"make test\"}"}}"#
        )
        let reviewed = event(
            sequence: 11,
            conversationID: requested.conversationID!,
            runID: runID,
            kind: "approval.reviewed",
            payload: #"{"review":{"review_id":"\#(reviewID.uuidString)","request":{"request_id":"req-1","tool":"shell","action":"{\"cwd\":\"/tmp/project\",\"command\":\"make test\"}"},"outcome":{"status":"denied","reason":"Outside the current policy."}}}"#
        )

        let first = try #require(requested.timelineProjections().first)
        let second = try #require(reviewed.timelineProjections().first)
        #expect(first.id == second.id)
        #expect(first.approval?.target == "/tmp/project")
        #expect(second.approval?.reviewID == reviewID)
        #expect(second.approval?.canAuthorizeRetry == true)
        #expect(second.approval?.action.contains("make test") == true)
    }

    @Test
    func interruptAndStopKeepDistinctLifecycleFeedback() {
        #expect(
            JetRunTermination(control: .interruptTurn, stage: .nativeCancellation)
                .summary.contains("next Turn")
        )
        #expect(
            JetRunTermination(control: .stopRun, stage: .kill)
                .summary.contains("Run stopped")
        )
    }

    @Test
    func composerHonorsQueueAndUtf8Limits() {
        let session = DesktopSession()
        session.draft = String(repeating: "a", count: JetTurnQueue.maximumPromptBytes + 1)
        #expect(!session.canSubmitDraft)

        session.draft = "next"
        session.turnQueue = JetTurnQueue(
            cursor: 1,
            turns: (1 ... JetTurnQueue.maximumEntries).map { position in
                JetTurnQueueEntry(
                    id: UUID(),
                    sequence: UInt64(position),
                    position: position,
                    source: .user,
                    state: .queued,
                    runID: nil,
                    withdrawable: false
                )
            }
        )
        #expect(session.queueIsFull)
        #expect(!session.canSubmitDraft)
    }

    private func event(
        sequence: UInt64,
        conversationID: UUID,
        runID: UUID? = nil,
        kind: String,
        payload: String
    ) -> JetEvent {
        JetEvent(
            sequence: sequence,
            eventID: UUID(),
            actor: JetRawJSON(source: #"{"interactive_client":{}}"#),
            origin: nil,
            recordedAtUnixMilliseconds: 1,
            conversationID: conversationID,
            runID: runID,
            kind: kind,
            payloadVersion: 1,
            payload: JetRawJSON(source: payload)
        )
    }
}
