import Foundation
#if os(macOS)
import AppKit
#endif

// The sidebar's state and actions (design §6.3, §6.15, §6.16): its sections, row
// copy, Search Tasks and the row menus. Views read these; they never do I/O.

// MARK: - Search

/// Search Tasks: titles filter instantly, and a debounced search asks every
/// computer for names, files and branches.
enum SidebarSearch {
    /// How long typing must pause before every computer is searched.
    nonisolated static let debounce: Duration = .milliseconds(300)
    /// The limits `JetClient.searchConversations` enforces.
    nonisolated static let maximumTerms = 16
    nonisolated static let maximumBytes = 256

    static func normalized(_ text: String) -> String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    static func terms(_ query: String) -> [String] {
        query.split(whereSeparator: \.isWhitespace).map(String.init)
    }

    /// Loaded tasks whose title contains every term, ignoring case and diacritics,
    /// in their list order.
    static func titleMatches(
        _ conversations: [JetConversationSummary],
        query: String
    ) -> [JetConversationSummary] {
        let terms = terms(normalized(query))
        return conversations.filter { conversation in
            terms.allSatisfy { conversation.title.localizedStandardContains($0) }
        }
    }

    /// A query the computers accept: 1 to 16 terms in at most 256 UTF-8 bytes.
    static func isRemoteSearchable(_ query: String) -> Bool {
        (1 ... maximumTerms).contains(terms(query).count) && query.utf8.count <= maximumBytes
    }
}

/// A task a computer found by name, file or branch that the title filter didn't.
struct SidebarOtherMatch: Identifiable, Equatable {
    let conversationID: UUID
    let planeRegistryID: UUID
    let planeName: String
    let field: JetSearchField
    let excerpt: String
    /// The task's title when it is loaded, otherwise nil.
    let title: String?

    var id: UUID { conversationID }

    var displayTitle: String {
        guard let title else { return String(localized: "Task on \(planeName)") }
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? String(localized: "Untitled task") : trimmed
    }
}

/// A computer the last search couldn't reach.
struct SidebarSearchFailure: Identifiable, Equatable {
    let planeRegistryID: UUID
    let computerName: String

    var id: UUID { planeRegistryID }
}

/// What the sidebar shows while Search Tasks has text.
struct SidebarSearchPresentation: Equatable {
    var titleMatches: [JetConversationSummary]
    var otherMatches: [SidebarOtherMatch]
    var failures: [SidebarSearchFailure]
    /// The computers are still being searched for this query.
    var isPending: Bool
    var showsNoResults: Bool

    /// - Parameters:
    ///   - result: the session's latest search result, which may belong to an
    ///     earlier query.
    ///   - resultQuery: the query `result` answers, or nil when unknown.
    ///   - remoteSearchApplies: the computers are searched for this query.
    static func make(
        query: String,
        conversations: [JetConversationSummary],
        result: JetFederatedSearchResult?,
        resultQuery: String?,
        computers: [(id: UUID, name: String)],
        remoteSearchApplies: Bool
    ) -> SidebarSearchPresentation {
        let titleMatches = SidebarSearch.titleMatches(conversations, query: query)
        var otherMatches: [SidebarOtherMatch] = []
        var failures: [SidebarSearchFailure] = []
        let current = remoteSearchApplies && result != nil && resultQuery == query
        if current, let result {
            var shown = Set(titleMatches.map(\.id))
            let titles = Dictionary(
                conversations.map { ($0.id, $0.title) },
                uniquingKeysWith: { first, _ in first }
            )
            for hit in result.hits where !shown.contains(hit.hit.conversationID) {
                shown.insert(hit.hit.conversationID)
                otherMatches.append(SidebarOtherMatch(
                    conversationID: hit.hit.conversationID,
                    planeRegistryID: hit.planeRegistryID,
                    planeName: hit.planeName,
                    field: hit.hit.field,
                    excerpt: hit.hit.excerpt,
                    title: titles[hit.hit.conversationID]
                ))
            }
            // A computer without a cursor either failed or couldn't be reached at all.
            failures = computers
                .filter { result.cursors[$0.id] == nil }
                .map { SidebarSearchFailure(planeRegistryID: $0.id, computerName: $0.name) }
        }
        let isPending = remoteSearchApplies && !current
        return SidebarSearchPresentation(
            titleMatches: titleMatches,
            otherMatches: otherMatches,
            failures: failures,
            isPending: isPending,
            showsNoResults: !isPending && titleMatches.isEmpty && otherMatches.isEmpty && failures.isEmpty
        )
    }
}

// MARK: - Library

/// The two task sections. Each task appears in exactly one of them, newest first.
struct SidebarSections: Equatable {
    var needsYou: [JetConversationSummary]
    var tasks: [JetConversationSummary]

    static func build(
        conversations: [JetConversationSummary],
        needsYouIDs: Set<UUID>
    ) -> SidebarSections {
        var sections = SidebarSections(needsYou: [], tasks: [])
        for conversation in conversations {
            if needsYouIDs.contains(conversation.id) {
                sections.needsYou.append(conversation)
            } else {
                sections.tasks.append(conversation)
            }
        }
        return sections
    }
}

/// What the Tasks section shows.
enum SidebarListState: Equatable, Sendable {
    case loading
    case failed
    case empty
    case ready

    static func make(
        usesLivePlane: Bool,
        hasTasks: Bool,
        freshness: JetConversationFreshness
    ) -> SidebarListState {
        if hasTasks { return .ready }
        guard usesLivePlane else { return .empty }
        switch freshness {
        case .loading: return .loading
        case .failed: return .failed
        case .live, .cached: return .empty
        }
    }
}

/// The copy of one task row: "web-app · 2 hr ago", read by VoiceOver as one element.
struct TaskRowPresentation: Equatable {
    let title: String
    /// The whole secondary line, "web-app · 2 hr ago · Studio Mac".
    let secondaryText: String
    /// The secondary line's parts, so a narrow row can shorten the project first.
    let secondaryProject: String?
    let secondaryDetail: String
    let secondaryComputer: String?
    let accessibilityLabel: String
    let help: String
    /// Rows on an offline computer use the secondary style.
    let isDimmed: Bool
    let status: TaskStatus
    let isUnread: Bool

    init(
        title: String,
        projectName: String?,
        computerName: String?,
        status: TaskStatus,
        isUnread: Bool,
        isOffline: Bool,
        relativeTime: String
    ) {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        let shownTitle = trimmed.isEmpty ? String(localized: "Untitled task") : trimmed
        let offline = String(localized: "Offline")
        // In Needs You the status replaces the time.
        let detail = isOffline ? offline : (status.needsYou ? status.title : relativeTime)
        let statusPhrase = status == .unknown || isOffline ? nil : status.accessibilityDescription
        let timePhrase = isOffline ? offline : (status.needsYou ? nil : relativeTime)

        self.title = shownTitle
        help = shownTitle
        secondaryText = Self.joined([projectName, detail, computerName], separator: " · ")
        secondaryProject = projectName.flatMap { $0.isEmpty ? nil : $0 }
        secondaryDetail = detail
        secondaryComputer = computerName.flatMap { $0.isEmpty ? nil : $0 }
        accessibilityLabel = Self.joined(
            [
                shownTitle, projectName, statusPhrase, timePhrase, computerName,
                isUnread ? String(localized: "new reply") : nil,
            ],
            separator: ", "
        )
        isDimmed = isOffline
        self.status = status
        self.isUnread = isUnread
    }

    private static func joined(_ parts: [String?], separator: String) -> String {
        parts.compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: separator)
    }
}

// MARK: - Session

extension DesktopSession {
    // MARK: Derived state

    var sidebarSearchQuery: String { SidebarSearch.normalized(searchText) }

    var isSidebarSearching: Bool { !sidebarSearchQuery.isEmpty }

    var sidebarListState: SidebarListState {
        SidebarListState.make(
            usesLivePlane: usesLivePlane,
            hasTasks: !conversations.isEmpty,
            freshness: conversationFreshness
        )
    }

    var sidebarSections: SidebarSections {
        SidebarSections.build(conversations: conversations, needsYouIDs: Set(needsYouConversationIDs))
    }

    /// Other computers that can't be reached, each shown as a row above New Task.
    /// This Mac's own connection is shown by the window's banner.
    var sidebarOfflineComputers: [JetPlanePresentation] {
        planes.filter { $0.id != localPlaneRegistryID && isComputerOffline($0.id) }
    }

    /// Every computer's projects: This Mac first, then the other computers in
    /// order, each sorted by name.
    var sidebarProjects: [JetPlaneProject] {
        let order = Dictionary(
            planes.enumerated().map { ($0.element.id, $0.offset) },
            uniquingKeysWith: { first, _ in first }
        )
        return allProjects.sorted { left, right in
            let leftOrder = left.planeRegistryID == localPlaneRegistryID ? -1 : order[left.planeRegistryID] ?? .max
            let rightOrder = right.planeRegistryID == localPlaneRegistryID ? -1 : order[right.planeRegistryID] ?? .max
            if leftOrder != rightOrder { return leftOrder < rightOrder }
            return left.project.name.localizedStandardCompare(right.project.name) == .orderedAscending
        }
    }

    /// A computer reported its projects, so an empty list really is empty.
    var sidebarKnowsProjects: Bool {
        setupSnapshot != nil || planes.contains { $0.snapshot != nil }
    }

    /// The branch the open task's changes were last kept on.
    var sidebarKeptBranchName: String? {
        guard let conversationID = selectedConversationID else { return nil }
        for delivery in gitDeliveries.reversed() where delivery.conversationID == conversationID {
            if case let .completed(_, branch?, _) = delivery.outcome, !branch.isEmpty { return branch }
        }
        return nil
    }

    // MARK: Rows

    /// The name of a task's project on the task's computer, when that computer reported it.
    func sidebarProjectName(for conversation: JetConversationSummary) -> String? {
        guard let projectID = conversation.projectID else { return nil }
        let planeRegistryID = planeRegistryID(for: conversation.id) ?? localPlaneRegistryID
        return planeSetupSnapshot(for: planeRegistryID)?.projects.projects
            .first { $0.id == projectID }?.name
    }

    /// The task's computer, named only when there are two or more computers.
    func sidebarComputerName(for conversationID: UUID) -> String? {
        guard showsComputerNames else { return nil }
        return planeName(planeRegistryID(for: conversationID) ?? localPlaneRegistryID)
    }

    func sidebarRowPresentation(
        for conversation: JetConversationSummary,
        now: Date = .now
    ) -> TaskRowPresentation {
        let planeRegistryID = planeRegistryID(for: conversation.id) ?? localPlaneRegistryID
        let status = taskStatus(for: conversation.id)
        return TaskRowPresentation(
            title: conversation.title,
            projectName: sidebarProjectName(for: conversation),
            computerName: sidebarComputerName(for: conversation.id),
            status: status,
            isUnread: isUnread(conversation.id),
            isOffline: status == .offline || isComputerOffline(planeRegistryID),
            relativeTime: JetCopy.relative(
                ms: conversation.createdAtUnixMilliseconds,
                now: sidebarReferenceDate(now)
            )
        )
    }

    /// "This Mac · Offline" under a project, or nil when there is nothing to add.
    func sidebarProjectDetail(_ project: JetPlaneProject) -> String? {
        let parts = [
            showsComputerNames ? project.planeName : nil,
            isComputerOffline(project.planeRegistryID) ? String(localized: "Offline") : nil,
        ].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// The time rows measure "2 hr ago" against. Preview sessions use the seed's
    /// fixed clock so screenshots don't drift.
    func sidebarReferenceDate(_ date: Date) -> Date {
#if DEBUG
        if isPreviewSession {
            return Date(timeIntervalSince1970: TimeInterval(DesktopPreviewData.now) / 1_000)
        }
#endif
        return date
    }

    func sidebarRef(for conversationID: UUID) -> ConversationRef? {
        planeRegistryID(for: conversationID).map {
            ConversationRef(conversationID: conversationID, planeRegistryID: $0)
        }
    }

    /// Rename, Repeat Daily and Move to Jet Trash need the task's computer.
    func sidebarCanChangeTask(_ conversationID: UUID) -> Bool {
        guard usesLivePlane, let ref = sidebarRef(for: conversationID) else { return false }
        return !isComputerOffline(ref.planeRegistryID)
    }

    func sidebarSearchPresentation(resultQuery: String?) -> SidebarSearchPresentation {
        let query = sidebarSearchQuery
        return SidebarSearchPresentation.make(
            query: query,
            conversations: conversations,
            result: searchResult,
            resultQuery: resultQuery,
            computers: planes.map { (id: $0.id, name: $0.name) },
            remoteSearchApplies: usesLivePlane && SidebarSearch.isRemoteSearchable(query)
        )
    }

    // MARK: Search

    /// Searches every computer once typing pauses. Returns the query the session's
    /// search result now answers, or nil when it answers none.
    @discardableResult
    func runSidebarSearch(delay: Duration = SidebarSearch.debounce) async -> String? {
        let query = sidebarSearchQuery
        // Preview sessions arrive with their result seeded and never search.
        if isPreviewSession { return searchResult != nil ? query : nil }
        guard !query.isEmpty else {
            // Clears the results and invalidates any search still in flight.
            await searchConversations()
            return nil
        }
        guard usesLivePlane, SidebarSearch.isRemoteSearchable(query) else { return nil }
        if delay > .zero {
            do {
                try await Task.sleep(for: delay)
            } catch {
                return nil
            }
        }
        guard sidebarSearchQuery == query else { return nil }
        let request = searchRequest
        await searchConversations()
        // A newer search replaced this one, or the text changed while it ran.
        guard !Task.isCancelled,
              searchRequest == request + 1,
              sidebarSearchQuery == query
        else { return nil }
        return query
    }

    /// Return in the field and Try Again search at once.
    @discardableResult
    func retrySidebarSearch() async -> String? {
        await runSidebarSearch(delay: .zero)
    }

    // MARK: Try Again

    /// "Couldn't load tasks": reconnects This Mac when it isn't connected,
    /// otherwise reloads the list.
    func retrySidebarTaskList() async {
        if !isPlaneConnected(localPlaneRegistryID) {
            await retryConnection()
        } else {
            await loadConversations()
        }
    }

    /// "Studio Mac is offline": refreshes every computer.
    func retrySidebarComputer(_ planeRegistryID: UUID) async {
        await refreshPlanes()
    }

    // MARK: Navigation

    /// Selects a sidebar row. It asks first about an unsaved file edit, and a
    /// project on another computer opens with that computer.
    func selectFromSidebar(_ item: SidebarItem) {
        sidebarItem = item
    }

    /// The toolbar's New Task: opens New Task and moves focus to the composer.
    func beginNewTaskFromSidebar() {
        guardUnsavedEdits { [weak self] in
            guard let self else { return }
            if sidebarItem != .newTask { open(.newTask) }
            composerFocusRequest += 1
        }
    }

    /// Return or a double-click on a row: New Task and tasks move focus to the
    /// composer. Returns false when the row has nothing to activate.
    @discardableResult
    func activateSidebarItem(_ item: SidebarItem?) -> Bool {
        switch item {
        case .newTask, .task:
            composerFocusRequest += 1
            return true
        case .project, .trash, nil:
            return false
        }
    }

    // MARK: Task rows

    /// Rename acts on the open task, so the task opens first.
    func renameFromSidebar(_ conversationID: UUID) {
        selectFromSidebar(.task(conversationID))
        guard selectedConversationID == conversationID else { return }
        presentRename()
    }

    func repeatDailyFromSidebar(_ conversationID: UUID) {
        guard let ref = sidebarRef(for: conversationID) else { return }
        presentRepeatDaily(ref)
    }

    func moveToTrashFromSidebar(_ conversationID: UUID) {
        guard let ref = sidebarRef(for: conversationID) else { return }
        presentMoveToTrash(ref, deleteEverywhere: false)
    }

    // MARK: Project rows

    /// Opens the project first, then its removal review. The review is prepared
    /// only when the project really is the open one, so it never asks another
    /// computer.
    func moveProjectFolderToTrashFromSidebar(_ project: JetPlaneProject) {
        selectFromSidebar(.project(project.project.id))
        guard sidebarSelection == .project,
              selectedProjectID == project.project.id,
              selectedPlaneRegistryID == project.planeRegistryID
        else { return }
        Task { await prepareProjectRemoval(project.project.id) }
    }

    /// Show in Finder is offered only for folders on this Mac.
    func sidebarCanRevealProject(_ project: JetPlaneProject) -> Bool {
#if os(macOS)
        return isLocalPlane(project.planeRegistryID)
#else
        return false
#endif
    }

    func sidebarRevealProject(_ project: JetPlaneProject) {
#if os(macOS)
        guard sidebarCanRevealProject(project) else { return }
        NSWorkspace.shared.activateFileViewerSelecting([
            URL(fileURLWithPath: project.project.root, isDirectory: true),
        ])
#endif
    }

    // MARK: Drops

    /// A folder dropped on New Task or Projects opens Add Project for This Mac,
    /// where the folder is. Files, and drops while something else is going on,
    /// are refused.
    @discardableResult
    func addProjectFromSidebarDrop(_ urls: [URL]) -> Bool {
        guard usesLivePlane, userOperation == nil, !isModalPresented,
              let folder = urls.first(where: Self.isFolder)
        else { return false }
        presentedSheet = .addProject(planeRegistryID: localPlaneRegistryID, droppedURL: folder)
        return true
    }

    private nonisolated static func isFolder(_ url: URL) -> Bool {
        guard url.isFileURL else { return false }
        return (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true
    }
}
