#if DEBUG
import Foundation

/// Representative live-path data for previews and screenshots. Everything is
/// built with the client's memberwise initializers; nothing reaches a Plane.
@MainActor
enum DesktopPreviewData {
    // MARK: - Fixed values

    /// 2026-09-22 14:13 UTC, so relative times stay stable across renders.
    static let now: Int64 = 1_790_000_000_000
    static let minute: Int64 = 60_000
    static let hour: Int64 = 3_600_000
    static let headCursor: UInt64 = 400

    static let negotiation = JetNegotiation(
        protocolVersion: 1,
        minorVersion: 43,
        codec: "json-v1",
        frameLimits: .protocolMaximum
    )

    static let planeID = uuid("7A3C0000-0000-4000-8000-000000000001")
    static let webApp = JetProjectSummary(
        id: uuid("7A3C0000-0000-4000-8000-000000000101"),
        root: "/Users/alex/code/web-app"
    )
    static let billingService = JetProjectSummary(
        id: uuid("7A3C0000-0000-4000-8000-000000000102"),
        root: "/Users/alex/code/billing-service"
    )
    nonisolated static let claudeCraftID = "jet-craft-claude"
    nonisolated static let codexCraftID = "jet-craft-codex"
    static let workspaceID = uuid("7A3C0000-0000-4000-8000-000000000201")
    static let workingCopyRoot = "/Users/alex/.jet/workspaces/web-app-7f3a"

    static var setupSnapshot: JetSetupSnapshot {
        JetSetupSnapshot(
            status: JetPlaneStatus(
                cursor: headCursor,
                planeID: planeID,
                daemonStarts: 3,
                startedAtUnixMilliseconds: now - 2 * hour,
                coreVersion: "1.4.0",
                security: nil,
                recovery: nil
            ),
            capabilities: JetCapabilitySummary(
                coreVersion: "1.4.0",
                platform: "macos-aarch64",
                externalTools: [
                    JetExternalToolSummary(tool: "git", availability: .present(version: "2.50.1")),
                ],
                harnesses: ["claude-code", "codex"],
                crafts: [
                    JetInstalledCraft(id: claudeCraftID, version: "1.4.0", harnesses: ["claude-code"]),
                    JetInstalledCraft(id: codexCraftID, version: "1.4.0", harnesses: ["codex"]),
                ],
                credentialStore: .available,
                degraded: []
            ),
            projects: JetProjectList(cursor: headCursor, projects: [webApp, billingService]),
            accounts: JetAccountBindingList(
                cursor: headCursor,
                bindings: [
                    JetAccountBindingSummary(
                        id: uuid("7A3C0000-0000-4000-8000-000000000301"),
                        provider: "anthropic",
                        label: "Claude Code sign-in",
                        state: "available",
                        stateLabel: "Available"
                    ),
                ]
            ),
            pairing: JetPairingSummary(
                cursor: headCursor,
                gate: "closed",
                pairedClients: 0,
                hasPendingOffer: false
            )
        )
    }

    // MARK: - Tasks

    /// Seven tasks, newest first, as the sidebar lists them.
    static let conversations: [JetConversationSummary] = [
        task(1, "Fix login redirect loop", project: webApp, age: 25 * minute),
        task(2, "Update payment dependencies", project: billingService, age: 50 * minute),
        task(3, "Add dark mode to settings", project: webApp, age: 2 * hour),
        task(4, "Speed up the dashboard query", project: webApp, age: 5 * hour),
        task(5, "Explain the invoice retry logic", project: billingService, age: 26 * hour),
        task(6, "Add CSV export to reports", project: webApp, age: 50 * hour),
        task(7, "Fix flaky checkout test", project: billingService, age: 4 * 24 * hour),
    ]

    static var loginRedirect: JetConversationSummary { conversations[0] }
    static var paymentDependencies: JetConversationSummary { conversations[1] }

    static func runID(_ index: Int) -> UUID {
        uuid(String(format: "7A3C0000-0000-4000-8000-%012ld", 500 + index))
    }

    /// A Run of one task.
    static func makeRun(
        for conversation: JetConversationSummary,
        index: Int = 1,
        lifecycle: JetRunLifecycle,
        startedAgo: Int64 = 12 * 60_000
    ) -> JetRunSummary {
        JetRunSummary(
            id: runID(index),
            conversationID: conversation.id,
            revision: 7,
            lifecycle: lifecycle,
            title: conversation.title,
            createdAtUnixMilliseconds: now - startedAgo,
            endedAtUnixMilliseconds: lifecycle.isLive ? nil : now - minute
        )
    }

    static func makeExecution(
        _ run: JetRunSummary,
        activity: JetRunActivity?,
        needsAttention: Bool = false
    ) -> JetRunExecution {
        JetRunExecution(
            cursor: headCursor,
            run: run,
            activity: activity,
            needsAttention: needsAttention,
            termination: nil
        )
    }

    // MARK: - Transcript

    static let followUpTurnID = uuid("7A3C0000-0000-4000-8000-000000000601")

    /// The login-redirect transcript. Unfinished, it stops while the reply is still
    /// working and a follow-up message waits to send.
    static func loginTranscript(finished: Bool) -> [JetTimelineEntry] {
        let run = runID(1)
        var entries: [JetTimelineEntry] = [
            entry("u1", .user, "After signing in, the app sends me back to /login instead of the page I was on. Can you find the cause and fix it?", sequence: 301, run: run, at: 12),
            entry("t1", .agent, "Read", sequence: 302, run: run, at: 11),
            entry("t2", .agent, "Grep", sequence: 303, run: run, at: 11),
            entry(
                "a1",
                .agent,
                """
                The loop comes from `redirectTo` in **src/auth/guard.ts**. After sign-in it still points at `/login`, so the guard sends you straight back.

                Plan:

                1. Remember the page you came from before redirecting to sign-in
                2. Return there after sign-in, falling back to `/dashboard`
                3. Add a regression test

                ```ts
                const target = session.returnTo ?? "/dashboard"
                return redirect(target)
                ```
                """,
                sequence: 304,
                run: run,
                at: 10
            ),
            entry("x1", .activity, "Run is working.", sequence: 305, run: run, at: 10, technical: true),
            entry("t3", .agent, "Edit", sequence: 306, run: run, at: 9),
            entry("t4", .agent, "Edit", sequence: 307, run: run, at: 8),
        ]
        if finished {
            entries += [
                entry("t5", .agent, "Bash", sequence: 308, run: run, at: 7),
                entry("t6", .agent, "Write", sequence: 309, run: run, at: 6),
                JetTimelineEntry(
                    id: "raw-310",
                    kind: .activity,
                    text: "3 background updates",
                    sequence: 312,
                    rawCount: 3,
                    recordedAtUnixMilliseconds: now - 5 * minute,
                    runID: run,
                    isTechnical: true
                ),
                JetTimelineEntry(
                    id: "313-result",
                    kind: .result,
                    text: "Jet recorded the completed turn and its changes.",
                    sequence: 313,
                    rawCount: 0,
                    recordedAtUnixMilliseconds: now - 4 * minute,
                    runID: run,
                    checkpointTurn: 1
                ),
                entry(
                    "a2",
                    .agent,
                    "Fixed. `guard.ts` now remembers where you were going, and the new `guard.test.ts` covers both redirects. All **42 tests** pass.",
                    sequence: 314,
                    run: run,
                    at: 4
                ),
            ]
        } else {
            entries.append(
                entry(followUpTurnID.uuidString.lowercased(), .user, "Also add a test for an expired session.", sequence: 308, run: nil, at: 1)
            )
        }
        return entries
    }

    static let permissionTranscript: [JetTimelineEntry] = {
        let run = runID(2)
        let approval = JetApprovalPresentation(
            requestID: "req-1",
            reviewID: nil,
            runID: run,
            tool: "Bash",
            action: #"{"command":"npm install stripe@17.4.0 @stripe/stripe-js@5.2.0","cwd":"/Users/alex/code/billing-service"}"#,
            target: "/Users/alex/code/billing-service",
            scope: "This action once",
            consequence: "This reply stays paused until the request is answered.",
            rationale: nil,
            state: .requested,
            canAuthorizeRetry: false
        )
        return [
            entry("p-u1", .user, "Update the Stripe SDK and the other payment dependencies to their latest minor versions, then run the tests.", sequence: 351, run: run, at: 9),
            entry("p-t1", .agent, "Read", sequence: 352, run: run, at: 8),
            entry("p-a1", .agent, "I'll update `stripe` and `@stripe/stripe-js`, then run the test suite.", sequence: 353, run: run, at: 7),
            JetTimelineEntry(
                id: "approval-\(run.uuidString.lowercased())-req-1",
                kind: .approval,
                text: "Bash needs approval.",
                sequence: 354,
                rawCount: 0,
                approval: approval,
                recordedAtUnixMilliseconds: now - 6 * minute,
                runID: run
            ),
        ]
    }()

    // MARK: - Changes

    static let patch = """
    diff --git a/src/auth/guard.ts b/src/auth/guard.ts
    index 3f2a1c4..8b7d9e0 100644
    --- a/src/auth/guard.ts
    +++ b/src/auth/guard.ts
    @@ -10,8 +10,11 @@ export function requireSession(request: Request) {
       const session = readSession(request)
       if (!session) {
    -    return redirect("/login")
    +    const returnTo = new URL(request.url).pathname
    +    return redirect(`/login?returnTo=${encodeURIComponent(returnTo)}`)
       }
       return session
     }
    \u{20}
    -export const redirectTo = "/login"
    +export function redirectAfterSignIn(session: Session) {
    +  return redirect(session.returnTo ?? "/dashboard")
    +}
    diff --git a/src/auth/login.ts b/src/auth/login.ts
    index 91c0d2e..4aa81f3 100644
    --- a/src/auth/login.ts
    +++ b/src/auth/login.ts
    @@ -21,4 +21,5 @@ export async function signIn(form: FormData) {
       const user = await verify(form)
    -  await createSession(user)
    -  return redirect(redirectTo)
    +  const returnTo = form.get("returnTo")?.toString()
    +  const session = await createSession(user, { returnTo })
    +  return redirectAfterSignIn(session)
     }
    diff --git a/src/auth/guard.test.ts b/src/auth/guard.test.ts
    new file mode 100644
    index 0000000..c1e5a77
    --- /dev/null
    +++ b/src/auth/guard.test.ts
    @@ -0,0 +1,12 @@
    +import { describe, expect, it } from "vitest"
    +import { redirectAfterSignIn } from "./guard"
    +
    +describe("redirectAfterSignIn", () => {
    +  it("returns to the page the person came from", () => {
    +    const response = redirectAfterSignIn({ returnTo: "/settings" } as Session)
    +    expect(response.headers.get("Location")).toBe("/settings")
    +  })
    +  it("falls back to the dashboard", () => {
    +    expect(redirectAfterSignIn({} as Session).headers.get("Location")).toBe("/dashboard")
    +  })
    +})

    """

    static let changedFiles: [JetChangedFile] = [
        JetChangedFile(
            path: "src/auth/guard.ts",
            beforeObject: "3f2a1c4e5b6d7a8f9e0d1c2b3a4f5e6d7c8b9a0f",
            afterObject: "8b7d9e0a1f2e3d4c5b6a7f8e9d0c1b2a3f4e5d6c",
            beforeSize: 1_204,
            afterSize: 1_391,
            origin: "run"
        ),
        JetChangedFile(
            path: "src/auth/login.ts",
            beforeObject: "91c0d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9",
            afterObject: "4aa81f3b2c1d0e9f8a7b6c5d4e3f2a1b0c9d8e7f",
            beforeSize: 2_310,
            afterSize: 2_402,
            origin: "run"
        ),
        JetChangedFile(
            path: "src/auth/guard.test.ts",
            beforeObject: "0000000000000000000000000000000000000000",
            afterObject: "c1e5a77d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3b",
            beforeSize: 0,
            afterSize: 498,
            origin: "run"
        ),
    ]

    static var changeDiff: JetChangeDiff {
        JetChangeDiff(
            cursor: headCursor,
            runID: runID(1),
            workspaceID: workspaceID,
            scope: .current,
            latestTurn: 1,
            totalFiles: UInt32(changedFiles.count),
            files: changedFiles,
            nextPage: nil,
            patch: patch,
            patchTruncated: false,
            contentComplete: true,
            artifact: JetChangeArtifact(
                sha256: String(repeating: "5d", count: 32),
                size: UInt64(patch.utf8.count),
                availability: .stored
            )
        )
    }

    // MARK: - Scenes (design §6.4)

    /// This Mac is connected with two projects, both assistants and seven tasks.
    static func connect(_ session: DesktopSession) {
        let local = session.localPlaneRegistryID
        let snapshot = setupSnapshot
        session.setupState = .ready(snapshot)
        session.connectionState = .connected(negotiation)
        session.updatePlane(local) { plane in
            plane.planeID = snapshot.status.planeID
            plane.connection = .connected(negotiation)
            plane.snapshot = snapshot
            plane.failure = nil
            plane.conversationCursor = headCursor
        }
        session.planeConversations[local] = conversations
        session.planeConversationCursors[local] = headCursor
        for conversation in conversations {
            session.conversationPlaneRegistryIDs[conversation.id] = local
        }
        session.rebuildConversationAggregation()
        session.conversationCursor = headCursor
        session.conversationFreshness = .live
        session.selectedProjectID = webApp.id
        session.chosenCraftID = claudeCraftID
        seedSidebarStatuses(session)
    }

    /// Working · Editing files, with Interrupt available and one waiting message.
    static func working(_ session: DesktopSession) {
        connect(session)
        let task = loginRedirect
        let run = makeRun(for: task, lifecycle: .active)
        open(
            session,
            task,
            run: run,
            execution: makeExecution(run, activity: .working),
            timeline: loginTranscript(finished: false),
            queue: JetTurnQueue(cursor: headCursor, turns: [
                JetTurnQueueEntry(id: uuid("7A3C0000-0000-4000-8000-000000000600"), sequence: 301, position: 1, source: .user, state: .active, runID: run.id, withdrawable: false),
                JetTurnQueueEntry(id: followUpTurnID, sequence: 308, position: 2, source: .user, state: .queued, runID: run.id, withdrawable: true),
            ])
        )
    }

    /// Waiting for your reply, with changes ready to keep.
    static func waitingWithChanges(_ session: DesktopSession) {
        connect(session)
        let task = loginRedirect
        let run = makeRun(for: task, lifecycle: .active)
        open(
            session,
            task,
            run: run,
            execution: makeExecution(run, activity: .waitingForUser),
            timeline: loginTranscript(finished: true),
            queue: JetTurnQueue(cursor: headCursor, turns: [])
        )
        session.workDiff = changeDiff
        session.workFiles = changedFiles
        session.workPatch = patch
        session.workTarget = .workspace(workspaceID)
        session.selectedWorkFilePath = changedFiles.first?.path
    }

    /// Needs permission: the assistant waits for a command the app can't approve.
    static func needsPermission(_ session: DesktopSession) {
        connect(session)
        let task = paymentDependencies
        let run = makeRun(for: task, index: 2, lifecycle: .active, startedAgo: 9 * minute)
        open(
            session,
            task,
            run: run,
            execution: makeExecution(run, activity: .waitingForApproval, needsAttention: true),
            timeline: permissionTranscript,
            queue: JetTurnQueue(cursor: headCursor, turns: [
                JetTurnQueueEntry(id: uuid("7A3C0000-0000-4000-8000-000000000602"), sequence: 351, position: 1, source: .user, state: .active, runID: run.id, withdrawable: false),
            ]),
            assistant: claudeCraftID
        )
    }

    /// Offline · showing saved view: This Mac's helper stopped answering.
    static func offline(_ session: DesktopSession) {
        waitingWithChanges(session)
        session.connectionState = .disconnected
        session.updatePlane(session.localPlaneRegistryID) { plane in
            plane.connection = .disconnected
            // A recorded failure makes the computer offline, not just between connections.
            plane.failure = .offline
        }
        session.conversationFreshness = .cached
    }

    /// Details open on Changes for a task waiting for your reply.
    static func inspector(_ session: DesktopSession) {
        waitingWithChanges(session)
        session.selectedWorkPanel = .changes
        session.isWorkPanelPresented = true
    }

    // MARK: - Helpers

    /// Makes a task the open one without loading anything.
    static func open(
        _ session: DesktopSession,
        _ task: JetConversationSummary,
        run: JetRunSummary,
        execution: JetRunExecution,
        timeline: [JetTimelineEntry],
        queue: JetTurnQueue,
        assistant: String = claudeCraftID
    ) {
        let snapshot = JetConversationSnapshot(
            cursor: headCursor,
            conversation: task,
            workspaceID: workspaceID,
            workspaceRoot: workingCopyRoot,
            runs: [run]
        )
        session.sidebarSelection = .conversation
        session.selectedConversationID = task.id
        session.conversationSnapshot = snapshot
        session.runExecution = execution
        session.turnQueue = queue
        session.timeline = timeline
        session.conversationFreshness = .live
        session.memory.recordAssistant(assistant, for: task.id)
        session.transcripts.markObservedStart(task.id)
        session.transcripts.save(timeline, for: task.id)
        session.recordSelectedSnapshot(snapshot)
    }

    /// Sidebar rows for the design's glyphs: working, needs permission, waiting
    /// with an unread reply, failed, stopped, unknown and finished.
    static func seedSidebarStatuses(_ session: DesktopSession) {
        let rows: [(Int, TaskStatusFacts?)] = [
            (0, TaskStatusFacts(lifecycle: .active, activity: .working, runID: runID(1), hasRuns: true, lastSequence: 380)),
            (1, TaskStatusFacts(lifecycle: .active, activity: .waitingForApproval, runID: runID(2), hasRuns: true, lastSequence: 360)),
            (2, TaskStatusFacts(lifecycle: .active, activity: .waitingForUser, runID: runID(3), hasRuns: true, hasRecordedChanges: true, lastSequence: 340, lastReplySequence: 338, lastInputSequence: 320)),
            (3, TaskStatusFacts(lifecycle: .failed, runID: runID(4), hasRuns: true, lastSequence: 250)),
            (4, TaskStatusFacts(lifecycle: .canceled, runID: runID(5), hasRuns: true, lastSequence: 200)),
            (5, nil),
            (6, TaskStatusFacts(lifecycle: .completed, runID: runID(7), hasRuns: true, hasRecordedChanges: true, lastSequence: 100)),
        ]
        for (index, facts) in rows {
            let conversation = conversations[index]
            session.memory.recordAssistant(index == 4 ? codexCraftID : claudeCraftID, for: conversation.id)
            if let facts { session.statusStore.seedForPreview(facts, for: conversation.id) }
        }
    }

    private static func task(
        _ index: Int,
        _ title: String,
        project: JetProjectSummary,
        age: Int64
    ) -> JetConversationSummary {
        JetConversationSummary(
            id: uuid(String(format: "7A3C0000-0000-4000-8000-%012ld", 400 + index)),
            revision: 3,
            title: title,
            createdAtUnixMilliseconds: now - age,
            projectID: project.id
        )
    }

    private static func entry(
        _ id: String,
        _ kind: JetTimelineKind,
        _ text: String,
        sequence: UInt64,
        run: UUID?,
        at minutesAgo: Int64,
        technical: Bool = false
    ) -> JetTimelineEntry {
        JetTimelineEntry(
            id: id,
            kind: kind,
            text: text,
            sequence: sequence,
            rawCount: 0,
            recordedAtUnixMilliseconds: now - minutesAgo * minute,
            runID: run,
            isTechnical: technical
        )
    }

    private static func uuid(_ value: String) -> UUID {
        guard let id = UUID(uuidString: value) else {
            preconditionFailure("Invalid preview UUID \(value)")
        }
        return id
    }
}
#endif
