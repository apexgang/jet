import Foundation
#if os(macOS)
import AppKit
#endif

/// The Queries and Commands the library pages and sheets use (design §6.11). One
/// boundary lets tests and previews answer them without a Plane.
nonisolated protocol JetLibraryAccess: Sendable {
    func conversation(_ conversationID: UUID) async throws -> JetConversationSnapshot
    func settings(scope: JetSettingScope) async throws -> JetSettingSnapshot
    func setSetting(
        _ key: SettingKey,
        value: JetSettingValue,
        scope: JetSettingScope,
        commandID: UUID
    ) async throws -> JetSettingSet
    func clearSetting(
        _ key: SettingKey,
        scope: JetSettingScope,
        commandID: UUID
    ) async throws -> JetSettingCleared
    func scheduledTasks(conversationID: UUID) async throws -> JetScheduledTaskSnapshot
    func createSchedule(
        conversationID: UUID,
        timeZone: String,
        localTime: String,
        prompt: String,
        commandID: UUID
    ) async throws -> JetScheduledTask
    func cancelSchedule(_ scheduleID: UUID, commandID: UUID) async throws
    func systemHealth() async throws -> JetSystemHealth
    func conversationTrash() async throws -> JetTrashSnapshot
    func retentionPreview(conversationID: UUID) async throws -> JetRetentionPreview
    func stageConversation(
        _ conversationID: UUID,
        action: JetRetentionAction,
        commandID: UUID
    ) async throws -> JetTrashEntry
    func restoreConversation(_ conversationID: UUID, commandID: UUID) async throws
    /// Stop Run for one Run of the task (`stop_run`).
    func stopRun(runID: UUID, commandID: UUID) async throws
}

extension JetClient: JetLibraryAccess {
    func stopRun(runID: UUID, commandID: UUID) async throws {
        _ = try await controlRun(runID: runID, control: .stopRun, commandID: commandID)
    }
}

typealias JetLibraryAccessProvider = @MainActor (UUID) async throws -> any JetLibraryAccess

// The library's session helpers (design §6.11). The shell files are frozen, so the
// project page, Jet Trash and the library sheets reach the session through here.
extension DesktopSession {
    // MARK: - Access

    func libraryAccess(for planeRegistryID: UUID) async throws -> any JetLibraryAccess {
#if DEBUG
        if isPreviewSession { return PreviewLibraryAccess.standard(for: self) }
#endif
        return try await client(for: planeRegistryID)
    }

    /// The provider models hold; it keeps the session weakly.
    var libraryAccessProvider: JetLibraryAccessProvider {
        { [weak self] planeRegistryID in
            guard let self else { throw CancellationError() }
            return try await self.libraryAccess(for: planeRegistryID)
        }
    }

    /// Fixture sessions have no computer to ask, so the library shows empty states.
    var canLoadLibrary: Bool { usesLivePlane || isPreviewSession }

    /// "Now" for dates in the library: fixed in previews so screenshots stay stable.
    var libraryNow: Date {
#if DEBUG
        if isPreviewSession { return PreviewLibraryAccess.now }
#endif
        return .now
    }

    /// "Now" for the project page's task times, matching the preview task list.
    var taskListNow: Date {
#if DEBUG
        if isPreviewSession {
            return Date(timeIntervalSince1970: TimeInterval(DesktopPreviewData.now) / 1_000)
        }
#endif
        return .now
    }

    // MARK: - Computers and projects

    /// A computer's name, or "This Mac" before the computer is known.
    func planeDisplayName(_ planeRegistryID: UUID) -> String {
        planes.first { $0.id == planeRegistryID }?.name ?? String(localized: "This Mac")
    }

    /// A project's tasks on one computer, newest first.
    func tasks(inProject projectID: UUID, on planeRegistryID: UUID) -> [JetConversationSummary] {
        conversations
            .filter {
                $0.projectID == projectID
                    && (conversationPlaneRegistryIDs[$0.id] ?? localPlaneRegistryID) == planeRegistryID
            }
            .sorted { $0.createdAtUnixMilliseconds > $1.createdAtUnixMilliseconds }
    }

    /// Show in Finder applies to folders on this Mac.
    func canRevealInFinder(on planeRegistryID: UUID) -> Bool {
#if os(macOS)
        return isLocalPlane(planeRegistryID)
#else
        return false
#endif
    }

    func revealInFinder(path: String) {
#if os(macOS)
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path, isDirectory: true)])
#endif
    }

    // MARK: - Project removal

    /// Opens the project and its reviewed removal sheet.
    func presentProjectRemoval(_ projectID: UUID, on planeRegistryID: UUID) async {
        selectProject(projectID, on: planeRegistryID)
        await prepareProjectRemoval(projectID)
    }

    /// Confirms the reviewed removal. On success the project is gone, so New Task
    /// opens and says what happened to the folder.
    func removeProjectFolder(typedName: String, permanently: Bool) async {
        guard let preview = removalPreview else { return }
        await confirmProjectRemoval(typedName: typedName, permanently: permanently)
        guard removalPreview == nil else { return }
        setupNotice = nil
        beginNewTask()
        composerNotice = ComposerNotice(
            kind: .confirmation,
            text: permanently
                ? String(localized: "Deleted the “\(preview.name)” folder.")
                : String(localized: "Moved the “\(preview.name)” folder to the macOS Trash.")
        )
    }

    // MARK: - Jet Trash

    /// After a task moved to Jet Trash: select the next task if it was open, offer
    /// Undo, close the sheet and refresh the list.
    func finishMoveToTrash(_ ref: ConversationRef, title: String) {
        trashedConversationIDs.insert(ref.conversationID)
        if selectedConversationID == ref.conversationID {
            // The row below in sidebar order, where Needs You comes first.
            let sections = sidebarSections
            let next = Self.taskToSelect(
                afterRemoving: ref.conversationID,
                orderedIDs: (sections.needsYou + sections.tasks).map(\.id)
            )
            if let next {
                selectConversation(next)
            } else {
                beginNewTask()
            }
        }
        composerNotice = ComposerNotice(
            kind: .confirmation,
            text: String(localized: "Moved “\(title)” to Jet Trash."),
            action: .undoMoveToTrash(ref)
        )
        dismissSheet()
        Task { await loadConversations() }
    }

    /// The task to open after the open task leaves the list: the row below, else
    /// the row above, else nil for New Task.
    static func taskToSelect(afterRemoving removedID: UUID, orderedIDs: [UUID]) -> UUID? {
        guard let index = orderedIDs.firstIndex(of: removedID) else { return nil }
        if index + 1 < orderedIDs.count { return orderedIDs[index + 1] }
        if index > 0 { return orderedIDs[index - 1] }
        return nil
    }

    /// A task came back from Jet Trash: it may be opened again and appears in the list.
    func libraryDidRestore(_ ref: ConversationRef) async {
        trashedConversationIDs.remove(ref.conversationID)
        conversationPlaneRegistryIDs[ref.conversationID] = ref.planeRegistryID
        await loadConversations()
    }

    /// Show Waiting Messages: the task opens with Details › Activity.
    func showWaitingMessages(_ ref: ConversationRef) {
        dismissSheet()
        conversationPlaneRegistryIDs[ref.conversationID] = ref.planeRegistryID
        guardUnsavedEdits { [weak self] in
            self?.selectConversation(ref.conversationID)
            self?.showDetails(.run)
        }
    }

    /// The composer's feedback line, shown on the project page and Jet Trash, which
    /// have no composer: a Move to Jet Trash confirmation with Undo, or what Undo
    /// reported. A notice left over from a task is cleared by `clearStaleLibraryNotice()`.
    var libraryPageNotice: ComposerNotice? {
        switch sidebarSelection {
        case .project, .trash: composerNotice
        default: nil
        }
    }

    /// Clears a notice that belonged to the task the person just left.
    func clearStaleLibraryNotice() {
        guard let composerNotice else { return }
        if case .undoMoveToTrash = composerNotice.action { return }
        self.composerNotice = nil
    }
}
