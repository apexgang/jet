import Foundation
import Testing
@testable import jet

@MainActor
struct SidebarSearchTests {
    // MARK: - Title filter and query limits

    @Test
    func titleFilterMatchesEveryTermIgnoringCaseAndDiacritics() {
        let login = Self.conversation("Fix login redirect loop")
        let menu = Self.conversation("Café menu")
        let loginPage = Self.conversation("Login page copy")
        let other = Self.conversation("Update payment dependencies")
        let all = [login, menu, loginPage, other]

        #expect(SidebarSearch.titleMatches(all, query: "LOGIN fix") == [login])
        #expect(SidebarSearch.titleMatches(all, query: "cafe") == [menu])
        #expect(SidebarSearch.titleMatches(all, query: "  login  ") == [login, loginPage])
        #expect(SidebarSearch.titleMatches(all, query: "login zebra").isEmpty)
    }

    @Test
    func remoteSearchRunsOnlyForValidQueries() {
        let terms = { (count: Int) in (1 ... count).map { "t\($0)" }.joined(separator: " ") }

        #expect(!SidebarSearch.isRemoteSearchable(""))
        #expect(SidebarSearch.isRemoteSearchable("login"))
        #expect(SidebarSearch.isRemoteSearchable(terms(16)))
        #expect(!SidebarSearch.isRemoteSearchable(terms(17)))
        #expect(SidebarSearch.isRemoteSearchable(String(repeating: "a", count: 256)))
        #expect(!SidebarSearch.isRemoteSearchable(String(repeating: "a", count: 257)))
        // Bytes, not characters: 128 two-byte characters fit, 129 don't.
        #expect(SidebarSearch.isRemoteSearchable(String(repeating: "é", count: 128)))
        #expect(!SidebarSearch.isRemoteSearchable(String(repeating: "é", count: 129)))
    }

    // MARK: - Search presentation

    @Test
    func otherMatchesSkipTitleMatchesAndKeepOneRowPerTask() {
        let local = UUID()
        let studio = UUID()
        let login = Self.conversation("Fix login redirect loop")
        let helpers = Self.conversation("Clean up session helpers")
        let unloaded = UUID()
        let result = Self.result(
            hits: [
                Self.hit(login.id, plane: local, .name, "Fix login redirect loop"),
                Self.hit(helpers.id, plane: local, .path, "src/auth/login.ts"),
                Self.hit(unloaded, plane: studio, name: "Studio Mac", .branch, "jet/login-redirect"),
                Self.hit(helpers.id, plane: local, .name, "login helpers"),
            ],
            cursors: [local: 10, studio: 4]
        )

        let presentation = SidebarSearchPresentation.make(
            query: "login",
            conversations: [login, helpers],
            result: result,
            resultQuery: "login",
            computers: [(id: local, name: "This Mac"), (id: studio, name: "Studio Mac")],
            remoteSearchApplies: true
        )

        #expect(presentation.titleMatches == [login])
        #expect(presentation.otherMatches == [
            SidebarOtherMatch(
                conversationID: helpers.id, planeRegistryID: local, planeName: "This Mac",
                field: .path, excerpt: "src/auth/login.ts", title: "Clean up session helpers"
            ),
            SidebarOtherMatch(
                conversationID: unloaded, planeRegistryID: studio, planeName: "Studio Mac",
                field: .branch, excerpt: "jet/login-redirect", title: nil
            ),
        ])
        #expect(presentation.failures.isEmpty)
        #expect(!presentation.isPending)
        #expect(!presentation.showsNoResults)
    }

    @Test
    func otherMatchTitleFallsBackToTheComputerName() {
        let match = SidebarOtherMatch(
            conversationID: UUID(), planeRegistryID: UUID(), planeName: "Studio Mac",
            field: .branch, excerpt: "jet/login-redirect", title: nil
        )
        #expect(match.displayTitle == "Task on Studio Mac")

        let untitled = SidebarOtherMatch(
            conversationID: UUID(), planeRegistryID: UUID(), planeName: "Studio Mac",
            field: .path, excerpt: "README.md", title: "  "
        )
        #expect(untitled.displayTitle == "Untitled task")
    }

    @Test
    func searchPresentationHidesResultsFromAnEarlierQuery() {
        let local = UUID()
        let result = Self.result(
            hits: [Self.hit(UUID(), plane: local, .path, "src/auth/login.ts")],
            cursors: [:]
        )

        let presentation = SidebarSearchPresentation.make(
            query: "login",
            conversations: [],
            result: result,
            resultQuery: "logi",
            computers: [(id: local, name: "This Mac")],
            remoteSearchApplies: true
        )

        #expect(presentation.otherMatches.isEmpty)
        #expect(presentation.failures.isEmpty)
        #expect(presentation.isPending)
        #expect(!presentation.showsNoResults)
    }

    @Test
    func searchPresentationReportsComputersThatCouldNotBeSearched() {
        let local = UUID()
        let studio = UUID()
        let office = UUID()
        let result = JetFederatedSearchResult(
            hits: [],
            cursors: [local: 10],
            indexedThrough: [local: 10],
            failures: [studio: .offline]
        )

        let presentation = SidebarSearchPresentation.make(
            query: "login",
            conversations: [],
            result: result,
            resultQuery: "login",
            computers: [(id: local, name: "This Mac"), (id: studio, name: "Studio Mac"), (id: office, name: "Office Mac")],
            remoteSearchApplies: true
        )

        // Studio Mac failed; Office Mac's client couldn't even be created.
        #expect(presentation.failures == [
            SidebarSearchFailure(planeRegistryID: studio, computerName: "Studio Mac"),
            SidebarSearchFailure(planeRegistryID: office, computerName: "Office Mac"),
        ])
        #expect(!presentation.showsNoResults)
    }

    @Test(arguments: [
        // (remote search applies, result answers the query, a title matches, a computer found a hit, no results)
        (true, false, false, false, false),
        (true, false, true, false, false),
        (true, true, false, false, true),
        (true, true, true, false, false),
        (true, true, false, true, false),
        (false, false, false, false, true),
        (false, false, true, false, false),
        // Fixture mode and overlong queries never wait for a search, even with an old result.
        (false, true, false, true, true),
    ])
    func noResultsShowsOnlyWhenNothingIsPending(
        remoteSearchApplies: Bool,
        resultIsCurrent: Bool,
        titleMatches: Bool,
        remoteHit: Bool,
        showsNoResults: Bool
    ) {
        let local = UUID()
        let conversations = titleMatches ? [Self.conversation("Fix login redirect loop")] : []
        let result = Self.result(
            hits: remoteHit ? [Self.hit(UUID(), plane: local, .path, "src/auth/login.ts")] : [],
            cursors: [local: 1]
        )

        let presentation = SidebarSearchPresentation.make(
            query: "login",
            conversations: conversations,
            result: resultIsCurrent ? result : nil,
            resultQuery: resultIsCurrent ? "login" : nil,
            computers: [(id: local, name: "This Mac")],
            remoteSearchApplies: remoteSearchApplies
        )

        #expect(presentation.showsNoResults == showsNoResults)
        #expect(presentation.isPending == (remoteSearchApplies && !resultIsCurrent))
    }

    // MARK: - Library

    @Test
    func eachTaskAppearsInExactlyOneSection() {
        let tasks = (1 ... 5).map { Self.conversation("Task \($0)") }
        let unloaded = UUID()

        let sections = SidebarSections.build(
            conversations: tasks,
            needsYouIDs: [tasks[1].id, tasks[3].id, unloaded]
        )

        #expect(sections.needsYou == [tasks[1], tasks[3]])
        #expect(sections.tasks == [tasks[0], tasks[2], tasks[4]])
        let listed = (sections.needsYou + sections.tasks).map(\.id)
        #expect(Set(listed) == Set(tasks.map(\.id)))
        #expect(listed.count == tasks.count)
    }

    @Test
    func taskRowSecondaryLines() {
        func row(
            _ title: String = "Fix login redirect",
            project: String? = "web-app",
            computer: String? = nil,
            status: TaskStatus = .finished,
            isOffline: Bool = false
        ) -> TaskRowPresentation {
            TaskRowPresentation(
                title: title, projectName: project, computerName: computer, status: status,
                isUnread: false, isOffline: isOffline, relativeTime: "2 hr ago"
            )
        }

        #expect(row().secondaryText == "web-app · 2 hr ago")
        #expect(row(status: .needsPermission).secondaryText == "web-app · Needs permission")
        #expect(row(computer: "Studio Mac").secondaryText == "web-app · 2 hr ago · Studio Mac")
        #expect(row(status: .offline, isOffline: true).secondaryText == "web-app · Offline")
        #expect(row(status: .offline, isOffline: true).isDimmed)
        #expect(!row().isDimmed)
        #expect(row("   ").title == "Untitled task")
        #expect(row("   ").help == "Untitled task")
        #expect(row(project: nil).secondaryText == "2 hr ago")
        #expect(row(project: nil).secondaryProject == nil)
        #expect(row(computer: "Studio Mac").secondaryDetail == "2 hr ago")
    }

    @Test
    func rowAccessibilityLabelReadsLikeASentence() {
        let row = TaskRowPresentation(
            title: "Fix login redirect", projectName: "web-app", computerName: nil,
            status: .needsPermission, isUnread: true, isOffline: false, relativeTime: "2 hr ago"
        )
        #expect(row.accessibilityLabel
            == "Fix login redirect, web-app, \(TaskStatus.needsPermission.accessibilityDescription), new reply")

        let offline = TaskRowPresentation(
            title: "Profile slow checkout query", projectName: nil, computerName: "Studio Mac",
            status: .offline, isUnread: false, isOffline: true, relativeTime: "3 hr ago"
        )
        #expect(offline.accessibilityLabel == "Profile slow checkout query, Offline, Studio Mac")

        let unknown = TaskRowPresentation(
            title: "Explain the retry logic", projectName: "billing", computerName: nil,
            status: .unknown, isUnread: false, isOffline: false, relativeTime: "yesterday"
        )
        #expect(unknown.accessibilityLabel == "Explain the retry logic, billing, yesterday")
    }

    @Test(arguments: [
        (false, false, JetConversationFreshness.loading, SidebarListState.empty),
        (false, true, JetConversationFreshness.failed, SidebarListState.ready),
        (true, true, JetConversationFreshness.loading, SidebarListState.ready),
        (true, true, JetConversationFreshness.failed, SidebarListState.ready),
        (true, false, JetConversationFreshness.loading, SidebarListState.loading),
        (true, false, JetConversationFreshness.failed, SidebarListState.failed),
        (true, false, JetConversationFreshness.live, SidebarListState.empty),
        (true, false, JetConversationFreshness.cached, SidebarListState.empty),
    ])
    func listStateMatrix(
        usesLivePlane: Bool,
        hasTasks: Bool,
        freshness: JetConversationFreshness,
        expected: SidebarListState
    ) {
        #expect(SidebarListState.make(usesLivePlane: usesLivePlane, hasTasks: hasTasks, freshness: freshness) == expected)
    }

    @Test
    func computerOfflineDetection() {
        let session = Self.connected()
        let local = session.localPlaneRegistryID
        #expect(!session.isComputerOffline(local))
        session.connectionState = .failed(.offline)
        #expect(session.isComputerOffline(local))
        // This Mac is the window banner's concern, never a sidebar row.
        #expect(session.sidebarOfflineComputers.isEmpty)

        let studio = Self.addRemote(session, connection: .connecting)
        #expect(!session.isComputerOffline(studio))

        session.updatePlane(studio) { $0.connection = .reconnecting(attempt: 1) }
        #expect(!session.isComputerOffline(studio))
        session.updatePlane(studio) { $0.failure = .offline }
        #expect(session.isComputerOffline(studio))
        #expect(session.sidebarOfflineComputers.map(\.id) == [studio])

        // A search error left on a connected computer never makes it offline.
        session.updatePlane(studio) { $0.connection = .connected(Self.negotiation) }
        #expect(!session.isComputerOffline(studio))
        #expect(session.sidebarOfflineComputers.isEmpty)
    }

    @Test
    func offlineComputerDimsItsRowsAndNamesTheComputer() {
        let session = Self.connected()
        let studio = Self.addRemote(session, connection: .failed(.offline), failure: .offline)
        let remote = Self.addTask(session, "Profile slow checkout query", on: studio)
        let local = Self.addTask(session, "Fix login redirect loop")

        let remoteRow = session.sidebarRowPresentation(for: remote)
        #expect(remoteRow.isDimmed)
        #expect(remoteRow.secondaryText == "Offline · Studio Mac")

        let localRow = session.sidebarRowPresentation(for: local)
        #expect(!localRow.isDimmed)
        #expect(localRow.secondaryProject == "web-app")
        #expect(localRow.secondaryComputer == "This Mac")
        #expect(!session.sidebarCanChangeTask(remote.id))
        #expect(session.sidebarCanChangeTask(local.id))
    }

    // MARK: - Running a search

    @Test
    func emptyQueryClearsEarlierResults() async {
        let session = Self.connected(isPreviewSession: false)
        session.searchResult = JetFederatedSearchResult(hits: [], cursors: [:], indexedThrough: [:], failures: [:])
        session.searchText = "   "

        let answered = await session.runSidebarSearch(delay: .zero)

        #expect(answered == nil)
        #expect(session.searchResult == nil)
        #expect(!session.isSidebarSearching)
    }

    @Test
    func overlongQueryNeverContactsAComputer() async {
        let session = Self.connected(isPreviewSession: false)
        session.searchText = (1 ... 17).map { "t\($0)" }.joined(separator: " ")

        let answered = await session.runSidebarSearch(delay: .zero)

        #expect(answered == nil)
        #expect(session.searchResult == nil)
        #expect(session.planes.allSatisfy { $0.failure == nil })
        let presentation = session.sidebarSearchPresentation(resultQuery: answered)
        #expect(!presentation.isPending)
    }

    @Test
    func unreachableComputerIsReportedAfterSearch() async {
        let session = DesktopSession(
            makeJetClient: { throw JetClientFailure.presentation(.offline) },
            notificationPreference: { _ in false },
            memory: Self.memory()
        )
        session.searchText = " login "

        let answered = await session.runSidebarSearch(delay: .zero)

        #expect(answered == "login")
        let presentation = session.sidebarSearchPresentation(resultQuery: answered)
        #expect(presentation.failures.map(\.computerName) == ["This Mac"])
        #expect(!presentation.isPending)
        #expect(!presentation.showsNoResults)
    }

    // MARK: - Navigation and row actions

    @Test
    func projectOnAnotherComputerSelectsThatComputer() {
        let session = Self.connected()
        let project = JetProjectSummary(id: UUID(), root: "/Users/alex/code/api-server")
        let studio = Self.addRemote(session, connection: .connected(Self.negotiation), projects: [project])

        #expect(session.sidebarProjects.map(\.project.id) == [Self.project.id, project.id])
        session.selectFromSidebar(.project(project.id))

        #expect(session.sidebarSelection == .project)
        #expect(session.selectedProjectID == project.id)
        #expect(session.newTaskPlaneRegistryID == studio)
        #expect(session.selectedPlaneRegistryID == studio)
        #expect(session.sidebarItem == .project(project.id))
    }

    @Test
    func renameFromSidebarSelectsTheTaskFirst() {
        let session = Self.connected()
        let first = Self.addTask(session, "Fix login redirect loop")
        let second = Self.addTask(session, "Update payment dependencies")
        session.selectFromSidebar(.task(first.id))

        session.renameFromSidebar(second.id)

        #expect(session.selectedConversationID == second.id)
        #expect(session.presentedSheet == .rename(
            ConversationRef(conversationID: second.id, planeRegistryID: session.localPlaneRegistryID)
        ))
    }

    @Test
    func moveToTrashFromSidebarKeepsSelection() {
        let session = Self.connected()
        let first = Self.addTask(session, "Fix login redirect loop")
        let second = Self.addTask(session, "Update payment dependencies")
        session.selectFromSidebar(.task(first.id))

        session.moveToTrashFromSidebar(second.id)

        #expect(session.selectedConversationID == first.id)
        #expect(session.presentedSheet == .moveToTrash(
            ConversationRef(conversationID: second.id, planeRegistryID: session.localPlaneRegistryID),
            deleteEverywhere: false
        ))

        session.dismissSheet()
        session.repeatDailyFromSidebar(second.id)
        #expect(session.selectedConversationID == first.id)
        #expect(session.presentedSheet == .repeatDaily(
            ConversationRef(conversationID: second.id, planeRegistryID: session.localPlaneRegistryID)
        ))
    }

    @Test
    func returnOnARowFocusesTheComposer() {
        let session = Self.connected()
        let task = Self.addTask(session, "Fix login redirect loop")
        let before = session.composerFocusRequest

        #expect(session.activateSidebarItem(.task(task.id)))
        #expect(session.activateSidebarItem(.newTask))
        #expect(!session.activateSidebarItem(.trash))
        #expect(!session.activateSidebarItem(.project(Self.project.id)))
        #expect(!session.activateSidebarItem(nil))
        #expect(session.composerFocusRequest == before + 2)

        session.selectFromSidebar(.task(task.id))
        session.beginNewTaskFromSidebar()
        #expect(session.sidebarItem == .newTask)
        #expect(session.composerFocusRequest == before + 3)
        // Already on New Task, the toolbar button still moves focus to the composer.
        session.beginNewTaskFromSidebar()
        #expect(session.composerFocusRequest == before + 4)
    }

    @Test
    func droppedFoldersOpenAddProjectForThisMac() throws {
        let session = Self.connected()
        let folder = FileManager.default.temporaryDirectory
            .appending(path: "jet-sidebar-drop-\(UUID().uuidString)", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let file = folder.appending(path: "notes.txt")
        try Data("notes".utf8).write(to: file)

        #expect(!session.addProjectFromSidebarDrop([file]))
        #expect(session.presentedSheet == nil)
        #expect(!session.addProjectFromSidebarDrop([URL(string: "https://example.com")!]))

        #expect(session.addProjectFromSidebarDrop([file, folder]))
        #expect(session.presentedSheet == .addProject(
            planeRegistryID: session.localPlaneRegistryID,
            droppedURL: folder
        ))
    }

    @Test
    func keptBranchNameComesFromTheOpenTasksLastCompletedStep() {
        let session = Self.connected()
        let task = Self.addTask(session, "Fix login redirect loop")
        session.selectFromSidebar(.task(task.id))
        #expect(session.sidebarKeptBranchName == nil)

        session.gitDeliveries = [
            Self.delivery(task.id, .completed(head: "abc", branch: "jet/first", pullRequest: nil)),
            Self.delivery(task.id, .completed(head: "def", branch: "jet/fix-login-redirect", pullRequest: nil)),
            Self.delivery(task.id, .failed(code: "git.push_rejected")),
            Self.delivery(UUID(), .completed(head: "123", branch: "jet/other", pullRequest: nil)),
        ]
        #expect(session.sidebarKeptBranchName == "jet/fix-login-redirect")
    }

#if DEBUG
    @Test
    func previewLibraryListsEverySeededTaskOnce() {
        let session = DesktopSession.preview { DesktopPreviewData.connect($0) }
        let sections = session.sidebarSections
        let listed = (sections.needsYou + sections.tasks).map(\.id)

        #expect(listed.count == DesktopPreviewData.conversations.count)
        #expect(Set(listed) == Set(DesktopPreviewData.conversations.map(\.id)))
        #expect(sections.needsYou.map(\.id) == [DesktopPreviewData.paymentDependencies.id])
        #expect(session.sidebarListState == .ready)
    }
#endif

    // MARK: - Helpers

    nonisolated static let project = JetProjectSummary(id: UUID(), root: "/Users/alex/code/web-app")
    static let negotiation = JetNegotiation(
        protocolVersion: 1, minorVersion: 43, codec: "json-v1", frameLimits: .protocolMaximum
    )

    static func memory() -> ClientMemory {
        let suite = "jet.tests.sidebar.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        return ClientMemory(defaults: defaults)
    }

    static func snapshot(projects: [JetProjectSummary]) -> JetSetupSnapshot {
        JetSetupSnapshot(
            status: JetPlaneStatus(
                cursor: 1, planeID: UUID(), daemonStarts: 1, startedAtUnixMilliseconds: 1,
                coreVersion: "test", security: nil, recovery: nil
            ),
            capabilities: JetCapabilitySummary(
                coreVersion: "test",
                platform: "macos",
                externalTools: [],
                harnesses: [],
                crafts: [],
                credentialStore: .available,
                degraded: []
            ),
            projects: JetProjectList(cursor: 1, projects: projects),
            accounts: JetAccountBindingList(cursor: 1, bindings: []),
            pairing: JetPairingSummary(cursor: 1, gate: "closed", pairedClients: 0, hasPendingOffer: false)
        )
    }

    /// This Mac, connected, with one project. Requests fail without reaching a Plane.
    static func connected(isPreviewSession: Bool = true) -> DesktopSession {
        let session = DesktopSession(
            makeJetClient: { throw CancellationError() },
            notificationPreference: { _ in false },
            memory: memory(),
            isPreviewSession: isPreviewSession
        )
        let snapshot = snapshot(projects: [project])
        session.setupState = .ready(snapshot)
        session.connectionState = .connected(negotiation)
        session.updatePlane(session.localPlaneRegistryID) { plane in
            plane.connection = .connected(negotiation)
            plane.snapshot = snapshot
        }
        session.conversationFreshness = .live
        return session
    }

    @discardableResult
    static func addRemote(
        _ session: DesktopSession,
        connection: JetConnectionState,
        failure: JetPresentationError? = nil,
        projects: [JetProjectSummary]? = nil
    ) -> UUID {
        let id = UUID()
        session.planes.append(JetPlanePresentation(
            id: id, name: "Studio Mac", endpoint: "alex@studio.example", isLocal: false,
            planeID: UUID(), connection: connection,
            snapshot: projects.map { snapshot(projects: $0) },
            failure: failure, conversationCursor: nil
        ))
        return id
    }

    static func addTask(
        _ session: DesktopSession,
        _ title: String,
        on planeRegistryID: UUID? = nil
    ) -> JetConversationSummary {
        let plane = planeRegistryID ?? session.localPlaneRegistryID
        let conversation = JetConversationSummary(
            id: UUID(), revision: 1, title: title,
            createdAtUnixMilliseconds: Int64(session.conversations.count + 1) * 60_000,
            projectID: planeRegistryID == nil ? project.id : UUID()
        )
        session.planeConversations[plane, default: []].append(conversation)
        session.conversationPlaneRegistryIDs[conversation.id] = plane
        session.rebuildConversationAggregation()
        return conversation
    }

    static func conversation(_ title: String) -> JetConversationSummary {
        JetConversationSummary(
            id: UUID(), revision: 1, title: title, createdAtUnixMilliseconds: 1, projectID: nil
        )
    }

    static func hit(
        _ conversationID: UUID,
        plane: UUID,
        name: String = "This Mac",
        _ field: JetSearchField,
        _ excerpt: String
    ) -> JetFederatedSearchHit {
        JetFederatedSearchHit(
            planeRegistryID: plane,
            planeName: name,
            hit: JetSearchHit(conversationID: conversationID, sequence: 1, field: field, excerpt: excerpt)
        )
    }

    static func result(hits: [JetFederatedSearchHit], cursors: [UUID: UInt64]) -> JetFederatedSearchResult {
        JetFederatedSearchResult(hits: hits, cursors: cursors, indexedThrough: cursors, failures: [:])
    }

    static func delivery(_ conversationID: UUID, _ outcome: JetGitDeliveryOutcome) -> JetGitDelivery {
        JetGitDelivery(
            id: UUID(),
            conversationID: conversationID,
            checkpoint: nil,
            operation: .branch(name: "jet/fix-login-redirect"),
            policy: JetGitDeliveryPolicy(
                automatic: false, branch: true, commit: false, push: false,
                draftPullRequest: false, branchPrefix: "jet/"
            ),
            utilityJobID: nil,
            message: nil,
            acknowledgedBy: nil,
            outcome: outcome
        )
    }
}
