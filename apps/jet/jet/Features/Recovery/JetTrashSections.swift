import Foundation
import Observation
import SwiftUI

// MARK: - Issues

/// One line of feedback in the library: what happened and at most one next step.
/// The stable error code shows only in the tooltip.
struct LibraryIssue: Equatable, Sendable {
    enum Action: Equatable, Sendable {
        case tryAgain
        case checkAgain
        case sendSameRequestAgain
        case showJetTrash
        case showWaitingMessages
        case review(JetSettingsPane)
        case openTask(UUID)

        var title: String {
            switch self {
            case .tryAgain: String(localized: "Try Again")
            case .checkAgain: String(localized: "Check Again")
            case .sendSameRequestAgain: String(localized: "Send Same Request Again")
            case .showJetTrash: String(localized: "Show Jet Trash")
            case .showWaitingMessages: String(localized: "Show Waiting Messages")
            case .review: String(localized: "Review…")
            case .openTask: String(localized: "Open Task")
            }
        }
    }

    var kind: ComposerNotice.Kind
    var text: String
    var action: Action?
    var code: String?

    init(kind: ComposerNotice.Kind, text: String, action: Action? = nil, code: String? = nil) {
        self.kind = kind
        self.text = text
        self.action = action
        self.code = code
    }

    /// Changes are paused while the computer protects its data (read-only recovery).
    static func paused() -> LibraryIssue {
        let recovery = JetPresentationError(
            category: .unavailable,
            code: "recovery.read_only",
            message: "",
            retryable: false
        )
        return LibraryIssue(
            kind: .warning,
            text: String(localized: "Jet paused changes to protect your data."),
            action: JetSettingsPane.resolving(recovery).map(Action.review),
            code: recovery.code
        )
    }

    /// Maps a failed Query or Command to casual copy. `computer` names the computer
    /// the request went to; `isLocal` picks "Your Mac" for disk space.
    static func from(_ error: Error, computer: String, isLocal: Bool) -> LibraryIssue {
        let failure = DesktopSession.presentationError(error)
        let prefix = failure.code.split(separator: ".").first.map(String.init)
        if failure.category == .offline {
            return LibraryIssue(
                kind: .warning,
                text: String(localized: "Can't reach \(computer). Try again when it's connected."),
                action: .tryAgain,
                code: failure.code
            )
        }
        if prefix == "recovery" {
            return LibraryIssue(
                kind: .warning,
                text: String(localized: "Jet paused changes to protect your data."),
                action: JetSettingsPane.resolving(failure).map(Action.review),
                code: failure.code
            )
        }
        if failure.code == "storage.disk_pressure" {
            return LibraryIssue(
                kind: .warning,
                text: isLocal
                    ? String(localized: "Your Mac is almost out of disk space. Jet paused new work.")
                    : String(localized: "\(computer) is almost out of disk space. Jet paused new work."),
                code: failure.code
            )
        }
        if failure.code == "protocol.feature_unavailable" {
            // Older computers lack the trash, schedule or settings Queries.
            return LibraryIssue(
                kind: .warning,
                text: String(localized: "Update Jet on \(computer) to use this."),
                code: failure.code
            )
        }
        if failure.category == .invalidInput {
            return LibraryIssue(kind: .error, text: failure.message, code: failure.code)
        }
        return LibraryIssue(
            kind: .error,
            text: String(localized: "Jet couldn't finish this. Try again."),
            action: .tryAgain,
            code: failure.code
        )
    }

    static func isOutcomeUnknown(_ error: Error) -> Bool {
        if case JetClientFailure.commandOutcomeUnknown = error { return true }
        return false
    }

    static func code(of error: Error) -> String {
        DesktopSession.presentationError(error).code
    }
}

/// A library issue in the inline notice style, with one small bordered button.
struct LibraryNoticeRow: View {
    let issue: LibraryIssue
    let perform: @MainActor (LibraryIssue.Action) -> Void

    @ScaledMetric(relativeTo: .callout) private var textSize = JetDesign.TextSize.control

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Image(systemName: InlineNotice.systemImage(for: issue.kind))
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(InlineNotice.tint(for: issue.kind))
                Text(issue.text)
                    .foregroundStyle(issue.kind == .warning || issue.kind == .error ? .primary : .secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Text(accessibilityText))
            .help(issue.code ?? "")

            Spacer(minLength: 8)

            if let action = issue.action {
                Button(action.title) { perform(action) }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
        }
        .font(.system(size: textSize))
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var accessibilityText: String {
        switch issue.kind {
        case .info, .confirmation: issue.text
        case .warning: String(localized: "Warning: \(issue.text)")
        case .error: String(localized: "Error: \(issue.text)")
        }
    }
}

// MARK: - Copy

/// Library copy shared by the pages, sheets and tests.
enum LibraryCopy {
    /// "Sep 20", with the year only when it differs from now's.
    static func shortDate(
        _ date: Date,
        now: Date,
        locale: Locale = .autoupdatingCurrent,
        timeZone: TimeZone = .autoupdatingCurrent
    ) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.locale = locale
        calendar.timeZone = timeZone
        var style = Date.FormatStyle(locale: locale, calendar: calendar, timeZone: timeZone)
            .month(.abbreviated).day()
        if calendar.component(.year, from: date) != calendar.component(.year, from: now) {
            style = style.year()
        }
        return date.formatted(style)
    }

    static var untitledTask: String { String(localized: "Untitled task") }

    /// Casual library strings, checked against `JetCopy.avoidWords`. Review copy
    /// (Move to Jet Trash with Stop Run, Delete Everywhere, project removal) is exempt.
    static var casual: [String] {
        [
            // Project page
            String(localized: "Tasks in This Project"),
            String(localized: "New Task in “web-app”"),
            String(localized: "Show All 12 Tasks"),
            String(localized: "No tasks in web-app yet."),
            String(localized: "When a Reply Finishes"),
            String(localized: "Replies that stop early or with an error aren't saved automatically."),
            String(localized: "Branches and Names"),
            String(localized: "Branch names start with"),
            String(localized: "Branch names can't contain spaces or ~ ^ : ? * [ \\."),
            String(localized: "Name tasks automatically"),
            String(localized: "Jet suggests a short title for each new task."),
            String(localized: "Reset to Default (jet/)"),
            String(localized: "Reset to Default (On)"),
            String(localized: "Location"),
            String(localized: "Show in Finder"),
            String(localized: "Copy Path"),
            String(localized: "No Project Selected"),
            String(localized: "Choose a project in the sidebar."),
            String(localized: "No Projects"),
            String(localized: "Add a Git repository to start working on it."),
            String(localized: "Can't reach This Mac. Project settings can't be changed right now."),
            String(localized: "Saving…"),
            String(localized: "Loading…"),
            // When a Reply Finishes
            ReplyFinishPreset.doNothing.title,
            ReplyFinishPreset.saveToBranch.title,
            ReplyFinishPreset.saveAndPush.title,
            ReplyFinishPreset.saveAndOpenPullRequest.title,
            String(localized: "Custom"),
            String(localized: "Changes stay in the task's working copy until you choose Keep Changes…."),
            String(localized: "After each reply with changes, Jet commits them to a new jet/ branch in web-app. Your checked-out files don't change."),
            String(localized: "Also pushes the branch to origin."),
            String(localized: "Also opens or updates a draft pull request on GitHub. Needs a GitHub token saved for Jet in your Keychain."),
            String(localized: "This project uses its own mix of Git steps. Choose an option to replace it."),
            String(localized: "This task uses its own mix of Git steps. Choose an option to replace it."),
            String(localized: "Couldn't finish changing this setting. Some steps were saved."),
            String(localized: "This setting changed in another Jet app. Jet refreshed it."),
            String(localized: "Jet couldn't confirm this change."),
            String(localized: "Push automatically after every reply?"),
            String(localized: "Jet pushes each reply's changes to origin without asking."),
            String(localized: "Jet pushes each reply's changes to origin and opens or updates a draft pull request on GitHub without asking."),
            String(localized: "Push Automatically"),
            // Task Settings
            String(localized: "Task Settings"),
            String(localized: "Use Project Default (Save to a Branch)"),
            String(localized: "Follows web-app's setting."),
            String(localized: "Applies to replies that finish from now on."),
            String(localized: "Can't reach This Mac. Settings can't be changed right now."),
            // Jet Trash
            String(localized: "Jet Trash Is Empty"),
            String(localized: "Tasks you move to Jet Trash stay here until they're removed for good."),
            String(localized: "Showing Jet Trash from when This Mac was last connected."),
            String(localized: "Can't Show Jet Trash"),
            String(localized: "Connect to This Mac to see removed tasks."),
            String(localized: "Tasks are removed for good after 30 days."),
            String(localized: "Change in Settings…"),
            String(localized: "Moved to another computer"),
            String(localized: "Deleted everywhere"),
            String(localized: "Cleaned up automatically"),
            String(localized: "Can't be restored here"),
            String(localized: "Restored “Fix login redirect”."),
            String(localized: "Jet couldn't confirm that “Fix login redirect” was restored."),
            String(localized: "Removed On"),
            String(localized: "Soon"),
            untitledTask,
            // Move to Jet Trash (the plain review is casual; Stop Run reviews are exempt)
            String(localized: "Move “Fix login redirect” to Jet Trash?"),
            String(localized: "You can restore it from Jet Trash until Oct 28. After that its working copy and Jet history are deleted. Claude Code's own history isn't deleted."),
            String(localized: "Messages are still waiting to send."),
            String(localized: "Remove them or wait until they're sent, then move the task."),
            String(localized: "“Fix login redirect” is already in Jet Trash. You can restore it until Oct 28."),
            String(localized: "Its working copy has changes that aren't saved to a branch."),
            String(localized: "It has commits that weren't pushed."),
            String(localized: "A Git step hasn't finished or couldn't be confirmed."),
            String(localized: "It still repeats a message daily. The message keeps arriving until the task is removed for good."),
            String(localized: "Stop Repeating…"),
            String(localized: "Claude Code is still starting. Try again in a moment."),
            String(localized: "Stopping Claude Code…"),
            String(localized: "Claude Code stopped. Check the details, then move the task."),
            String(localized: "Claude Code hasn't stopped yet. Jet is still stopping it."),
            String(localized: "Jet couldn't confirm that the task moved to Jet Trash."),
            String(localized: "Moved “Fix login redirect” to Jet Trash."),
            // Repeat Daily
            String(localized: "Repeat Daily"),
            String(localized: "Jet sends this message to Claude Code in “Fix login redirect” every day."),
            String(localized: "Every day at 9:00 AM (Berlin)"),
            String(localized: "Next: Sep 29 at 9:00 AM"),
            String(localized: "Stop repeating this message?"),
            String(localized: "It won't be sent again. If it's waiting to send, it's removed. A reply in progress continues."),
            String(localized: "New Daily Message"),
            String(localized: "What should Claude Code do each day?"),
            String(localized: "The first message waits until this task has started."),
            String(localized: "Repeats daily at 9:00 AM (Berlin)."),
            String(localized: "Jet couldn't confirm the new daily message."),
            String(localized: "Jet couldn't confirm that the message stopped repeating."),
            // Issues
            String(localized: "Can't reach This Mac. Try again when it's connected."),
            String(localized: "Jet paused changes to protect your data."),
            String(localized: "Your Mac is almost out of disk space. Jet paused new work."),
            String(localized: "Jet couldn't finish this. Try again."),
            String(localized: "Update Jet on This Mac to use this."),
        ]
    }
}

// MARK: - Jet Trash rows

/// One row of the Jet Trash table.
struct JetTrashRow: Identifiable, Equatable {
    let entry: JetTrashEntry
    let title: String
    /// Why the task is here, when it wasn't moved by hand.
    let note: String?
    let projectName: String?

    var id: UUID { entry.conversationID }
    var canRestore: Bool { entry.canRestore }
    var deletedAt: Date { entry.trashedAt }
    var removedOn: Date { entry.expiresAt }
    var projectSortKey: String { projectName ?? "" }

    /// Rows newest deleted first. Titles come from `conversation(id)`, then the
    /// loaded list, then the remembered Jet Trash titles, then "Untitled task".
    static func rows(
        entries: [JetTrashEntry],
        fetched: [UUID: JetConversationSummary],
        listed: [JetConversationSummary],
        remembered: (UUID) -> String?,
        projects: [UUID: String]
    ) -> [JetTrashRow] {
        let listedByID = Dictionary(listed.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        return entries
            .map { entry in
                let id = entry.conversationID
                let summary = fetched[id] ?? listedByID[id]
                let candidates: [String?] = [fetched[id]?.title, listedByID[id]?.title, remembered(id)]
                let title = candidates
                    .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
                    .first { !$0.isEmpty } ?? LibraryCopy.untitledTask
                return JetTrashRow(
                    entry: entry,
                    title: title,
                    note: note(for: entry.reason),
                    projectName: summary?.projectID.flatMap { projects[$0] }
                )
            }
            .sorted { $0.deletedAt > $1.deletedAt }
    }

    static func note(for reason: String) -> String? {
        if reason == "plane_transfer" { return String(localized: "Moved to another computer") }
        if reason.hasSuffix("_everywhere") { return String(localized: "Deleted everywhere") }
        if reason == "autodelete_rule" || reason == "automatic_forget" {
            return String(localized: "Cleaned up automatically")
        }
        return nil
    }
}

// MARK: - Jet Trash model

/// Jet Trash on one computer: its entries, titles, grace period and Restore.
@MainActor
@Observable
final class JetTrashModel {
    enum Phase: Equatable {
        case loading
        case loaded
        /// Offline or failed with nothing to show.
        case unavailable
    }

    static let graceDaysKey = SettingKey(rawValue: "retention.trash_grace_days")!

    @ObservationIgnored private let makeAccess: JetLibraryAccessProvider
    @ObservationIgnored private let memory: ClientMemory
    @ObservationIgnored let now: () -> Date
    @ObservationIgnored private var generation = 0
    @ObservationIgnored private var isSeeded = false

    private(set) var planeRegistryID: UUID?
    private(set) var phase: Phase = .loading
    private(set) var entries: [JetTrashEntry] = []
    private(set) var fetched: [UUID: JetConversationSummary] = [:]
    private(set) var graceDays: UInt32?
    private(set) var isReadOnly = false
    /// Entries kept from an earlier load while the computer can't be reached.
    private(set) var isShowingCache = false
    private(set) var restoring: Set<UUID> = []
    var issue: LibraryIssue?
    /// "Restored “X”." with Open Task.
    var notice: LibraryIssue?
    /// A restore keeps its Command ID until its outcome is known.
    private(set) var restoreCommandIDs: [UUID: UUID] = [:]
    private(set) var uncertainRestore: UUID?

    init(
        makeAccess: @escaping JetLibraryAccessProvider,
        memory: ClientMemory,
        now: @escaping () -> Date = { .now }
    ) {
        self.makeAccess = makeAccess
        self.memory = memory
        self.now = now
    }

    /// Restore is offered only with a live, writable Jet Trash.
    var canChange: Bool { !isReadOnly && !isShowingCache && phase == .loaded }

    func load(planeRegistryID: UUID, computer: String, isLocal: Bool) async {
        guard !isSeeded else { return }
        generation += 1
        let current = generation
        if self.planeRegistryID != planeRegistryID {
            self.planeRegistryID = planeRegistryID
            entries = []
            fetched = [:]
            graceDays = nil
            isReadOnly = false
            isShowingCache = false
            phase = .loading
            notice = nil
            issue = nil
            restoring = []
        }
        let hadCache = phase == .loaded
        do {
            let access = try await makeAccess(planeRegistryID)
            let trash = try await access.conversationTrash()
            guard current == generation else { return }
            entries = trash.entries
            isShowingCache = false
            phase = .loaded
            if uncertainRestore == nil { issue = nil }
            if case let .count(days)? = try? await access.settings(scope: .plane).value(for: Self.graceDaysKey),
               current == generation
            {
                graceDays = days
            }
            if let health = try? await access.systemHealth(), current == generation {
                isReadOnly = health.recoveryState == "read_only"
            }
            await fetchTitles(access, generation: current)
        } catch {
            guard current == generation, !(error is CancellationError) else { return }
            let mapped = LibraryIssue.from(error, computer: computer, isLocal: isLocal)
            if hadCache, DesktopSession.presentationError(error).category == .offline {
                isShowingCache = true
                issue = nil
            } else if hadCache {
                issue = mapped
            } else {
                phase = .unavailable
                issue = mapped
            }
        }
    }

    /// Titles from `conversation(id)`, at most four at a time. A newer load stops them.
    private func fetchTitles(_ access: any JetLibraryAccess, generation current: Int) async {
        let ids = entries.map(\.conversationID).filter { fetched[$0] == nil }
        var start = 0
        while start < ids.count {
            let batch = Array(ids[start ..< min(start + 4, ids.count)])
            start += 4
            let results = await withTaskGroup(of: (UUID, JetConversationSnapshot?).self) { group in
                for id in batch {
                    group.addTask {
                        (id, try? await access.conversation(id))
                    }
                }
                var values: [(UUID, JetConversationSnapshot?)] = []
                for await value in group { values.append(value) }
                return values
            }
            guard current == generation, !Task.isCancelled else { return }
            for (id, snapshot) in results {
                if let snapshot { fetched[id] = snapshot.conversation }
            }
        }
    }

    func rows(listed: [JetConversationSummary], projects: [UUID: String]) -> [JetTrashRow] {
        let memory = memory
        return JetTrashRow.rows(
            entries: entries,
            fetched: fetched,
            listed: listed,
            remembered: { memory.trashedTitle($0) },
            projects: projects
        )
    }

    /// Restores one task with a single Command. Returns true when the task is back.
    @discardableResult
    func restore(_ row: JetTrashRow, computer: String, isLocal: Bool) async -> Bool {
        guard let planeRegistryID, row.canRestore, !restoring.contains(row.id) else { return false }
        // ASVS 2.3.1: no mutation is offered while the computer protects its data.
        guard !isReadOnly else {
            issue = .paused()
            return false
        }
        let id = row.id
        let commandID = restoreCommandIDs[id] ?? UUID()
        restoreCommandIDs[id] = commandID
        restoring.insert(id)
        defer { restoring.remove(id) }
        notice = nil
        issue = nil
        do {
            let access = try await makeAccess(planeRegistryID)
            do {
                try await access.restoreConversation(id, commandID: commandID)
            } catch let error where LibraryIssue.code(of: error) == "retention.not_trashed" {
                // Already out of Jet Trash: the task is back either way.
            } catch let error where LibraryIssue.isOutcomeUnknown(error) {
                // One confirming Query: the entry is gone only if the restore happened.
                let trash = try? await access.conversationTrash()
                guard let trash, !trash.entries.contains(where: { $0.conversationID == id }) else {
                    uncertainRestore = id
                    issue = LibraryIssue(
                        kind: .warning,
                        text: String(localized: "Jet couldn't confirm that “\(row.title)” was restored."),
                        action: .sendSameRequestAgain,
                        code: LibraryIssue.code(of: error)
                    )
                    return false
                }
            }
            restoreCommandIDs.removeValue(forKey: id)
            if uncertainRestore == id { uncertainRestore = nil }
            memory.forgetTrashedTitle(id)
            entries.removeAll { $0.conversationID == id }
            notice = LibraryIssue(
                kind: .confirmation,
                text: String(localized: "Restored “\(row.title)”."),
                action: .openTask(id)
            )
            await reload(computer: computer, isLocal: isLocal)
            return true
        } catch {
            // A definite refusal gets a fresh Command ID next time.
            restoreCommandIDs.removeValue(forKey: id)
            if uncertainRestore == id { uncertainRestore = nil }
            if LibraryIssue.code(of: error).hasPrefix("recovery.") { isReadOnly = true }
            issue = LibraryIssue.from(error, computer: computer, isLocal: isLocal)
            return false
        }
    }

    /// Send Same Request Again: the uncertain restore with its original Command ID.
    @discardableResult
    func sendSameRequestAgain(rows: [JetTrashRow], computer: String, isLocal: Bool) async -> Bool {
        guard let id = uncertainRestore, let row = rows.first(where: { $0.id == id }) else { return false }
        return await restore(row, computer: computer, isLocal: isLocal)
    }

    func reload(computer: String, isLocal: Bool) async {
        guard let planeRegistryID else { return }
        let notice = notice
        await load(planeRegistryID: planeRegistryID, computer: computer, isLocal: isLocal)
        self.notice = notice
    }

#if DEBUG
    /// A fixed state for previews and screenshots; `load` then does nothing.
    func seedForPreview(
        planeRegistryID: UUID,
        phase: Phase,
        entries: [JetTrashEntry] = [],
        fetched: [UUID: JetConversationSummary] = [:],
        graceDays: UInt32? = 30,
        isShowingCache: Bool = false,
        issue: LibraryIssue? = nil
    ) {
        isSeeded = true
        self.planeRegistryID = planeRegistryID
        self.phase = phase
        self.entries = entries
        self.fetched = fetched
        self.graceDays = graceDays
        self.isShowingCache = isShowingCache
        self.issue = issue
    }
#endif
}

// MARK: - Legacy Settings section

// delete in wave 3: the pre-WP11 Settings pane still embeds this list. Jet Trash
// now has its own page (JetTrashPage) and Move to Jet Trash its own sheet.
struct JetTrashSection: View {
    @Bindable var model: JetRecoveryModel
    let planeName: String
    let selectedTitle: String?
    let conversationTitles: [UUID: String]

    // ASVS 2.3.1: do not offer a mutation while the Plane is in Recovery mode.
    private var isReadOnly: Bool { model.health?.recoveryState == "read_only" }

    var body: some View {
        Section("Jet Trash") {
            if model.isLoading && model.trash == nil {
                Text("Loading…").foregroundStyle(.secondary)
            }
            if let entries = model.trash?.entries {
                if entries.isEmpty {
                    Text("Jet Trash Is Empty").foregroundStyle(.secondary)
                }
                ForEach(entries) { entry in
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(conversationTitles[entry.conversationID] ?? LibraryCopy.untitledTask)
                            Text("Removed on \(entry.expiresAt.formatted(date: .abbreviated, time: .omitted))")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        if entry.canRestore {
                            Button("Restore") { Task { await model.restore(entry.conversationID) } }
                                .disabled(isReadOnly || model.operation != nil || model.issues[.trash] != nil)
                                .accessibilityIdentifier("trash-restore-\(entry.id.uuidString)")
                        } else {
                            Text("Can't be restored here").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            }
            RecoveryIssue(error: model.issues[.trash])
        }
    }
}
