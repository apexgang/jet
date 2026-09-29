#if DEBUG
import SwiftUI

extension DesktopPreviewScenes {
    /// Details inspector states (WP8). Each starts from `DesktopPreviewData.inspector`
    /// and renders DetailsInspector on its own at the inspector's widths.
    @MainActor static var details: [DesktopPreviewScene] {
        [
            detailsScene("details-changes", size: CGSize(width: 300, height: 760)) {
                DetailsSeed.apply(DetailsSeed.diff(files: DetailsSeed.threeFiles, patch: DetailsSeed.threeFilePatch), to: $0)
            },
            detailsScene("details-changes-truncated", model: {
                let model = DetailsPanelModel()
                model.expandedPaths = [DetailsSeed.heroImage, DetailsSeed.sessionFile]
                return model
            }) {
                DetailsSeed.apply(DetailsSeed.truncatedDiff, to: $0)
                $0.selectedWorkFilePath = DetailsSeed.sessionFile
            },
            detailsScene("details-changes-earlier") { session in
                let run = DetailsSeed.currentRun(lifecycle: .active)
                DetailsSeed.useRuns([DetailsSeed.earlierRun, run], in: session)
                DetailsSeed.apply(
                    DetailsSeed.diff(
                        files: [DetailsSeed.file("src/auth/login.ts", .modified)],
                        patch: DetailsSeed.loginPatch,
                        scope: .turn(2),
                        latestTurn: 3
                    ),
                    to: session
                )
                session.checkpointKind = .turn
                session.checkpointTurn = 2
                session.timeline.append(DetailsSeed.changesEntry(reply: 2, run: run.id, minutesAgo: 6))
                session.timeline.append(DetailsSeed.changesEntry(reply: 3, run: run.id, minutesAgo: 2))
            },
            detailsScene("details-changes-offline") { session in
                DetailsSeed.apply(DetailsSeed.diff(files: DetailsSeed.threeFiles, patch: DetailsSeed.threeFilePatch), to: session)
                session.connectionState = .disconnected
                session.updatePlane(session.localPlaneRegistryID) { plane in
                    plane.connection = .disconnected
                    plane.failure = .offline
                }
                session.conversationFreshness = .cached
            },
            detailsScene("details-changes-empty") {
                DetailsSeed.apply(DetailsSeed.diff(files: [], patch: ""), to: $0)
            },
            detailsScene("details-changes-loading") {
                DetailsSeed.clearChanges($0)
                $0.workOperation = "refresh"
            },
            detailsScene("details-changes-error") {
                DetailsSeed.clearChanges($0)
                $0.workError = .offline
            },
            detailsScene("details-no-task") {
                $0.selectedConversationID = nil
                $0.conversationSnapshot = nil
                $0.runExecution = nil
                DetailsSeed.clearChanges($0)
            },
            detailsScene("details-edit") {
                DetailsSeed.apply(DetailsSeed.diff(files: DetailsSeed.threeFiles, patch: DetailsSeed.threeFilePatch), to: $0)
                DetailsSeed.openEditor($0)
            },
            detailsScene("details-edit-conflict") {
                DetailsSeed.apply(DetailsSeed.diff(files: DetailsSeed.threeFiles, patch: DetailsSeed.threeFilePatch), to: $0)
                DetailsSeed.openEditor($0)
                let conflict = JetPresentationError(
                    category: .conflict,
                    code: "user_edit.stale_revision",
                    message: "The file changed after it was read.",
                    retryable: false,
                    recoveryActions: [.refreshFile]
                )
                $0.workNoticeError = conflict
                $0.workNotice = $0.plainMessage(for: conflict)
            },
            DesktopPreviewScene(id: "details-comment-form", size: CGSize(width: 340, height: 260)) {
                let session = DesktopSession.preview {
                    DesktopPreviewData.inspector($0)
                    $0.reviewLine = 42
                    $0.reviewComment = "Keep the returnTo check here too, so a crafted link can't send people to another site."
                }
                return AnyView(
                    CommentOnLineForm(session: session, path: "src/auth/login.ts") {}
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                        .background(.background)
                        .tint(JetDesign.accent)
                )
            },
            DesktopPreviewScene(id: "details-range-form", size: CGSize(width: 300, height: 200)) {
                AnyView(
                    ChangesRangeForm(latest: 4, initialFrom: 2, initialTo: 3, onShow: { _, _ in }, onCancel: {})
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                        .background(.background)
                        .tint(JetDesign.accent)
                )
            },
            detailsScene("details-terminal") { session in
                DetailsSeed.apply(DetailsSeed.diff(files: DetailsSeed.threeFiles, patch: DetailsSeed.threeFilePatch), to: session)
                session.selectedWorkPanel = .terminal
                session.workTerminals = DetailsSeed.terminals
                session.selectedTerminalID = DetailsSeed.terminals[0].id
                session.attachedTerminalID = DetailsSeed.terminals[0].id
                session.terminalOutput[DetailsSeed.terminals[0].id] = DetailsSeed.terminalOutput
            },
            detailsScene("details-terminal-empty") {
                DetailsSeed.apply(DetailsSeed.diff(files: DetailsSeed.threeFiles, patch: DetailsSeed.threeFilePatch), to: $0)
                $0.selectedWorkPanel = .terminal
                $0.workTerminals = []
            },
            detailsScene("details-terminal-unavailable") { session in
                DetailsSeed.useProjectFolder(session)
                DetailsSeed.apply(
                    DetailsSeed.diff(files: DetailsSeed.threeFiles, patch: DetailsSeed.threeFilePatch, inProjectFolder: true),
                    to: session
                )
                session.selectedWorkPanel = .terminal
            },
            detailsScene("details-activity") { session in
                DesktopPreviewData.working(session)
                session.isWorkPanelPresented = true
                session.selectedWorkPanel = .run
                let run = DesktopPreviewData.makeRun(for: DesktopPreviewData.loginRedirect, lifecycle: .active)
                session.turnQueue = JetTurnQueue(cursor: DesktopPreviewData.headCursor, turns: [
                    JetTurnQueueEntry(id: DetailsSeed.uuid(700), sequence: 301, position: 1, source: .user, state: .active, runID: run.id, withdrawable: false),
                    JetTurnQueueEntry(id: DesktopPreviewData.followUpTurnID, sequence: 308, position: 2, source: .user, state: .queued, runID: run.id, withdrawable: true),
                    JetTurnQueueEntry(id: DetailsSeed.uuid(701), sequence: 309, position: 3, source: .schedule, state: .queued, runID: nil, withdrawable: false),
                ])
                DetailsSeed.apply(DetailsSeed.diff(files: DetailsSeed.threeFiles, patch: DetailsSeed.threeFilePatch, latestTurn: 1), to: session)
            },
            detailsScene("details-activity-finished", model: {
                let model = DetailsPanelModel()
                model.showsTechnicalDetails = true
                return model
            }) { session in
                let run = DetailsSeed.currentRun(lifecycle: .completed)
                session.runExecution = JetRunExecution(
                    cursor: DesktopPreviewData.headCursor,
                    run: run,
                    activity: nil,
                    needsAttention: false,
                    termination: nil
                )
                DetailsSeed.useProjectFolder(session, runs: [run])
                DetailsSeed.apply(
                    DetailsSeed.diff(files: DetailsSeed.threeFiles, patch: DetailsSeed.threeFilePatch, inProjectFolder: true),
                    to: session
                )
                session.selectedWorkPanel = .run
            },
        ]
    }

    /// A Details scene: the inspector alone, seeded after `DesktopPreviewData.inspector`.
    @MainActor private static func detailsScene(
        _ id: String,
        size: CGSize = CGSize(width: 360, height: 800),
        model: (@MainActor () -> DetailsPanelModel)? = nil,
        seed: @escaping @MainActor (DesktopSession) -> Void
    ) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id, size: size) {
            let session = DesktopSession.preview {
                DesktopPreviewData.inspector($0)
                seed($0)
            }
            let inspector = model.map { DetailsInspector(session: session, previewModel: $0()) }
                ?? DetailsInspector(session: session)
            return AnyView(
                inspector
                    .frame(width: size.width, height: size.height)
                    .background(.background)
                    .tint(JetDesign.accent)
            )
        }
    }
}

/// Seed data for the Details scenes and tests.
@MainActor
enum DetailsSeed {
    enum Status {
        case added
        case modified
        case deleted
    }

    static let heroImage = "public/images/sign-in-hero.png"
    static let sessionFile = "src/auth/session.ts"

    static func uuid(_ index: Int) -> UUID {
        UUID(uuidString: String(format: "7A3C0000-0000-4000-8000-%012ld", 900 + index)) ?? UUID()
    }

    private static let zeros = String(repeating: "0", count: 40)

    /// A changed file as the change diff lists it.
    static func file(
        _ path: String,
        _ status: Status,
        origin: String = "agent",
        contentAvailable: Bool = true
    ) -> JetChangedFile {
        let before = String(repeating: "3f2a1c4e5b", count: 4)
        let after = String(repeating: "8b7d9e0a1f", count: 4)
        let objects: (String?, String?) = switch status {
        case .added: (zeros, after)
        case .modified: (before, after)
        case .deleted: (before, zeros)
        }
        return JetChangedFile(
            path: path,
            beforeObject: contentAvailable ? objects.0 : nil,
            afterObject: contentAvailable ? objects.1 : nil,
            beforeSize: status == .added ? 0 : 1_204,
            afterSize: status == .deleted ? 0 : 1_391,
            origin: origin
        )
    }

    /// A change diff of the open task's Run. The patch is cut off when
    /// `artifactSize` is larger than it.
    static func diff(
        files: [JetChangedFile],
        patch: String,
        scope: JetChangeScope = .current,
        latestTurn: UInt32 = 3,
        totalFiles: Int? = nil,
        nextPage: UUID? = nil,
        artifactSize: UInt64? = nil,
        availability: JetArtifactAvailability = .stored,
        contentComplete: Bool = true,
        inProjectFolder: Bool = false
    ) -> JetChangeDiff {
        let size = artifactSize ?? UInt64(patch.utf8.count)
        return JetChangeDiff(
            cursor: DesktopPreviewData.headCursor,
            runID: DesktopPreviewData.runID(1),
            workspaceID: inProjectFolder ? nil : DesktopPreviewData.workspaceID,
            scope: scope,
            latestTurn: latestTurn,
            totalFiles: UInt32(totalFiles ?? files.count),
            files: files,
            nextPage: nextPage,
            patch: patch,
            patchTruncated: size > UInt64(patch.utf8.count),
            contentComplete: contentComplete,
            artifact: JetChangeArtifact(
                sha256: String(repeating: "5d", count: 32),
                size: size,
                availability: availability
            )
        )
    }

    /// Puts a change diff into the session as a finished load would.
    static func apply(_ diff: JetChangeDiff, to session: DesktopSession) {
        session.workDiff = diff
        session.workFiles = diff.files
        session.workNextPage = diff.nextPage
        session.workPatch = diff.patch
        session.workArtifactBytes = Data(diff.patch.utf8)
        session.workTarget = diff.workspaceID.map(JetFileTarget.workspace) ?? .project(DesktopPreviewData.webApp.id)
        session.workOperation = nil
        session.workError = nil
        session.selectedWorkFilePath = diff.files.first?.path
    }

    static func clearChanges(_ session: DesktopSession) {
        session.workDiff = nil
        session.workFiles = []
        session.workNextPage = nil
        session.workPatch = ""
        session.workArtifactBytes = Data()
        session.selectedWorkFilePath = nil
    }

    // MARK: Runs

    static func currentRun(lifecycle: JetRunLifecycle) -> JetRunSummary {
        DesktopPreviewData.makeRun(for: DesktopPreviewData.loginRedirect, lifecycle: lifecycle, startedAgo: 40 * DesktopPreviewData.minute)
    }

    static var earlierRun: JetRunSummary {
        JetRunSummary(
            id: uuid(1),
            conversationID: DesktopPreviewData.loginRedirect.id,
            revision: 4,
            lifecycle: .completed,
            title: DesktopPreviewData.loginRedirect.title,
            createdAtUnixMilliseconds: DesktopPreviewData.now - 26 * DesktopPreviewData.hour,
            endedAtUnixMilliseconds: DesktopPreviewData.now - 25 * DesktopPreviewData.hour
        )
    }

    /// Replaces the open task's snapshot with these Runs; the last one is current.
    static func useRuns(_ runs: [JetRunSummary], in session: DesktopSession) {
        guard let snapshot = session.conversationSnapshot else { return }
        session.conversationSnapshot = JetConversationSnapshot(
            cursor: snapshot.cursor,
            conversation: snapshot.conversation,
            workspaceID: snapshot.workspaceID,
            workspaceRoot: snapshot.workspaceRoot,
            runs: runs
        )
        if let current = runs.last, let execution = session.runExecution {
            session.runExecution = JetRunExecution(
                cursor: execution.cursor,
                run: current,
                activity: execution.activity,
                needsAttention: execution.needsAttention,
                termination: execution.termination
            )
        }
    }

    /// The task works directly in its project folder: no separate working copy.
    static func useProjectFolder(_ session: DesktopSession, runs: [JetRunSummary]? = nil) {
        guard let snapshot = session.conversationSnapshot else { return }
        session.conversationSnapshot = JetConversationSnapshot(
            cursor: snapshot.cursor,
            conversation: snapshot.conversation,
            workspaceID: nil,
            workspaceRoot: nil,
            runs: runs ?? snapshot.runs
        )
        session.workTerminals = []
    }

    /// A transcript entry recording a reply's changes.
    static func changesEntry(reply: UInt32, run: UUID, minutesAgo: Int64) -> JetTimelineEntry {
        JetTimelineEntry(
            id: "changes-\(reply)",
            kind: .result,
            text: "Jet recorded the completed turn and its changes.",
            sequence: 400 + UInt64(reply),
            rawCount: 0,
            recordedAtUnixMilliseconds: DesktopPreviewData.now - minutesAgo * DesktopPreviewData.minute,
            runID: run,
            checkpointTurn: reply
        )
    }

    // MARK: Edit mode

    static let loginSource = """
    import { verify } from "./credentials"
    import { redirectAfterSignIn } from "./guard"
    import { createSession, destroySession } from "./session"
    import { redirect } from "./http"

    export const loginPath = "/login"

    export async function signIn(form: FormData) {
      const user = await verify(form)
      if (!user) {
        return redirect(`${loginPath}?error=invalid`)
      }

      const returnTo = form.get("returnTo")?.toString()
      const session = await createSession(user, { returnTo })
      return redirectAfterSignIn(session)
    }

    export async function signOut(request: Request) {
      await destroySession(request)
      return redirect(loginPath)
    }

    """

    /// Changes › edit mode on login.ts with an unsaved edit.
    static func openEditor(_ session: DesktopSession) {
        let path = "src/auth/login.ts"
        session.selectedWorkPanel = .files
        session.selectedWorkFilePath = path
        session.editableFile = JetEditableFile(
            cursor: DesktopPreviewData.headCursor,
            target: .workspace(DesktopPreviewData.workspaceID),
            path: path,
            content: loginSource,
            revision: JetFileRevision(object: String(repeating: "4aa81f3b2c", count: 4), mode: "100644")
        )
        session.fileDraft = loginSource.replacingOccurrences(
            of: "  const returnTo = form.get(\"returnTo\")?.toString()",
            with: "  // Only same-site paths; guard.ts checks the rest.\n  const returnTo = form.get(\"returnTo\")?.toString()"
        )
    }

    // MARK: Terminals

    static let terminals: [JetWorkspaceTerminal] = [
        JetWorkspaceTerminal(id: uuid(10), workspaceID: DesktopPreviewData.workspaceID, state: .open),
        JetWorkspaceTerminal(id: uuid(11), workspaceID: DesktopPreviewData.workspaceID, state: .closed),
    ]

    static let terminalOutput = """
    $ npm test -- src/auth

    > web-app@2.4.0 test
    > vitest run src/auth

     ✓ src/auth/guard.test.ts (4 tests) 12ms
     ✓ src/auth/login.test.ts (6 tests) 18ms

     Test Files  2 passed (2)
          Tests  10 passed (10)
       Duration  412ms

    $
    """

    // MARK: Patches

    static let threeFiles: [JetChangedFile] = [
        file("src/auth/guard.ts", .added),
        file("src/auth/legacy-redirect.ts", .deleted),
        file("src/auth/login.ts", .modified),
    ]

    static let guardPatch = """
    diff --git a/src/auth/guard.ts b/src/auth/guard.ts
    new file mode 100644
    index 0000000000000000000000000000000000000000..8b7d9e0a1f2e3d4c5b6a7f8e9d0c1b2a3f4e5d6c
    --- /dev/null
    +++ b/src/auth/guard.ts
    @@ -0,0 +1,28 @@
    +import { redirect } from "./http"
    +import { readSession, type Session } from "./session"
    +
    +// Paths that never need a signed-in session.
    +const publicPaths = new Set(["/login", "/signup", "/reset-password"])
    +
    +export function requireSession(request: Request) {
    +  const url = new URL(request.url)
    +  if (publicPaths.has(url.pathname)) {
    +    return null
    +  }
    +
    +  const session = readSession(request)
    +  if (!session) {
    +    const returnTo = url.pathname + url.search
    +    return redirect(`/login?returnTo=${encodeURIComponent(returnTo)}`)
    +  }
    +  return session
    +}
    +
    +export function redirectAfterSignIn(session: Session) {
    +  const target = session.returnTo ?? "/dashboard"
    +  // Only same-site paths, so a crafted link can't send people elsewhere.
    +  if (!target.startsWith("/") || target.startsWith("//")) {
    +    return redirect("/dashboard")
    +  }
    +  return redirect(target)
    +}

    """

    static let legacyPatch = """
    diff --git a/src/auth/legacy-redirect.ts b/src/auth/legacy-redirect.ts
    deleted file mode 100644
    index 5e4d3c2b1a0f9e8d7c6b5a4f3e2d1c0b9a8f7e6d..0000000000000000000000000000000000000000
    --- a/src/auth/legacy-redirect.ts
    +++ /dev/null
    @@ -1,4 +0,0 @@
    -// Kept for old bookmarks; every sign-in went back to /login.
    -export function legacyRedirect() {
    -  return "/login"
    -}

    """

    static let loginPatch = """
    diff --git a/src/auth/login.ts b/src/auth/login.ts
    index 91c0d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9..4aa81f3b2c1d0e9f8a7b6c5d4e3f2a1b0c9d8e7f 100644
    --- a/src/auth/login.ts
    +++ b/src/auth/login.ts
    @@ -1,5 +1,6 @@
     import { verify } from "./credentials"
    -import { legacyRedirect } from "./legacy-redirect"
    +import { redirectAfterSignIn } from "./guard"
    +import { createSession, destroySession } from "./session"
     import { redirect } from "./http"
    \u{20}
     export const loginPath = "/login"
    @@ -21,4 +22,14 @@ export async function signIn(form: FormData) {
       const user = await verify(form)
    -  await createSession(user)
    -  return redirect(legacyRedirect())
    +  if (!user) {
    +    return redirect(`${loginPath}?error=invalid`)
    +  }
    +
    +  const returnTo = form.get("returnTo")?.toString()
    +  const session = await createSession(user, { returnTo })
    +  return redirectAfterSignIn(session)
    +}
    +
    +export async function signOut(request: Request) {
    +  await destroySession(request)
    +  return redirect(loginPath)
     }

    """

    /// guard.ts added (+28), legacy-redirect.ts deleted (−4) and login.ts
    /// modified in two hunks (+14 −3): +42 −7 in all.
    static let threeFilePatch = guardPatch + legacyPatch + loginPatch

    static let heroPatch = """
    diff --git a/public/images/sign-in-hero.png b/public/images/sign-in-hero.png
    new file mode 100644
    index 0000000000000000000000000000000000000000..3c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d
    GIT binary patch
    literal 1204
    zcmV-u1Ww&~P)<h;3K|Lk000e1NJLTq002M$002M$0ssI2+1jZq0000FbW%=J0RR90
    zfB*mhfB*mhKmY&$0Qj6a{r~^~2XskIMF-*o5D+#9pgCg*00005bVXQnQ*UN;cVTj6

    literal 0
    HcmV?d00001


    """

    /// session.ts, cut off inside its second hunk.
    static let sessionPatchStart = """
    diff --git a/src/auth/session.ts b/src/auth/session.ts
    index 1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c..2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d 100644
    --- a/src/auth/session.ts
    +++ b/src/auth/session.ts
    @@ -3,6 +3,7 @@ import { cookies } from "./http"
     export interface Session {
       userID: string
       expiresAt: number
    +  returnTo?: string
     }
    \u{20}
     const maxAge = 60 * 60 * 24 * 14
    @@ -18,9 +19,14 @@ export async function createSession(user: User) {
    -export async function createSession(user: User) {
    +export async function createSession(user: User, options: { returnTo?: string } = {}) {
       const session: Session = {
         userID: user.id,
         expiresAt: Date.now() + maxAge * 1000,
    +    returnTo: options.returnTo,
       }
    """

    /// Five files, four listed, a binary image and a cut-off patch that can load
    /// more.
    static var truncatedDiff: JetChangeDiff {
        let patch = heroPatch + guardPatch + loginPatch + sessionPatchStart
        return diff(
            files: [
                file(heroImage, .added),
                file("src/auth/guard.ts", .added),
                file("src/auth/login.ts", .modified),
                file(sessionFile, .modified),
            ],
            patch: patch,
            totalFiles: 5,
            nextPage: uuid(20),
            artifactSize: UInt64(patch.utf8.count) + 18_400
        )
    }
}
#endif
