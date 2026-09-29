#if DEBUG
import SwiftUI

extension DesktopPreviewScenes {
    /// Transcript states (WP6). WP13's history scenes are appended by the lead.
    @MainActor static var transcript: [DesktopPreviewScene] {
        typealias P = TranscriptPreviewData
        return [
            window("transcript-working", seed: P.working),
            window("transcript-finished", seed: P.finished),
            window("transcript-permission", seed: P.permission),
            DesktopPreviewScene(id: "transcript-permission-card", size: CGSize(width: 800, height: 560)) {
                let session = DesktopSession.preview(configure: P.permissionCard)
                let row = session.transcriptRows(showsTechnical: false).lazy.compactMap { row -> PermissionRow? in
                    if case let .permission(permission) = row, permission.presentation == .pending { return permission }
                    return nil
                }.first
                return AnyView(P.frame {
                    if let row { ApprovalCardView(row: row, session: session, showsDetailsInitially: true) }
                })
            },
            window("transcript-review-denied", seed: P.reviewDenied),
            window("transcript-stopped", seed: P.stopped),
            window("transcript-could-not-load", seed: P.couldNotLoad),
            DesktopPreviewScene(id: "transcript-technical") {
                AnyView(ContentView(session: .preview(configure: P.technical))
                    .defaultAppStorage(P.defaults("technical", ["jet.transcript.show-technical": true])))
            },
            DesktopPreviewScene(id: "transcript-large-text", size: CGSize(width: 1000, height: 760)) {
                AnyView(ContentView(session: .preview(configure: P.finished))
                    .defaultAppStorage(P.defaults("large-text", [JetDesign.transcriptScaleKey: 2.0])))
            },
            DesktopPreviewScene(id: "transcript-jump", size: CGSize(width: 900, height: 600)) {
                AnyView(TranscriptView(session: .preview(configure: P.longTask), startsAtTop: true)
                    .background(.background)
                    .tint(JetDesign.accent))
            },
            DesktopPreviewScene(id: "transcript-steps", size: CGSize(width: 760, height: 360)) {
                let session = DesktopSession.preview(configure: DesktopPreviewData.working)
                return AnyView(P.frame {
                    ForEach(P.stepRows) { row in
                        TranscriptRowView(row: row, session: session, expandedSteps: .constant(["steps-done"]))
                    }
                })
            },
            DesktopPreviewScene(id: "transcript-rename", size: CGSize(width: 420, height: 280)) {
                let session = DesktopSession.preview(configure: DesktopPreviewData.waitingWithChanges)
                return AnyView(RenameTaskSheet(session: session, initialName: "Fix the sign-in redirect loop")
                    .frame(width: 420, height: 280, alignment: .top)
                    .background(.background)
                    .tint(JetDesign.accent))
            },
            DesktopPreviewScene(id: "transcript-rename-conflict", size: CGSize(width: 420, height: 280)) {
                let session = DesktopSession.preview(configure: DesktopPreviewData.waitingWithChanges)
                return AnyView(RenameTaskSheet(session: session, capturedRevision: 2, initialName: "Fix the sign-in redirect")
                    .frame(width: 420, height: 280, alignment: .top)
                    .background(.background)
                    .tint(JetDesign.accent))
            },
        ]
    }
}

/// Transcripts built from synthetic Events, projected and merged exactly as live
/// Events are.
@MainActor
enum TranscriptPreviewData {
    static let firstTurn = turn(1)
    static let followUpTurn = turn(2)

    // MARK: - Scenes

    /// Working · Editing files, with a waiting follow-up.
    static func working(_ session: DesktopSession) {
        DesktopPreviewData.connect(session)
        let task = DesktopPreviewData.loginRedirect
        let run = DesktopPreviewData.makeRun(for: task, lifecycle: .active)
        var script = Script()
        script.you(firstTurn, "After signing in, the app sends me back to /login instead of the page I was on. Can you find the cause and fix it?", at: 14)
        script.tools("Read", "Grep", "Read")
        script.reply(richReply)
        script.you(followUpTurn, "Also add a test for an expired session.", at: 4, delivered: false)
        script.text("The guard reads the session before the cookie refresh finishes, so I'll move the check after it.")
        script.tools("Edit")
        DesktopPreviewData.open(
            session, task, run: run,
            execution: DesktopPreviewData.makeExecution(run, activity: .working),
            timeline: timeline(script.events),
            queue: JetTurnQueue(cursor: DesktopPreviewData.headCursor, turns: [
                queueEntry(firstTurn, position: 1, state: .active, run: run.id),
                queueEntry(followUpTurn, position: 2, state: .queued, run: run.id, withdrawable: true),
            ])
        )
    }

    /// Waiting for your reply after two replies that changed files.
    static func finished(_ session: DesktopSession) {
        DesktopPreviewData.waitingWithChanges(session)
        replace(session, with: finishedScript().events)
    }

    static func finishedScript() -> Script {
        var script = Script()
        script.you(firstTurn, "After signing in, the app sends me back to /login instead of the page I was on. Can you find the cause and fix it?", at: 20)
        script.tools("Read", "Grep")
        script.reply("The loop comes from `redirectTo` in **src/auth/guard.ts**: after sign-in it still points at `/login`. I'll remember the page you came from and return there.")
        script.tools("Edit", "Edit", "Bash")
        script.reply("Fixed. `guard.ts` now remembers where you were going, and all **42 tests** pass.")
        script.changes(turn: 1)
        script.you(followUpTurn, "Also add a test for an expired session.", at: 6)
        script.tools("Write", "Bash")
        script.reply("Added `guard.test.ts` with a case for an expired session. The suite passes.")
        script.changes(turn: 2)
        return script
    }

    /// Allowed and blocked requests, then the one the reply waits for.
    static func permission(_ session: DesktopSession) {
        DesktopPreviewData.needsPermission(session)
        var script = Script(run: DesktopPreviewData.runID(2))
        script.you(firstTurn, "Update the Stripe SDK and the other payment dependencies to their latest minor versions, then run the tests.", at: 9)
        script.tools("Read")
        script.requested("req-0", command: "npm view stripe version")
        script.reviewed("req-0", command: "npm view stripe version", outcome: ["status": "decided", "decision": "allow"])
        script.reply("Stripe 17.4.0 is the newest minor release. I'll update it together with `@stripe/stripe-js`.")
        script.requested("req-1", command: "rm -rf node_modules package-lock.json")
        script.reviewed("req-1", command: "rm -rf node_modules package-lock.json", outcome: ["status": "denied", "reason": "Deleting the lockfile isn't needed for a minor update."])
        script.requested("req-2", command: "npm install stripe@17.4.0 @stripe/stripe-js@5.2.0")
        replace(session, with: script.events)
    }

    /// The pending card with every detail row, including the reviewer's reason.
    static func permissionCard(_ session: DesktopSession) {
        DesktopPreviewData.needsPermission(session)
        var script = Script(run: DesktopPreviewData.runID(2))
        script.you(firstTurn, "Update the payment dependencies.", at: 9)
        script.requested("req-2", command: "npm install stripe@17.4.0 @stripe/stripe-js@5.2.0")
        script.reviewed("req-2", command: "npm install stripe@17.4.0 @stripe/stripe-js@5.2.0", outcome: ["status": "unavailable", "reason": "The safety reviewer couldn't be reached before the time limit."])
        replace(session, with: script.events)
    }

    /// A blocked request the person can send to the reviewer once more.
    static func reviewDenied(_ session: DesktopSession) {
        DesktopPreviewData.needsPermission(session)
        var script = Script(run: DesktopPreviewData.runID(2))
        script.you(firstTurn, "Publish the updated package.", at: 9)
        script.tools("Read", "Bash")
        script.requested("req-3", command: ["sh", "-c", "npm publish --access public"])
        script.reviewed("req-3", command: ["sh", "-c", "npm publish --access public"], outcome: ["status": "denied", "reason": "Publishing a package leaves this Mac."], reviewID: UUID())
        replace(session, with: script.events)
    }

    /// Every way a reply can end; the canceled line after Stop is folded away.
    static func stopped(_ session: DesktopSession) {
        DesktopPreviewData.connect(session)
        let task = DesktopPreviewData.conversations[3]
        let runs = (11 ... 14).map(DesktopPreviewData.runID)
        var script = Script(run: runs[0])
        script.you(turn(11), "Rename the `User` model to `Account` everywhere.", at: 50)
        script.tools("Grep")
        script.terminated("interrupt_turn", "native_cancellation")
        script.you(turn(12), "Only rename it in the API layer.", at: 45)
        script.tools("Edit")
        script.terminated("stop_run", "terminate")
        script.lifecycle("canceled")
        script.run = runs[1]
        script.you(turn(13), "Continue with the API layer.", at: 30)
        script.tools("Edit")
        script.terminated("stop_run", "unobserved")
        script.run = runs[2]
        script.you(turn(14), "Pick up where you left off.", at: 20)
        script.tools("Bash")
        script.lifecycle("lost")
        script.run = runs[3]
        script.you(turn(15), "Run the tests after the rename.", at: 10)
        script.tools("Bash")
        script.lifecycle("failed")
        let run = DesktopPreviewData.makeRun(for: task, index: 14, lifecycle: .failed, startedAgo: 10 * DesktopPreviewData.minute)
        DesktopPreviewData.open(
            session, task, run: run,
            execution: DesktopPreviewData.makeExecution(run, activity: nil),
            timeline: timeline(script.events),
            queue: JetTurnQueue(cursor: DesktopPreviewData.headCursor, turns: [])
        )
    }

    /// The task's snapshot couldn't be read and nothing is cached.
    static func couldNotLoad(_ session: DesktopSession) {
        DesktopPreviewData.connect(session)
        session.sidebarSelection = .conversation
        session.selectedConversationID = DesktopPreviewData.conversations[4].id
        session.conversationSnapshot = nil
        session.timeline = []
        session.conversationFreshness = .failed
    }

    /// Show Technical Activity: lifecycle, activity and background lines appear.
    static func technical(_ session: DesktopSession) {
        DesktopPreviewData.waitingWithChanges(session)
        var script = Script()
        script.lifecycle("starting")
        script.lifecycle("active")
        script.you(firstTurn, "Why does the dashboard query take four seconds?", at: 15)
        script.activity("working")
        script.tools("Read", "Grep")
        script.add("run.processes_changed", [:])
        script.add("run.processes_changed", [:])
        script.reply("The query loads every invoice before filtering. An index on `created_at` and a date filter bring it under 200 ms.")
        script.noChanges(turn: 1)
        script.activity("waiting_for_user")
        replace(session, with: script.events)
    }

    /// Twelve exchanges, for the Jump to Latest button.
    static func longTask(_ session: DesktopSession) {
        DesktopPreviewData.waitingWithChanges(session)
        var script = Script()
        for index in 1 ... 12 {
            script.you(turn(100 + index), "Step \(index): tighten the next part of the checkout flow.", at: Int64(60 - index * 4))
            script.tools("Read", "Edit")
            script.reply("Done with step \(index). The checkout form now validates its fields before it submits, and the tests pass.")
        }
        replace(session, with: script.events)
    }

    static let stepRows: [TranscriptRow] = [
        .steps(StepsRow(
            id: "steps-live",
            items: [.reasoning("The guard reads the session before the cookie refresh finishes."), .tool("Read"), .tool("Edit")],
            recordedAt: DesktopPreviewData.now - 2 * DesktopPreviewData.minute,
            phase: .editingFiles,
            isLive: true
        )),
        .steps(StepsRow(
            id: "steps-done",
            items: [
                .tool("Read"), .tool("Grep"), .tool("Read"),
                .reasoning("Both redirects share one helper, so a single change covers sign-in and sign-up. I'll add a test for each and run the whole suite to make sure nothing else depends on the old constant."),
                .tool("Edit"), .tool("Bash"), .tool("WebFetch"),
            ],
            recordedAt: DesktopPreviewData.now - 9 * DesktopPreviewData.minute,
            showsLabel: false
        )),
    ]

    // MARK: - Helpers

    static let richReply = """
    The loop comes from `redirectTo` in **src/auth/guard.ts**. After sign-in it still points at `/login`, so the guard sends you straight back.

    ## Plan

    1. Remember the page you came from before redirecting to sign-in
    2. Return there after sign-in, falling back to `/dashboard`
    3. Add a regression test

    - Files to change:
      - `src/auth/guard.ts`
      - `src/auth/login.ts`
    - [x] Find the cause
    - [ ] Fix the redirect
    - [ ] Add a test

    ```ts
    export function redirectAfterSignIn(session: Session) { return redirect(session.returnTo ?? "/dashboard") } // falls back when the original page isn't known
    ```
    """

    /// Projects and merges Events oldest first, as `DesktopSession.receive` does.
    static func timeline(_ events: [JetEvent]) -> [JetTimelineEntry] {
        var entries: [JetTimelineEntry] = []
        for event in events {
            let projections = event.timelineProjections()
            if projections.isEmpty {
                TranscriptStore.groupRaw(sequence: event.sequence, into: &entries)
            } else {
                for projection in projections { TranscriptStore.merge(projection, into: &entries) }
            }
        }
        return entries
    }

    static func replace(_ session: DesktopSession, with events: [JetEvent]) {
        guard let id = session.selectedConversationID else { return }
        session.timeline = timeline(events)
        session.transcripts.save(session.timeline, for: id)
    }

    /// A defaults suite of its own, so settings don't leak between screenshots.
    static func defaults(_ name: String, _ values: [String: Any]) -> UserDefaults {
        let suiteName = "jet.preview.transcript-\(name)"
        let defaults = UserDefaults(suiteName: suiteName) ?? .standard
        defaults.removePersistentDomain(forName: suiteName)
        for (key, value) in values { defaults.set(value, forKey: key) }
        return defaults
    }

    static func frame<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 12) { content() }
            .padding(24)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(.background)
            .tint(JetDesign.accent)
    }

    static func turn(_ index: Int) -> UUID {
        UUID(uuidString: String(format: "7A3C0000-0000-4000-8000-%012ld", 700 + index)) ?? UUID()
    }

    static func queueEntry(
        _ id: UUID,
        position: Int,
        state: JetTurnState,
        run: UUID?,
        withdrawable: Bool = false
    ) -> JetTurnQueueEntry {
        JetTurnQueueEntry(id: id, sequence: 0, position: position, source: .user, state: state, runID: run, withdrawable: withdrawable)
    }

    /// Writes Events oldest first, one sequence apart.
    struct Script {
        var run: UUID?
        private(set) var events: [JetEvent] = []
        private var sequence: UInt64 = 300
        private var minutesAgo: Int64 = 30

        /// Defaults to `DesktopPreviewData.runID(1)`, the working task's Run.
        init(run: UUID? = UUID(uuidString: "7A3C0000-0000-4000-8000-000000000501")) {
            self.run = run
        }

        mutating func add(_ kind: String, _ payload: [String: Any]) {
            sequence += 1
            events.append(JetEvent(
                sequence: sequence,
                eventID: UUID(),
                actor: JetRawJSON(source: #"{"interactive_client":{}}"#),
                origin: nil,
                recordedAtUnixMilliseconds: DesktopPreviewData.now - minutesAgo * DesktopPreviewData.minute,
                conversationID: nil,
                runID: run,
                kind: kind,
                payloadVersion: 1,
                payload: JetRawJSON(source: Self.json(payload))
            ))
        }

        mutating func you(_ turn: UUID, _ text: String, at minutes: Int64, delivered: Bool = true) {
            minutesAgo = minutes
            add("turn.input", ["turn_id": turn.uuidString, "text": text])
            if delivered { turnChanged(turn, "active") }
        }

        mutating func turnChanged(_ turn: UUID, _ state: String) {
            add("turn.changed", ["turn": ["turn_id": turn.uuidString, "sequence": 1, "source": "user", "state": state]])
        }

        mutating func tools(_ names: String...) {
            for name in names { text(name) }
        }

        mutating func text(_ value: String) {
            output(kind: "text", value)
        }

        mutating func reply(_ markdown: String) {
            minutesAgo = max(0, minutesAgo - 1)
            output(kind: "markdown", markdown)
        }

        mutating func changes(turn: Int) {
            add("change.checkpoint_recorded", ["turn": turn, "outcome": "completed", "artifact": ["availability": "stored", "sha256": String(repeating: "5d", count: 32), "size": 2_048]])
        }

        mutating func noChanges(turn: Int) {
            add("change.checkpoint_recorded", ["turn": turn, "outcome": "completed", "artifact": ["availability": "stored", "sha256": String(repeating: "0a", count: 32), "size": 0]])
        }

        mutating func lifecycle(_ to: String) {
            add("run.lifecycle_changed", ["from": "active", "to": to])
        }

        mutating func activity(_ value: String) {
            add("run.activity_changed", ["activity": value])
        }

        mutating func terminated(_ control: String, _ stage: String) {
            add("run.terminated", ["termination": ["control": control, "stage": stage]])
        }

        mutating func requested(_ id: String, command: Any) {
            add("approval.requested", ["request": request(id, command: command)])
        }

        mutating func reviewed(_ id: String, command: Any, outcome: [String: Any], reviewID: UUID = UUID()) {
            add("approval.reviewed", ["review": [
                "review_id": reviewID.uuidString,
                "request": request(id, command: command),
                "outcome": outcome,
            ]])
        }

        private func request(_ id: String, command: Any) -> [String: Any] {
            ["request_id": id, "tool": "Bash", "action": Self.json(["command": command, "cwd": "/Users/alex/code/billing-service"])]
        }

        private mutating func output(kind: String, _ text: String) {
            add("run.output", ["native_json": "{}", "presentation_json": [Self.json(["kind": kind, "text": text])]])
        }

        private static func json(_ object: [String: Any]) -> String {
            guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]) else { return "{}" }
            return String(decoding: data, as: UTF8.self)
        }
    }
}
#endif
