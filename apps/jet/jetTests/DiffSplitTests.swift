import Foundation
import Testing
@testable import jet

@MainActor
struct DiffSplitTests {
    // MARK: - DiffSplit

    static let modeChange = """
    diff --git a/scripts/deploy.sh b/scripts/deploy.sh
    old mode 100644
    new mode 100755

    """

    static let threeFilePatch = """
    diff --git a/src/auth/guard.ts b/src/auth/guard.ts
    new file mode 100644
    index 0000000..8b7d9e0
    --- /dev/null
    +++ b/src/auth/guard.ts
    @@ -0,0 +1,3 @@
    +export function requireSession() {
    +  return null
    +}
    diff --git a/src/auth/legacy-redirect.ts b/src/auth/legacy-redirect.ts
    deleted file mode 100644
    index 5e4d3c2..0000000
    --- a/src/auth/legacy-redirect.ts
    +++ /dev/null
    @@ -1,2 +0,0 @@
    -export const legacy = "/login"
    -export default legacy
    diff --git a/src/auth/login.ts b/src/auth/login.ts
    index 91c0d2e..4aa81f3 100644
    --- a/src/auth/login.ts
    +++ b/src/auth/login.ts
    @@ -10,4 +10,5 @@ export async function signIn(form: FormData) {
       const user = await verify(form)
    -  await createSession(user)
    +  const session = await createSession(user)
    +  return session
       return redirect("/")
    \\ No newline at end of file

    """

    @Test
    func theSeedPatchSplitsIntoThreeCompleteFilesTotallingPlus42Minus7() {
        let result = DiffSplit.split(DetailsSeed.threeFilePatch, patchIsComplete: true)
        #expect(result.order == ["src/auth/guard.ts", "src/auth/legacy-redirect.ts", "src/auth/login.ts"])
        #expect(result.additions == 42)
        #expect(result.deletions == 7)
        #expect(result.isComplete)
        #expect(result.order.allSatisfy { result.files[$0]?.isComplete == true })
        #expect(result.files["src/auth/guard.ts"]?.additions == 28)
        #expect(result.files["src/auth/legacy-redirect.ts"]?.deletions == 4)
        #expect(result.files["src/auth/login.ts"]?.hunks.count == 2)
        #expect(result.files["src/auth/login.ts"]?.additions == 14)
        #expect(result.files["src/auth/login.ts"]?.deletions == 3)
    }

    @Test
    func addedAndDeletedFilesTakeTheirPathFromTheRealSide() {
        let result = DiffSplit.split(Self.threeFilePatch, patchIsComplete: true)
        #expect(result.order == ["src/auth/guard.ts", "src/auth/legacy-redirect.ts", "src/auth/login.ts"])
        #expect(result.files["src/auth/guard.ts"]?.additions == 3)
        #expect(result.files["src/auth/legacy-redirect.ts"]?.deletions == 2)
        #expect(result.additions == 5)
        #expect(result.deletions == 3)
    }

    @Test
    func aHeaderOnlyModeChangeUsesTheSymmetricHeaderPath() throws {
        let result = DiffSplit.split(Self.modeChange, patchIsComplete: true)
        #expect(result.order == ["scripts/deploy.sh"])
        let file = try #require(result.files["scripts/deploy.sh"])
        #expect(!file.hasTextChanges)
        #expect(file.hunks.isEmpty)
        #expect(!file.isBinary)
    }

    @Test
    func quotedPathsAreUnescaped() {
        let patch = """
        diff --git "a/docs/caf\\303\\251.txt" "b/docs/caf\\303\\251.txt"
        index 1111111..2222222 100644
        --- "a/docs/caf\\303\\251.txt"
        +++ "b/docs/caf\\303\\251.txt"
        @@ -1 +1 @@
        -old
        +new
        diff --git "a/notes \\"draft\\".md" "b/notes \\"draft\\".md"
        old mode 100644
        new mode 100755

        """
        let result = DiffSplit.split(patch, patchIsComplete: true)
        #expect(result.order == ["docs/café.txt", "notes \"draft\".md"])
        #expect(DiffSplit.unquote("\"a\\tb\\\\c\\nd\"") == "a\tb\\c\nd")
        #expect(DiffSplit.unquote("plain.txt") == "plain.txt")
    }

    @Test
    func binarySectionsAreMarkedAndLeftOutOfTheTotals() throws {
        let patch = """
        diff --git a/logo.png b/logo.png
        new file mode 100644
        index 0000000..3c9d0e1
        GIT binary patch
        literal 12
        zcmV-u1Ww&~P)<h;3K|Lk000e1

        literal 0
        HcmV?d00001

        diff --git a/icon.gif b/icon.gif
        index 1111111..2222222 100644
        Binary files a/icon.gif and b/icon.gif differ
        diff --git a/readme.md b/readme.md
        index 1111111..2222222 100644
        --- a/readme.md
        +++ b/readme.md
        @@ -1 +1,2 @@
         Jet
        +Now with Details.

        """
        let result = DiffSplit.split(patch, patchIsComplete: true)
        #expect(result.order == ["logo.png", "icon.gif", "readme.md"])
        #expect(try #require(result.files["logo.png"]).isBinary)
        #expect(try #require(result.files["icon.gif"]).isBinary)
        #expect(try #require(result.files["logo.png"]).hunks.isEmpty)
        #expect(result.additions == 1)
        #expect(result.deletions == 0)
    }

    @Test
    func aTruncatedPatchMarksOnlyTheLastSectionIncomplete() {
        let cut = String(Self.threeFilePatch.prefix(while: { $0 != "\\" }))
        let result = DiffSplit.split(cut, patchIsComplete: false)
        #expect(!result.isComplete)
        #expect(result.files["src/auth/guard.ts"]?.isComplete == true)
        #expect(result.files["src/auth/legacy-redirect.ts"]?.isComplete == true)
        #expect(result.files["src/auth/login.ts"]?.isComplete == false)
    }

    @Test
    func headerLikeLinesInsideAHunkAreContent() throws {
        let patch = """
        diff --git a/notes.md b/notes.md
        index 1111111..2222222 100644
        --- a/notes.md
        +++ b/notes.md
        @@ -1,2 +1,2 @@
        ---y
        +++x
         context

        """
        let file = try #require(DiffSplit.split(patch, patchIsComplete: true).files["notes.md"])
        #expect(file.additions == 1)
        #expect(file.deletions == 1)
        #expect(file.hunks[0].lines.map(\.kind) == [.removed, .added, .context])
        #expect(file.hunks[0].lines.map(\.text) == ["--y", "++x", "context"])
    }

    struct HeaderCase: Sendable {
        let line: String
        let expected: DiffHunkHeader
        var testDescription: String { line }
    }

    @Test(arguments: [
        HeaderCase(line: "-10,6 +10,9 @@ func", expected: DiffHunkHeader(oldStart: 10, oldCount: 6, newStart: 10, newCount: 9, heading: "func")),
        HeaderCase(line: "-1 +1", expected: DiffHunkHeader(oldStart: 1, oldCount: 1, newStart: 1, newCount: 1, heading: nil)),
        HeaderCase(line: "-0,0 +1,3", expected: DiffHunkHeader(oldStart: 0, oldCount: 0, newStart: 1, newCount: 3, heading: nil)),
        HeaderCase(line: "@@ -21,4 +22,14 @@ export async function signIn()", expected: DiffHunkHeader(oldStart: 21, oldCount: 4, newStart: 22, newCount: 14, heading: "export async function signIn()")),
    ])
    func hunkHeadersParse(_ testCase: HeaderCase) {
        #expect(DiffSplit.parseHunkHeader(testCase.line) == testCase.expected)
    }

    @Test
    func malformedHunkHeadersAreRejected() {
        #expect(DiffSplit.parseHunkHeader("@@ nonsense @@") == nil)
        #expect(DiffSplit.parseHunkHeader("-a +1") == nil)
    }

    @Test
    func linesAreNumberedPerSideAndTheNoNewlineMarkerHasNoNumbers() throws {
        let file = try #require(DiffSplit.split(Self.threeFilePatch, patchIsComplete: true).files["src/auth/login.ts"])
        let lines = file.hunks[0].lines
        #expect(lines == [
            DiffLine(kind: .context, text: "  const user = await verify(form)", oldNumber: 10, newNumber: 10),
            DiffLine(kind: .removed, text: "  await createSession(user)", oldNumber: 11, newNumber: nil),
            DiffLine(kind: .added, text: "  const session = await createSession(user)", oldNumber: nil, newNumber: 11),
            DiffLine(kind: .added, text: "  return session", oldNumber: nil, newNumber: 12),
            DiffLine(kind: .context, text: "  return redirect(\"/\")", oldNumber: 12, newNumber: 13),
            DiffLine(kind: .noNewlineMarker, text: "No newline at end of file", oldNumber: nil, newNumber: nil),
        ])
    }

    @Test
    func anEmptyPatchHasNoFiles() {
        let result = DiffSplit.split("", patchIsComplete: true)
        #expect(result == DiffSplitResult(order: [], files: [:], isComplete: true, additions: 0, deletions: 0))
    }

    @Test
    func availabilityCoversEveryCase() throws {
        let stored = JetChangeArtifact(sha256: "ab", size: 10, availability: .stored)
        let full = JetChangeArtifact(sha256: "ab", size: 10, availability: .diskPressure)
        let complete = DiffSplit.split(Self.threeFilePatch + Self.modeChange, patchIsComplete: true)
        let loginFile = DetailsSeed.file("src/auth/login.ts", .modified)
        let login = try #require(complete.files["src/auth/login.ts"])

        // Text changes.
        #expect(DiffSplit.availability(for: loginFile, in: complete, artifact: stored) == .lines(login))
        // Binary.
        let binary = DiffSplit.split("diff --git a/a.png b/a.png\nBinary files a/a.png and b/a.png differ\n", patchIsComplete: true)
        #expect(DiffSplit.availability(for: DetailsSeed.file("a.png", .modified), in: binary, artifact: stored) == .binary)
        // Cut off inside the file, with more stored.
        let cut = DiffSplit.split(String(Self.threeFilePatch.prefix(while: { $0 != "\\" })), patchIsComplete: false)
        let partial = try #require(cut.files["src/auth/login.ts"])
        #expect(DiffSplit.availability(for: loginFile, in: cut, artifact: stored)
            == .cutOff(partial: partial, canLoadMore: true, reason: nil))
        // Cut off before the file, and nothing more was saved.
        #expect(DiffSplit.availability(for: DetailsSeed.file("z.ts", .added), in: cut, artifact: full)
            == .cutOff(partial: nil, canLoadMore: false, reason: "The rest wasn't saved because the disk was almost full."))
        // Too large: listed without content, and no section in a complete patch.
        #expect(DiffSplit.availability(for: DetailsSeed.file("big.json", .modified, contentAvailable: false), in: complete, artifact: stored)
            == .tooLarge)
        // No text changes: a mode change.
        #expect(DiffSplit.availability(for: DetailsSeed.file("scripts/deploy.sh", .modified), in: complete, artifact: stored)
            == .noTextChanges)
    }

    @Test
    func cutOffReasonsNameWhyTheRestIsMissing() {
        #expect(DiffSplit.cutOffReason(.stored) == nil)
        #expect(DiffSplit.cutOffReason(.runBudgetExceeded) == "The rest wasn't saved because this task reached its storage limit.")
        #expect(DiffSplit.cutOffReason(.artifactSizeExceeded) == "The full changes are too large to save.")
    }

    @Test
    func hunkTitlesDescribeTheNewSide() {
        func hunk(_ newStart: Int, _ newCount: Int, heading: String? = nil) -> DiffHunk {
            DiffHunk(oldStart: 10, oldCount: 3, newStart: newStart, newCount: newCount, heading: heading, lines: [])
        }
        #expect(hunk(10, 9).title == "Lines 10–18")
        #expect(hunk(10, 1).title == "Line 10")
        #expect(hunk(9, 0).title == "Removed at line 10")
        #expect(hunk(10, 9, heading: "func signIn()").title == "Lines 10–18 · func signIn()")
    }

    // MARK: - Scope choices

    struct ScopeCase: Sendable {
        let kind: WorkCheckpointKind
        let turn: UInt32
        let fromTurn: UInt32
        let toTurn: UInt32
        let choice: ChangesScopeChoice
        let segment: ChangesSegment?
        var testDescription: String { "\(kind) \(turn) \(fromTurn)–\(toTurn)" }
    }

    @Test(arguments: [
        ScopeCase(kind: .current, turn: 1, fromTurn: 0, toTurn: 1, choice: .all, segment: .all),
        ScopeCase(kind: .turn, turn: 3, fromTurn: 0, toTurn: 1, choice: .lastReply, segment: .lastReply),
        ScopeCase(kind: .turn, turn: 2, fromTurn: 0, toTurn: 1, choice: .reply(2), segment: nil),
        ScopeCase(kind: .final, turn: 1, fromTurn: 0, toTurn: 1, choice: .whenStopped, segment: nil),
        ScopeCase(kind: .historical, turn: 1, fromTurn: 1, toTurn: 3, choice: .range(fromReply: 2, toReply: 3), segment: nil),
    ])
    func scopeChoicesRoundTripThroughTheCheckpointFields(_ testCase: ScopeCase) throws {
        let choice = ChangesScopeChoice.from(
            kind: testCase.kind,
            turn: testCase.turn,
            fromTurn: testCase.fromTurn,
            toTurn: testCase.toTurn,
            latestTurn: 3
        )
        #expect(choice == testCase.choice)
        #expect(choice.segment == testCase.segment)
        let checkpoint = try #require(choice.checkpoint(latestTurn: 3, runIsLive: false))
        #expect(checkpoint.kind == testCase.kind)
        #expect(ChangesScopeChoice.from(
            kind: checkpoint.kind,
            turn: checkpoint.turn,
            fromTurn: checkpoint.fromTurn,
            toTurn: checkpoint.toTurn,
            latestTurn: 3
        ) == choice)
    }

    @Test
    func scopeTitlesUseTheDesignWords() {
        #expect(ChangesScopeChoice.all.title(assistant: "Claude Code") == "All Changes")
        #expect(ChangesScopeChoice.lastReply.title(assistant: nil) == "Last Reply")
        #expect(ChangesScopeChoice.reply(2).title(assistant: nil) == "Reply 2")
        #expect(ChangesScopeChoice.range(fromReply: 2, toReply: 3).title(assistant: nil) == "Replies 2–3")
        #expect(ChangesScopeChoice.range(fromReply: 3, toReply: 3).title(assistant: nil) == "Reply 3")
        #expect(ChangesScopeChoice.whenStopped.title(assistant: "Claude Code") == "When Claude Code Stopped")
        #expect(ChangesScopeChoice.whenStopped.title(assistant: nil) == "When the Assistant Stopped")
    }

    @Test
    func checkpointsAreOnlyValidForTheTasksReplies() {
        #expect(ChangesScopeChoice.all.checkpoint(latestTurn: 0, runIsLive: true) == DetailsCheckpoint(kind: .current))
        #expect(ChangesScopeChoice.lastReply.checkpoint(latestTurn: 0, runIsLive: false) == nil)
        #expect(ChangesScopeChoice.lastReply.checkpoint(latestTurn: 4, runIsLive: true) == DetailsCheckpoint(kind: .turn, turn: 4))
        #expect(ChangesScopeChoice.reply(0).checkpoint(latestTurn: 4, runIsLive: false) == nil)
        #expect(ChangesScopeChoice.reply(5).checkpoint(latestTurn: 4, runIsLive: false) == nil)
        #expect(ChangesScopeChoice.reply(2).checkpoint(latestTurn: 4, runIsLive: false) == DetailsCheckpoint(kind: .turn, turn: 2))
        #expect(ChangesScopeChoice.whenStopped.checkpoint(latestTurn: 4, runIsLive: true) == nil)
        #expect(ChangesScopeChoice.whenStopped.checkpoint(latestTurn: 4, runIsLive: false) == DetailsCheckpoint(kind: .final))
        #expect(ChangesScopeChoice.range(fromReply: 2, toReply: 3).checkpoint(latestTurn: 4, runIsLive: false)
            == DetailsCheckpoint(kind: .historical, fromTurn: 1, toTurn: 3))
        #expect(ChangesScopeChoice.range(fromReply: 0, toReply: 3).checkpoint(latestTurn: 4, runIsLive: false) == nil)
        #expect(ChangesScopeChoice.range(fromReply: 3, toReply: 2).checkpoint(latestTurn: 4, runIsLive: false) == nil)
        #expect(ChangesScopeChoice.range(fromReply: 2, toReply: 5).checkpoint(latestTurn: 4, runIsLive: false) == nil)
    }

    // MARK: - Session presentation

    static func session() -> DesktopSession {
        let suite = "jet.tests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        return DesktopSession(memory: ClientMemory(defaults: defaults))
    }

    @Test
    func terminalsAreNumberedInListOrder() {
        let session = Self.session()
        let workspaceID = UUID()
        let ids = [UUID(), UUID(), UUID()]
        session.workTerminals = [
            JetWorkspaceTerminal(id: ids[0], workspaceID: workspaceID, state: .open),
            JetWorkspaceTerminal(id: ids[1], workspaceID: workspaceID, state: .closed),
            JetWorkspaceTerminal(id: ids[2], workspaceID: workspaceID, state: .unavailable),
        ]
        #expect(session.terminalTitles == [
            ids[0]: "Terminal 1",
            ids[1]: "Terminal 2 (Closed)",
            ids[2]: "Terminal 3 (Closed)",
        ])
    }

    @Test
    func waitingMessagesUseTheTranscriptTextOrASourceFallback() {
        let session = Self.session()
        let fromHere = UUID()
        func entry(_ id: UUID, _ position: Int, _ source: JetTurnSource, state: JetTurnState = .queued) -> JetTurnQueueEntry {
            JetTurnQueueEntry(id: id, sequence: UInt64(position), position: position, source: source, state: state, runID: nil, withdrawable: true)
        }
        session.turnQueue = JetTurnQueue(cursor: 1, turns: [
            entry(UUID(), 4, .user),
            entry(UUID(), 3, .autoContinue),
            entry(UUID(), 2, .schedule),
            entry(fromHere, 1, .user),
            entry(UUID(), 0, .user, state: .active),
        ])
        session.timeline = [
            JetTimelineEntry(id: fromHere.uuidString.lowercased(), kind: .user, text: "Also add a test.", sequence: 1, rawCount: 0),
        ]
        #expect(session.queuedMessages.map(\.text) == [
            "Also add a test.",
            "A scheduled message",
            "An automatic follow-up",
            "Message from another Jet app",
        ])
        #expect(session.queuedMessages.map(\.ordinal) == (1 ... 4).map { JetCopy.ordinal($0) })
    }

    @Test
    func noticesPassWorkNoticesThroughAndClassifyFailures() {
        let session = Self.session()
        #expect(session.detailsNotice == nil)
        session.workNotice = "All changes loaded."
        #expect(session.detailsNotice == DetailsNotice(kind: .confirmation, text: "All changes loaded.", error: nil))

        let conflict = JetPresentationError(
            category: .conflict, code: "user_edit.stale_revision", message: "stale", retryable: false,
            recoveryActions: [.refreshFile]
        )
        session.workNotice = "stale"
        session.workNoticeError = conflict
        #expect(session.detailsNotice == DetailsNotice(
            kind: .warning,
            text: "This file changed since you opened it. Reload it to get the latest version.",
            error: conflict
        ))

        let failure = JetPresentationError(category: .invalidInput, code: "review.comment_invalid", message: "Enter a line.", retryable: false)
        session.workNotice = "Enter a line."
        session.workNoticeError = failure
        #expect(session.detailsNotice?.kind == .error)
        #expect(session.detailsNotice?.text == "Enter a line.")

        session.workNoticeError = .offline
        #expect(session.detailsNotice?.kind == .warning)
        #expect(session.detailsNotice?.text == "Jet can't reach This Mac right now.")
    }

    @Test
    func errorsReadInCasualWords() {
        func error(_ category: JetPresentationErrorCategory, _ code: String) -> JetPresentationError {
            JetPresentationError(category: category, code: code, message: "raw message", retryable: false)
        }
        func text(_ error: JetPresentationError, _ action: DetailsFailedAction = .other, fallback: String? = nil) -> String {
            DetailsCopy.error(error, computer: "Studio Mac", assistant: "Claude Code", action: action, fallback: fallback)
        }
        #expect(text(error(.offline, "transport.offline")) == "Jet can't reach Studio Mac right now.")
        #expect(text(error(.conflict, "user_edit.stale_revision"), .save)
            == "This file changed since you opened it. Reload it to get the latest version.")
        #expect(text(error(.outcomeUnknown, "command.outcome_unknown"), .save)
            == "Jet couldn't confirm the save. Click Save to check; Jet sends the same request.")
        #expect(text(error(.outcomeUnknown, "command.outcome_unknown"), .comment)
            == "Jet couldn't confirm the comment was sent. Send it again to check; it won't be sent twice.")
        #expect(text(error(.conflict, "checkpoint.run_active")) == "Available after Claude Code stops.")
        #expect(text(error(.notFound, "checkpoint.not_found")) == "These changes are no longer available.")
        #expect(text(error(.invalidInput, "review.comment_invalid")) == "raw message")
        #expect(text(error(.internalFailure, "internal"), fallback: "Something went wrong.") == "Something went wrong.")
    }

    @Test
    func recoveryButtonsUseDetailsTitles() {
        #expect(JetRecoveryAction.refreshFile.detailsTitle == "Reload File")
        #expect(JetRecoveryAction.refreshConversation(UUID()).detailsTitle == "Refresh Task")
        #expect(JetRecoveryAction.refreshRun(UUID()).detailsTitle == "Refresh")
        #expect(JetRecoveryAction.resumeEvents(after: 3).detailsTitle == "Reconnect")
    }

    @Test
    func casualStringsNeedNoJetWords() {
        let strings = DetailsCopy.casualStrings
        #expect(strings.count > 100)
        for string in strings {
            #expect(JetCopy.foundAvoidWords(in: string).isEmpty, "\(string)")
        }
    }

    // MARK: - Edit mode and scenes

#if DEBUG
    @Test
    func editModeIsPartOfChangesAndItsExitsKeepTheDraftRules() {
        let session = DesktopSession.preview {
            DesktopPreviewData.inspector($0)
            DetailsSeed.openEditor($0)
        }
        #expect(session.detailsTab == .changes)
        #expect(session.isEditingFile)
        #expect(session.hasUnsavedFileEdit)

        session.revertFileEdits()
        #expect(!session.hasUnsavedFileEdit)
        #expect(session.fileDraft == DetailsSeed.loginSource)

        session.finishEditingFile(reload: false)
        #expect(session.selectedWorkPanel == .changes)
        #expect(session.editableFile == nil)
        #expect(!session.isEditingFile)
    }

    @Test
    func leavingAnUnsavedEditAsksFirst() {
        let session = DesktopSession.preview {
            DesktopPreviewData.inspector($0)
            DetailsSeed.openEditor($0)
        }
        session.selectDetailsTab(.terminal)
        #expect(session.pendingNavigation != nil)
        #expect(session.selectedWorkPanel == .files)
        session.pendingNavigation?.perform()
        #expect(session.selectedWorkPanel == .terminal)
        #expect(session.editableFile == nil)
    }

    @Test
    func choosingAScopeSetsTheCheckpointAndIgnoresInvalidChoices() async {
        let session = DesktopSession.preview {
            DesktopPreviewData.inspector($0)
            DetailsSeed.apply(DetailsSeed.diff(files: DetailsSeed.threeFiles, patch: DetailsSeed.threeFilePatch, latestTurn: 3), to: $0)
        }
        await session.selectChangesScope(.reply(2))
        #expect(session.checkpointKind == .turn)
        #expect(session.checkpointTurn == 2)
        #expect(session.selectedWorkFilePath == nil)
        #expect(session.changesScopeChoice == .reply(2))

        await session.selectChangesScope(.range(fromReply: 2, toReply: 3))
        #expect(session.changesScopeChoice == .range(fromReply: 2, toReply: 3))

        await session.selectChangesScope(.reply(9))
        #expect(session.changesScopeChoice == .range(fromReply: 2, toReply: 3))

        // The Run is still live, so When Stopped isn't available.
        await session.selectChangesScope(.whenStopped)
        #expect(session.changesScopeChoice == .range(fromReply: 2, toReply: 3))
    }

    @Test
    func projectFolderTasksHaveNoWorkingCopyTerminal() {
        let session = DesktopSession.preview {
            DesktopPreviewData.inspector($0)
            DetailsSeed.useProjectFolder($0)
            DetailsSeed.apply(DetailsSeed.diff(files: [], patch: "", inProjectFolder: true), to: $0)
        }
        #expect(session.detailsWorksInProjectFolder)
        #expect(session.detailsWorkingCopyPath == DesktopPreviewData.webApp.root)
        #expect(session.detailsProjectName == "web-app")
    }

    @Test
    func detailsScenesAreUniqueAndComplete() {
        let ids = DesktopPreviewScenes.details.map(\.id)
        #expect(ids.count == 17)
        #expect(Set(ids).count == ids.count)
        #expect(ids.allSatisfy { $0.hasPrefix("details-") })
    }
#endif
}
