import Foundation
import SwiftUI
import Testing
@testable import jet

@MainActor
struct TaskStatusPresentationTests {
    /// Every status the design names, including each phase of Working.
    nonisolated static let shown: [TaskStatus] = [
        .offline,
        .starting,
        .working(nil),
        .working(.editingFiles),
        .working(.runningCommand),
        .working(.readingProject),
        .working(.searchingWeb),
        .working(.working),
        .reconnecting,
        .stopping,
        .needsPermission,
        .needsSignIn,
        .usageLimit,
        .gitUnconfirmed,
        .failed,
        .waitingForReply,
        .finished,
        .stopped,
        .notStarted,
    ]

    /// Fails to compile when TaskStatus gains a case, so `shown` stays complete.
    private static func isListed(_ status: TaskStatus) -> Bool {
        switch status {
        case .unknown: false
        case .offline, .starting, .working, .reconnecting, .stopping, .needsPermission,
             .needsSignIn, .usageLimit, .gitUnconfirmed, .failed, .waitingForReply,
             .finished, .stopped, .notStarted: true
        }
    }

    @Test(arguments: shown)
    func everyShownStatusHasTextAndAVisual(_ status: TaskStatus) {
        #expect(Self.isListed(status))
        #expect(!status.title.isEmpty)
        #expect(!status.accessibilityDescription.isEmpty)
        // Status is always a symbol or spinner plus text (design §10).
        #expect(status.showsSpinner != (status.systemImage != nil))
    }

    @Test
    func anUnknownStatusShowsNothing() {
        #expect(TaskStatus.unknown.title.isEmpty)
        #expect(TaskStatus.unknown.accessibilityDescription.isEmpty)
        #expect(TaskStatus.unknown.systemImage == nil)
        #expect(!TaskStatus.unknown.showsSpinner)
        #expect(TaskStatus.unknown.rowGlyph(isUnread: false) == .none)
    }

    @Test
    func theDesignCopyIsExact() {
        #expect(TaskStatus.offline.title == "Offline · showing saved view")
        #expect(TaskStatus.starting.title == "Starting…")
        #expect(TaskStatus.working(.editingFiles).title == "Working · Editing files")
        #expect(TaskStatus.working(.runningCommand).title == "Working · Running a command")
        #expect(TaskStatus.working(.readingProject).title == "Working · Reading the project")
        #expect(TaskStatus.working(.searchingWeb).title == "Working · Searching the web")
        #expect(TaskStatus.working(.working).title == "Working")
        #expect(TaskStatus.working(nil).title == "Working")
        #expect(TaskStatus.reconnecting.title == "Reconnecting…")
        #expect(TaskStatus.stopping.title == "Stopping…")
        #expect(TaskStatus.needsPermission.title == "Needs permission")
        #expect(TaskStatus.needsSignIn.title == "Sign-in needed")
        #expect(TaskStatus.usageLimit.title == "Usage limit reached")
        #expect(TaskStatus.gitUnconfirmed.title == "Couldn't confirm a Git step")
        #expect(TaskStatus.failed.title == "Stopped with an error")
        #expect(TaskStatus.waitingForReply.title == "Waiting for your reply")
        #expect(TaskStatus.finished.title == "Finished · send a message to continue")
        #expect(TaskStatus.stopped.title == "Stopped · send a message to continue")
        #expect(TaskStatus.needsPermission.accessibilityDescription == "Needs permission")
        #expect(TaskStatus.working(.editingFiles).accessibilityDescription == "Working, editing files")
    }

    @Test
    func theDesignSymbolsAreExact() {
        #expect(TaskStatus.offline.systemImage == "wifi.slash")
        #expect(TaskStatus.needsPermission.systemImage == "hand.raised.fill")
        #expect(TaskStatus.needsSignIn.systemImage == "person.crop.circle.badge.exclamationmark")
        #expect(TaskStatus.usageLimit.systemImage == "hourglass")
        #expect(TaskStatus.gitUnconfirmed.systemImage == "questionmark.circle")
        #expect(TaskStatus.failed.systemImage == "xmark.octagon.fill")
        #expect(TaskStatus.waitingForReply.systemImage == "bubble.left")
        #expect(TaskStatus.finished.systemImage == "checkmark.circle")
        #expect(TaskStatus.stopped.systemImage == "stop.circle")
        // Stop Assistant never uses stop.fill; that symbol belongs to Interrupt.
        #expect(!Self.shown.contains { $0.systemImage == "stop.fill" })
    }

    @Test(arguments: shown)
    func needsYouStatusesAreOrange(_ status: TaskStatus) {
        if status.needsYou {
            #expect(status.tint == .orange)
        } else {
            #expect(status.tint != .orange)
        }
    }

    @Test
    func failuresAreRedButAStopIsNeutral() {
        #expect(TaskStatus.failed.tint == .red)
        #expect(TaskStatus.stopped.tint == .secondary)
    }

    @Test
    func lostAndCanceledShareTheStoppedTitle() {
        let lost = TaskStatus.derive(
            facts: TaskStatusFacts(lifecycle: .lost, hasRuns: true),
            isOffline: false,
            phase: nil
        )
        let canceled = TaskStatus.derive(
            facts: TaskStatusFacts(lifecycle: .canceled, hasRuns: true),
            isOffline: false,
            phase: nil
        )
        #expect(lost.title == canceled.title)
        #expect(lost.title == "Stopped · send a message to continue")
        #expect(lost.tint == .secondary)
    }

    @Test(arguments: shown)
    func noTitleUsesAJetDomainWord(_ status: TaskStatus) {
        #expect(JetCopy.foundAvoidWords(in: status.title).isEmpty)
        #expect(JetCopy.foundAvoidWords(in: status.accessibilityDescription).isEmpty)
    }

    @Test
    func theAvoidWordCheckMatchesWholeWordsOnly() {
        #expect(JetCopy.foundAvoidWords(in: "Running a command") == [])
        #expect(JetCopy.foundAvoidWords(in: "Interrupt this Turn?") == ["Turn"])
        #expect(JetCopy.foundAvoidWords(in: "Turn On") == ["Turn"])
        #expect(JetCopy.foundAvoidWords(in: "the plane's cursor") == ["cursor"])
    }

    // MARK: - Row glyphs (design §8)

    @Test
    func rowGlyphsFollowTheTable() {
        #expect(TaskStatus.working(.editingFiles).rowGlyph(isUnread: true) == .spinner)
        #expect(TaskStatus.starting.rowGlyph(isUnread: false) == .spinner)
        #expect(TaskStatus.needsPermission.rowGlyph(isUnread: true) == .symbol("hand.raised.fill"))
        #expect(TaskStatus.failed.rowGlyph(isUnread: false) == .symbol("xmark.octagon.fill"))
        #expect(TaskStatus.waitingForReply.rowGlyph(isUnread: true) == .unreadDot)
        #expect(TaskStatus.waitingForReply.rowGlyph(isUnread: false) == .none)
        #expect(TaskStatus.finished.rowGlyph(isUnread: true) == .unreadDot)
        #expect(TaskStatus.stopped.rowGlyph(isUnread: false) == .none)
    }

    // MARK: - Inline notice

    @Test
    func inlineNoticeKindsMapToTheirSymbolAndColour() {
        #expect(InlineNotice.systemImage(for: .info) == "info.circle")
        #expect(InlineNotice.systemImage(for: .confirmation) == "checkmark.circle")
        #expect(InlineNotice.systemImage(for: .warning) == "exclamationmark.triangle.fill")
        #expect(InlineNotice.systemImage(for: .error) == "xmark.octagon.fill")
        #expect(InlineNotice.tint(for: .info) == .secondary)
        #expect(InlineNotice.tint(for: .confirmation) == .secondary)
        #expect(InlineNotice.tint(for: .warning) == .orange)
        #expect(InlineNotice.tint(for: .error) == .red)
    }

    // MARK: - Copy helpers

    @Test
    func ordinalsUseTheLocale() {
        let english = Locale(identifier: "en_US")
        #expect(JetCopy.ordinal(1, locale: english) == "1st")
        #expect(JetCopy.ordinal(2, locale: english) == "2nd")
        #expect(JetCopy.ordinal(23, locale: english) == "23rd")
    }

    @Test
    func relativeTimesStayShortAndNeverPointAhead() {
        let english = Locale(identifier: "en_US")
        let now = Date(timeIntervalSince1970: 1_790_000_000)
        let ms = { (seconds: TimeInterval) in Int64((now.timeIntervalSince1970 - seconds) * 1_000) }
        #expect(JetCopy.relative(ms: ms(20), now: now, locale: english) == "now")
        #expect(JetCopy.relative(ms: ms(-600), now: now, locale: english) == "now")
        #expect(JetCopy.relative(ms: ms(2 * 3_600), now: now, locale: english).contains("2"))
        #expect(JetCopy.relative(ms: ms(2 * 3_600), now: now, locale: english).contains("ago"))
        let old = JetCopy.relative(ms: ms(20 * 86_400), now: now, locale: english)
        #expect(!old.contains("ago"))
        #expect(!old.isEmpty)
    }

    @Test
    func transcriptScaleStepsRunFrom85To200Percent() {
        #expect(JetDesign.transcriptScaleKey == "jet.transcript.text-scale")
        #expect(JetDesign.transcriptScaleSteps.first == 0.85)
        #expect(JetDesign.transcriptScaleSteps.last == 2.0)
        #expect(JetDesign.transcriptScaleSteps.contains(1.0))
        #expect(JetDesign.transcriptScaleSteps == JetDesign.transcriptScaleSteps.sorted())
    }
}

/// The shell router's dialog actions must not race with dialog dismissal.
@MainActor
struct ShellPresentationsTests {
    /// A seeded session whose computer never answers, so a Command stays in flight
    /// until its task is cancelled.
    private func stalledSession() -> DesktopSession {
        let suite = "jet.tests.shell-presentations"
        let defaults = UserDefaults(suiteName: suite) ?? .standard
        defaults.removePersistentDomain(forName: suite)
        let session = DesktopSession(
            makeJetClient: {
                try await Task.sleep(for: .seconds(600))
                throw CancellationError()
            },
            notificationPreference: { _ in false },
            memory: ClientMemory(defaults: defaults),
            isPreviewSession: true
        )
        DesktopPreviewData.working(session)
        return session
    }

    @Test
    func confirmingInterruptStartsBeforeTheDialogDismisses() async {
        let session = stalledSession()
        session.runControlConfirmation = .interruptTurn
        #expect(ShellPresentations.isRunControlPresented(session))

        let task = ShellPresentations.confirmRunControl(session)
        // The Command is already in flight when the button's action returns, so a
        // dismissal that follows can't cancel it.
        #expect(session.supervisionOperation == JetRunControl.interruptTurn.rawValue)
        #expect(session.runControlConfirmation == .interruptTurn)
        #expect(!ShellPresentations.isRunControlPresented(session))

        task.cancel()
        await task.value
        // The request failed: the notice explains it and the dialog doesn't come back.
        #expect(session.supervisionOperation == nil)
        #expect(session.runControlConfirmation == nil)
        #expect(!ShellPresentations.isRunControlPresented(session))
        #expect(session.composerNotice != nil)
    }

    @Test
    func aFailedStopAssistantDoesNotReopenTheDialog() async {
        // The preview session fails without suspending.
        let session = DesktopSession.preview { DesktopPreviewData.working($0) }
        session.runControlConfirmation = .stopRun
        await ShellPresentations.confirmRunControl(session).value
        #expect(session.supervisionOperation == nil)
        #expect(session.runControlConfirmation == nil)
        #expect(!ShellPresentations.isRunControlPresented(session))
        #expect(session.composerNotice != nil)
    }

    @Test
    func resolvingAHeldNavigationClearsItAtOnce() async {
        let session = DesktopSession.preview { DesktopPreviewData.connect($0) }
        var performed = 0
        session.pendingNavigation = PendingNavigation(fileName: "login.ts") { performed += 1 }
        let discard = ShellPresentations.resolvePendingNavigation(session, save: false)
        #expect(session.pendingNavigation == nil)
        await discard.value
        #expect(performed == 1)

        session.pendingNavigation = PendingNavigation(fileName: "login.ts") { performed += 1 }
        let cancel = ShellPresentations.resolvePendingNavigation(session, save: nil)
        #expect(session.pendingNavigation == nil)
        await cancel.value
        #expect(performed == 1)
    }
}
