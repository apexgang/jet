import Foundation
import Observation
import SwiftUI

// MARK: - Model

/// Move to Jet Trash… and Delete Everywhere… for one task (design §6.11). Each
/// step is one typed Command with a retained ID; nothing is resent automatically.
@MainActor
@Observable
final class MoveToTrashModel {
    enum Mode: Hashable, Sendable {
        case forget
        case deleteEverywhere

        var action: JetRetentionAction {
            switch self {
            case .forget: .forget
            case .deleteEverywhere: .deleteEverywhere
            }
        }
    }

    enum Phase: Equatable, Sendable {
        case loading
        case ready
        /// Stop Run was sent; Jet waits for the assistant to end.
        case stopping
        case moving
        case alreadyInTrash(until: Date)
        case unavailable
    }

    /// Why Move to Jet Trash can't go ahead yet, in the order they are checked.
    enum Blocker: Equatable, Sendable {
        case activeRun
        case pendingTurn
        case readOnly
    }

    /// A Command whose outcome is unknown, kept for Send Same Request Again.
    enum Uncertain: Equatable, Sendable {
        case stage(Mode)
        case stop(runID: UUID)
    }

    struct ProtectionLine: Equatable, Sendable {
        let code: String
        let text: String
        /// "Stop Repeating…" goes with the daily message line.
        let offersStopRepeating: Bool
    }

    static let pollLimit = 30
    static let liveProtections: Set<String> = ["active_run", "pending_turn"]

    let ref: ConversationRef
    private(set) var title: String
    let assistantName: String?
    private(set) var mode: Mode
    let computer: String
    let isLocal: Bool
    @ObservationIgnored private let makeAccess: JetLibraryAccessProvider
    @ObservationIgnored private let memory: ClientMemory
    @ObservationIgnored let now: () -> Date
    @ObservationIgnored private let sleep: @Sendable (Duration) async throws -> Void
    @ObservationIgnored private var isSeeded = false
    @ObservationIgnored private var flow: Task<Void, Never>?

    private(set) var phase: Phase = .loading
    private(set) var preview: JetRetentionPreview?
    private(set) var runs: [JetRunSummary] = []
    private(set) var isReadOnly = false
    private(set) var graceDays: UInt32?
    var issue: LibraryIssue?
    private(set) var uncertain: Uncertain?
    private(set) var stageCommandIDs: [Mode: UUID] = [:]
    private(set) var stopCommandIDs: [UUID: UUID] = [:]
    /// The protections the person saw when they chose Stop and Move.
    private(set) var reviewedProtections: Set<String> = []

    init(
        ref: ConversationRef,
        title: String,
        assistantName: String?,
        mode: Mode,
        makeAccess: @escaping JetLibraryAccessProvider,
        memory: ClientMemory,
        now: @escaping () -> Date = { .now },
        sleep: @escaping @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) },
        computer: String = String(localized: "This Mac"),
        isLocal: Bool = true
    ) {
        self.ref = ref
        self.title = title
        self.assistantName = assistantName
        self.mode = mode
        self.makeAccess = makeAccess
        self.memory = memory
        self.now = now
        self.sleep = sleep
        self.computer = computer
        self.isLocal = isLocal
    }

    // MARK: Reading

    var protections: [String] { preview?.protections ?? [] }

    var blocker: Blocker? {
        switch mode {
        case .forget:
            if protections.contains("active_run") { return .activeRun }
            if protections.contains("pending_turn") { return .pendingTurn }
            if isReadOnly { return .readOnly }
            return nil
        case .deleteEverywhere:
            return isReadOnly ? .readOnly : nil
        }
    }

    /// The blocker the sheet shows: while stopping, still the open assistant.
    var displayedBlocker: Blocker? {
        phase == .stopping ? .activeRun : blocker
    }

    /// When the task would be removed for good, from the computer's grace period.
    var restoreDate: Date? {
        graceDays.map { now().addingTimeInterval(TimeInterval($0) * 86_400) }
    }

    var protectionLines: [ProtectionLine] { Self.protectionLines(protections).lines }
    var detailLines: [String] {
        var lines = Self.protectionLines(protections).details
        if let records = preview?.auditRecords, records > 0 {
            lines.append(records == 1
                ? String(localized: "1 security record mentions this task.")
                : String(localized: "\(records) security records mention this task."))
        }
        return lines
    }

    /// Protections as sentences; unknown codes go under Details. Live work is a
    /// blocker, not a protection line.
    static func protectionLines(_ codes: [String]) -> (lines: [ProtectionLine], details: [String]) {
        var lines: [ProtectionLine] = []
        var details: [String] = []
        for code in codes where !liveProtections.contains(code) {
            switch code {
            case "dirty_workspace":
                lines.append(ProtectionLine(code: code, text: String(localized: "Its working copy has changes that aren't saved to a branch."), offersStopRepeating: false))
            case "unpushed_work":
                lines.append(ProtectionLine(code: code, text: String(localized: "It has commits that weren't pushed."), offersStopRepeating: false))
            case "unresolved_effect":
                lines.append(ProtectionLine(code: code, text: String(localized: "A Git step hasn't finished or couldn't be confirmed."), offersStopRepeating: false))
            case "enabled_schedule":
                // Forgetting doesn't cancel schedules, so the message keeps arriving.
                lines.append(ProtectionLine(code: code, text: String(localized: "It still repeats a message daily. The message keeps arriving until the task is removed for good."), offersStopRepeating: true))
            default:
                details.append(code)
            }
        }
        return (lines, details)
    }

    // MARK: Copy

    /// "Oct 28", or nil when the grace period is unknown.
    static func dateText(
        _ date: Date?,
        now: Date,
        locale: Locale = .autoupdatingCurrent,
        timeZone: TimeZone = .autoupdatingCurrent
    ) -> String? {
        date.map { LibraryCopy.shortDate($0, now: now, locale: locale, timeZone: timeZone) }
    }

    var restoreDateText: String? { Self.dateText(restoreDate, now: now()) }

    /// The assistant mid-sentence ("stops Claude Code") and at the start of one.
    private var assistant: String { assistantName ?? String(localized: "the assistant") }
    private var assistantAtStart: String { assistantName ?? String(localized: "The assistant") }

    var headline: String {
        switch phase {
        case .alreadyInTrash:
            return String(localized: "Already in Jet Trash")
        case .loading, .unavailable:
            return mode == .forget
                ? String(localized: "Move “\(title)” to Jet Trash?")
                : String(localized: "Delete “\(title)” Everywhere?")
        case .ready, .stopping, .moving:
            break
        }
        if mode == .deleteEverywhere { return String(localized: "Delete “\(title)” Everywhere?") }
        switch displayedBlocker {
        case .activeRun: return String(localized: "\(assistantAtStart) is still open for this task.")
        case .pendingTurn: return String(localized: "Messages are still waiting to send.")
        case .readOnly, nil: return String(localized: "Move “\(title)” to Jet Trash?")
        }
    }

    var message: String? {
        switch phase {
        case .loading, .unavailable:
            return nil
        case let .alreadyInTrash(until):
            let date = LibraryCopy.shortDate(until, now: now())
            return String(localized: "“\(title)” is already in Jet Trash. You can restore it until \(date).")
        case .ready, .stopping, .moving:
            break
        }
        let date = restoreDateText
        if mode == .deleteEverywhere {
            return Self.deleteEverywhereMessage(assistant: assistant, date: date)
        }
        switch displayedBlocker {
        case .activeRun:
            return Self.activeRunMessage(assistant: assistant, date: date)
        case .pendingTurn:
            return String(localized: "Remove them or wait until they're sent, then move the task.")
        case .readOnly, nil:
            return Self.forgetMessage(owner: assistantAtStart, date: date)
        }
    }

    static func forgetMessage(owner: String, date: String?) -> String {
        if let date {
            return String(localized: "You can restore it from Jet Trash until \(date). After that its working copy and Jet history are deleted. \(owner)'s own history isn't deleted.")
        }
        return String(localized: "You can restore it from Jet Trash until it's removed for good. After that its working copy and Jet history are deleted. \(owner)'s own history isn't deleted.")
    }

    static func activeRunMessage(assistant: String, date: String?) -> String {
        if let date {
            return String(localized: "To move it to Jet Trash, Jet first stops \(assistant), including commands it started (Stop Run). You can restore the task until \(date). After that its working copy and Jet history are deleted.")
        }
        return String(localized: "To move it to Jet Trash, Jet first stops \(assistant), including commands it started (Stop Run). You can restore the task until it's removed for good. After that its working copy and Jet history are deleted.")
    }

    static func deleteEverywhereMessage(assistant: String, date: String?) -> String {
        if let date {
            return String(localized: "Jet stops \(assistant) for this task (Stop Run), cancels messages waiting to send, and moves it to Jet Trash. You can restore it until \(date). After that its working copy and Jet history are deleted. This version of Jet can't delete \(assistant)'s own history, so it stays.")
        }
        return String(localized: "Jet stops \(assistant) for this task (Stop Run), cancels messages waiting to send, and moves it to Jet Trash. You can restore it until it's removed for good. After that its working copy and Jet history are deleted. This version of Jet can't delete \(assistant)'s own history, so it stays.")
    }

    var stillStartingText: String { String(localized: "\(assistantAtStart) is still starting. Try again in a moment.") }
    var stoppingText: String { String(localized: "Stopping \(assistant)…") }

    // MARK: Loading

    func load() async {
        guard !isSeeded else { return }
        phase = .loading
        issue = nil
        do {
            let access = try await makeAccess(ref.planeRegistryID)
            let preview = try await access.retentionPreview(conversationID: ref.conversationID)
            if let snapshot = try? await access.conversation(ref.conversationID) {
                runs = snapshot.runs
                let loaded = snapshot.conversation.title.trimmingCharacters(in: .whitespacesAndNewlines)
                if !loaded.isEmpty { title = loaded }
            }
            if let health = try? await access.systemHealth() {
                isReadOnly = health.recoveryState == "read_only"
            }
            if case let .count(days)? = try? await access.settings(scope: .plane).value(for: JetTrashModel.graceDaysKey) {
                graceDays = days
            }
            self.preview = preview
            phase = preview.trash.map { .alreadyInTrash(until: $0.expiresAt) } ?? .ready
        } catch {
            guard !(error is CancellationError) else { return }
            issue = LibraryIssue.from(error, computer: computer, isLocal: isLocal)
            phase = .unavailable
        }
    }

    /// Delete Everywhere…: the same sheet reviews deleting everywhere instead.
    func reviewDeleteEverywhere() {
        mode = .deleteEverywhere
        issue = nil
    }

    /// Check Again: reads the preview and the task's state once more.
    func checkAgain() async {
        issue = nil
        await load()
    }

    // MARK: Flows

    /// Runs a flow tied to the sheet: Cancel stops waiting, never a sent Command.
    func run(
        _ operation: @escaping @MainActor (MoveToTrashModel) async -> Bool,
        onMoved: @escaping @MainActor () -> Void
    ) {
        flow?.cancel()
        flow = Task { [weak self] in
            guard let self else { return }
            if await operation(self) { onMoved() }
        }
    }

    func cancel() {
        flow?.cancel()
        flow = nil
    }

    /// Move to Jet Trash or Delete Everywhere. True once the task is in Jet Trash.
    func moveToTrash() async -> Bool {
        guard phase == .ready, blocker == nil else { return false }
        return await stage(mode)
    }

    /// Stop and Move to Jet Trash: Stop Run for each live Run, wait up to 30 s for
    /// it to end, check again, then move the task.
    func stopAndMove() async -> Bool {
        guard mode == .forget, phase == .ready, blocker == .activeRun, !isReadOnly else { return false }
        reviewedProtections = Set(protections).subtracting(Self.liveProtections)
        phase = .stopping
        issue = nil
        let access: any JetLibraryAccess
        do {
            access = try await makeAccess(ref.planeRegistryID)
            if let snapshot = try? await access.conversation(ref.conversationID) { runs = snapshot.runs }
        } catch {
            return stopFailed(error)
        }
        for run in runs where run.lifecycle.isLive && run.lifecycle != .stopping {
            let commandID = stopCommandIDs[run.id] ?? UUID()
            stopCommandIDs[run.id] = commandID
            do {
                try await access.stopRun(runID: run.id, commandID: commandID)
                stopCommandIDs.removeValue(forKey: run.id)
                if uncertain == .stop(runID: run.id) { uncertain = nil }
            } catch let error where LibraryIssue.isOutcomeUnknown(error) {
                // One confirming Query: the Run is stopping or has ended.
                let lifecycle = (try? await access.conversation(ref.conversationID))?
                    .runs.first { $0.id == run.id }?.lifecycle
                if let lifecycle, lifecycle == .stopping || !lifecycle.isLive {
                    stopCommandIDs.removeValue(forKey: run.id)
                    if uncertain == .stop(runID: run.id) { uncertain = nil }
                    continue
                }
                uncertain = .stop(runID: run.id)
                phase = .ready
                issue = LibraryIssue(
                    kind: .warning,
                    text: String(localized: "Jet couldn't confirm that \(assistant) was asked to stop."),
                    action: .sendSameRequestAgain,
                    code: LibraryIssue.code(of: error)
                )
                return false
            } catch {
                stopCommandIDs.removeValue(forKey: run.id)
                if uncertain == .stop(runID: run.id) { uncertain = nil }
                return stopFailed(error)
            }
        }
        return await waitThenMove(access)
    }

    private func stopFailed(_ error: Error) -> Bool {
        phase = .ready
        if LibraryIssue.code(of: error) == "run.not_started" {
            issue = LibraryIssue(kind: .info, text: stillStartingText, code: "run.not_started")
        } else {
            if LibraryIssue.code(of: error).hasPrefix("recovery.") { isReadOnly = true }
            issue = LibraryIssue.from(error, computer: computer, isLocal: isLocal)
        }
        return false
    }

    private func waitThenMove(_ access: any JetLibraryAccess) async -> Bool {
        for _ in 0 ..< Self.pollLimit {
            do {
                try await sleep(.seconds(1))
            } catch {
                phase = .ready
                return false
            }
            guard let latest = try? await access.retentionPreview(conversationID: ref.conversationID) else {
                continue
            }
            preview = latest
            if let entry = latest.trash {
                phase = .alreadyInTrash(until: entry.expiresAt)
                return false
            }
            guard !latest.protections.contains("active_run") else { continue }
            phase = .ready
            // Anything new since the person chose, including waiting messages, needs another look.
            let current = Set(latest.protections).subtracting(Self.liveProtections)
            if blocker != nil || !current.isSubset(of: reviewedProtections) {
                issue = LibraryIssue(
                    kind: .info,
                    text: String(localized: "\(assistantAtStart) stopped. Check the details, then move the task.")
                )
                return false
            }
            return await stage(.forget)
        }
        phase = .ready
        issue = LibraryIssue(
            kind: .warning,
            text: String(localized: "\(assistantAtStart) hasn't stopped yet. Jet is still stopping it."),
            action: .checkAgain
        )
        return false
    }

    /// Send Same Request Again: the uncertain Command with its original ID and body.
    func sendSameRequestAgain() async -> Bool {
        switch uncertain {
        case let .stage(mode):
            phase = .ready
            return await stage(mode)
        case .stop:
            return await stopAndMove()
        case nil:
            return false
        }
    }

    /// Stages the task. The title is remembered first, because Jet Trash entries have none.
    private func stage(_ mode: Mode) async -> Bool {
        let id = ref.conversationID
        let commandID = stageCommandIDs[mode] ?? UUID()
        stageCommandIDs[mode] = commandID
        phase = .moving
        issue = nil
        memory.recordTrashedTitle(title, for: id)
        do {
            let access = try await makeAccess(ref.planeRegistryID)
            do {
                _ = try await access.stageConversation(id, action: mode.action, commandID: commandID)
            } catch let error where LibraryIssue.isOutcomeUnknown(error) {
                // One confirming Query: the preview names the Jet Trash entry only if it moved.
                let confirmed = try? await access.retentionPreview(conversationID: id)
                guard confirmed?.trash != nil else {
                    uncertain = .stage(mode)
                    phase = .ready
                    issue = LibraryIssue(
                        kind: .warning,
                        text: String(localized: "Jet couldn't confirm that the task moved to Jet Trash."),
                        action: .sendSameRequestAgain,
                        code: LibraryIssue.code(of: error)
                    )
                    return false
                }
            }
            stageCommandIDs.removeValue(forKey: mode)
            uncertain = nil
            return true
        } catch {
            // A definite refusal: nothing moved, and the next try is a new Command.
            stageCommandIDs.removeValue(forKey: mode)
            uncertain = nil
            let code = LibraryIssue.code(of: error)
            if code == "retention.already_trashed" {
                await load()
                return false
            }
            memory.forgetTrashedTitle(id)
            phase = .ready
            switch code {
            case "retention.run_starting":
                issue = LibraryIssue(kind: .info, text: stillStartingText, code: code)
            case "retention.live_work":
                // Something started since the review; the preview shows what.
                if let latest = try? await makeAccess(ref.planeRegistryID).retentionPreview(conversationID: id) {
                    preview = latest
                }
            default:
                if code.hasPrefix("recovery.") { isReadOnly = true }
                issue = LibraryIssue.from(error, computer: computer, isLocal: isLocal)
            }
            return false
        }
    }

#if DEBUG
    /// A fixed state for previews and screenshots; `load` then does nothing.
    func seedForPreview(
        preview: JetRetentionPreview?,
        runs: [JetRunSummary] = [],
        isReadOnly: Bool = false,
        graceDays: UInt32? = 30,
        phase: Phase = .ready,
        issue: LibraryIssue? = nil
    ) {
        isSeeded = true
        self.preview = preview
        self.runs = runs
        self.isReadOnly = isReadOnly
        self.graceDays = graceDays
        self.phase = phase
        self.issue = issue
    }
#endif
}

// MARK: - Sheet

/// Move to Jet Trash… (design §6.11). Cancel is the default button.
struct MoveToTrashSheet: View {
    let session: DesktopSession
    let ref: ConversationRef
    let deleteEverywhere: Bool
    @State private var model: MoveToTrashModel

    init(
        session: DesktopSession,
        ref: ConversationRef,
        deleteEverywhere: Bool,
        model: MoveToTrashModel? = nil
    ) {
        self.session = session
        self.ref = ref
        self.deleteEverywhere = deleteEverywhere
        let title = session.conversations.first { $0.id == ref.conversationID }?.title
            .trimmingCharacters(in: .whitespacesAndNewlines)
        _model = State(initialValue: model ?? MoveToTrashModel(
            ref: ref,
            title: title.flatMap { $0.isEmpty ? nil : $0 } ?? LibraryCopy.untitledTask,
            assistantName: session.assistantName(for: ref.conversationID),
            mode: deleteEverywhere ? .deleteEverywhere : .forget,
            makeAccess: session.libraryAccessProvider,
            memory: session.memory,
            now: { [weak session] in session?.libraryNow ?? .now },
            computer: session.planeDisplayName(ref.planeRegistryID),
            isLocal: session.isLocalPlane(ref.planeRegistryID)
        ))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: JetDesign.gap) {
            VStack(alignment: .leading, spacing: 6) {
                Text(model.headline)
                    .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
                    .fixedSize(horizontal: false, vertical: true)
                if let message = model.message {
                    Text(message)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            status

            if showsProtections {
                protections
            }

            buttons
        }
        .padding(JetDesign.sectionGap)
        .frame(width: 480, alignment: .leading)
        .task { await model.load() }
        .onDisappear { model.cancel() }
        .interactiveDismissDisabled(model.phase == .moving)
#if os(macOS)
        .onExitCommand(perform: cancel)
#endif
    }

    private var showsProtections: Bool {
        switch model.phase {
        case .ready, .stopping, .moving: model.blocker != .pendingTurn
        case .loading, .alreadyInTrash, .unavailable: false
        }
    }

    // MARK: Status

    @ViewBuilder
    private var status: some View {
        switch model.phase {
        case .loading:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Loading…").foregroundStyle(.secondary)
            }
        case .stopping:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text(model.stoppingText)
            }
            .accessibilityElement(children: .combine)
        default:
            EmptyView()
        }
        if let issue = model.issue {
            LibraryNoticeRow(issue: issue, perform: perform)
        } else if model.phase == .ready, model.isReadOnly {
            // ASVS 2.3.1: no retention Command is offered while the computer is read-only.
            LibraryNoticeRow(issue: .paused(), perform: perform)
        }
    }

    @ViewBuilder
    private var protections: some View {
        let lines = model.protectionLines
        let details = model.detailLines
        if !lines.isEmpty {
            VStack(alignment: .leading, spacing: JetDesign.smallGap) {
                ForEach(lines, id: \.code) { line in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .symbolRenderingMode(.monochrome)
                            .foregroundStyle(.orange)
                            .accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 4) {
                            Text(line.text)
                                .fixedSize(horizontal: false, vertical: true)
                            if line.offersStopRepeating {
                                Button("Stop Repeating…") { session.presentRepeatDaily(ref) }
                                    .buttonStyle(.borderless)
                                    .foregroundStyle(JetDesign.accentText)
                            }
                        }
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel(Text("Warning: \(line.text)"))
                }
            }
        }
        if !details.isEmpty {
            DisclosureGroup("Details") {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(details, id: \.self) { line in
                        Text(line)
                            .font(.callout.monospaced())
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 4)
            }
        }
    }

    // MARK: Buttons

    private var buttons: some View {
        HStack(spacing: JetDesign.smallGap) {
            if model.mode == .forget, model.phase == .ready {
                Button("Delete Everywhere…") { model.reviewDeleteEverywhere() }
                    .buttonStyle(.borderless)
                    .foregroundStyle(.red)
                    .accessibilityIdentifier("retention-delete-everywhere-review")
            }
            Spacer()
            Button("Cancel", action: cancel)
                .keyboardShortcut(.defaultAction)
                .disabled(model.phase == .moving)
            primary
        }
    }

    @ViewBuilder
    private var primary: some View {
        let isWorking = model.phase == .stopping || model.phase == .moving
        switch model.phase {
        case .alreadyInTrash:
            Button("Show Jet Trash") {
                session.dismissSheet()
                session.showTrash()
            }
        case .unavailable, .loading:
            EmptyView()
        case .ready, .stopping, .moving:
            if model.mode == .deleteEverywhere {
                Button("Delete Everywhere", role: .destructive) {
                    model.run({ await $0.moveToTrash() }, onMoved: finish)
                }
                .disabled(isWorking || model.blocker != nil)
                .accessibilityIdentifier("retention-delete-everywhere")
            } else {
                switch model.displayedBlocker {
                case .activeRun:
                    Button("Stop and Move to Jet Trash", role: .destructive) {
                        model.run({ await $0.stopAndMove() }, onMoved: finish)
                    }
                    .disabled(isWorking || model.isReadOnly)
                    .accessibilityIdentifier("retention-forget")
                case .pendingTurn:
                    Button("Show Waiting Messages") { session.showWaitingMessages(ref) }
                case .readOnly, nil:
                    Button("Move to Jet Trash") {
                        model.run({ await $0.moveToTrash() }, onMoved: finish)
                    }
                    .disabled(isWorking || model.blocker != nil)
                    .accessibilityIdentifier("retention-forget")
                }
            }
        }
    }

    // MARK: Actions

    private func cancel() {
        guard model.phase != .moving else { return }
        model.cancel()
        session.dismissSheet()
    }

    private func finish() {
        session.finishMoveToTrash(ref, title: model.title)
    }

    private func perform(_ action: LibraryIssue.Action) {
        switch action {
        case .tryAgain, .checkAgain:
            Task { await model.checkAgain() }
        case .sendSameRequestAgain:
            model.run({ await $0.sendSameRequestAgain() }, onMoved: finish)
        case .showJetTrash:
            session.dismissSheet()
            session.showTrash()
        case .showWaitingMessages:
            session.showWaitingMessages(ref)
        case let .review(pane):
            session.perform(.openSettings(pane))
        case .openTask:
            break
        }
    }
}

#if DEBUG
#Preview("Move to Jet Trash") {
    DesktopPreviewScenes.view("library-move-to-trash")
        .frame(width: 480, height: 440)
}
#endif
