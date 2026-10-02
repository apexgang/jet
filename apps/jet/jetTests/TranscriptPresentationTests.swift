import Foundation
import Testing
@testable import jet

@MainActor
struct TranscriptPresentationTests {
    private static let run = UUID(uuidString: "7A3C0000-0000-4000-8000-0000000000A1")!
    private static let otherRun = UUID(uuidString: "7A3C0000-0000-4000-8000-0000000000A2")!
    private static let turnA = UUID(uuidString: "7A3C0000-0000-4000-8000-0000000000B1")!
    private static let turnB = UUID(uuidString: "7A3C0000-0000-4000-8000-0000000000B2")!

    // MARK: - Projection

    @Test
    func userInputProjectsOneCompleteEntry() {
        let entries = Self.event(7, "turn.input", #"{"turn_id":"\#(Self.turnA.uuidString)","text":"Fix it"}"#)
            .timelineProjections()
        #expect(entries == [
            JetTimelineEntry(
                id: Self.turnA.uuidString.lowercased(), kind: .user, text: "Fix it", sequence: 7, rawCount: 0,
                recordedAtUnixMilliseconds: 1_000, runID: Self.run
            ),
        ])
        #expect(entries.map(TranscriptEntryRole.of) == [.message])
    }

    @Test
    func outputBlocksKeepTheirRoleInTheID() {
        let blocks = [
            #"{\"kind\":\"markdown\",\"text\":\"**Done**\"}"#,
            #"{\"kind\":\"text\",\"text\":\"Read\"}"#,
            #"{\"kind\":\"actions\",\"actions\":[{}]}"#,
            #"{\"kind\":\"future\",\"private\":\"hidden\"}"#,
        ]
        let entries = Self.event(9, "run.output", #"{"presentation_json":["\#(blocks.joined(separator: "\",\""))"]}"#)
            .timelineProjections()
        #expect(entries.map(\.id) == ["9-output-0", "9-text-1", "9-actions-2", "9-unknown-3"])
        #expect(entries.map(TranscriptEntryRole.of) == [.markdown, .text, .technical, .technical])
        #expect(entries.map(\.isTechnical) == [false, false, true, true])
        #expect(entries.allSatisfy { $0.runID == Self.run && $0.recordedAtUnixMilliseconds == 1_000 })
        #expect(!entries.map(\.text).joined().contains("hidden"))
    }

    @Test
    func lifecycleAndTerminationAddStatusRows() {
        func project(_ kind: String, _ payload: String) -> [String] {
            Self.event(11, kind, payload).timelineProjections().map(\.id)
        }
        #expect(project("run.lifecycle_changed", #"{"from":"active","to":"failed"}"#) == ["11-lifecycle", "11-status-failed"])
        #expect(project("run.lifecycle_changed", #"{"from":"active","to":"lost"}"#) == ["11-lifecycle", "11-status-lost"])
        #expect(project("run.lifecycle_changed", #"{"from":"stopping","to":"canceled"}"#) == ["11-lifecycle", "11-status-canceled"])
        #expect(project("run.lifecycle_changed", #"{"from":"active","to":"completed"}"#) == ["11-lifecycle"])
        #expect(project("run.terminated", #"{"termination":{"control":"interrupt_turn","stage":"native_cancellation"}}"#)
            == ["11-termination", "11-status-interrupted"])
        #expect(project("run.terminated", #"{"termination":{"control":"stop_run","stage":"unobserved"}}"#)
            == ["11-termination", "11-status-stop-unconfirmed"])
        #expect(project("run.terminated", #"{"termination":{"control":"stop_run","stage":"kill"}}"#)
            == ["11-termination", "11-status-stopped"])
        #expect(project("run.terminated", #"{"termination":{}}"#) == ["11-termination"])

        let failed = Self.event(11, "run.lifecycle_changed", #"{"to":"failed"}"#).timelineProjections()
        #expect(failed.map(TranscriptEntryRole.of) == [.technical, .status(.failed)])
        #expect(failed.map(\.isTechnical) == [true, false])
        #expect(failed.last?.text == "Stopped with an error.")
    }

    @Test
    func checkpointsWithChangesBecomeChangesRows() {
        let changed = Self.event(14, "change.checkpoint_recorded", #"{"turn":2,"outcome":"completed","artifact":{"sha256":"ab","size":40,"availability":"stored"}}"#)
            .timelineProjections()
        #expect(changed.map(\.id) == ["14-changes"])
        #expect(changed.first?.checkpointTurn == 2)
        #expect(changed.first?.isTechnical == false)
        #expect(changed.map(TranscriptEntryRole.of) == [.changes])

        let empty = Self.event(15, "change.checkpoint_recorded", #"{"turn":2,"artifact":{"sha256":"ab","size":0}}"#)
            .timelineProjections()
        #expect(empty.map(\.id) == ["15-checkpoint"])
        #expect(empty.first?.text == "Reply 2 recorded no file changes.")
        #expect(empty.first?.isTechnical == true)
        #expect(empty.first?.checkpointTurn == nil)
        #expect(Self.event(16, "change.checkpoint_recorded", #"{"turn":2}"#).timelineProjections().map(\.isTechnical) == [true])
    }

    @Test
    func turnChangesBecomeMarkers() {
        let id = Self.turnA.uuidString
        let active = Self.event(20, "turn.changed", #"{"turn":{"turn_id":"\#(id)","state":"active"}}"#).timelineProjections()
        let withdrawn = Self.event(21, "turn.changed", #"{"turn":{"turn_id":"\#(id)","state":"withdrawn"}}"#).timelineProjections()
        #expect(active.map(\.id) == ["turn-active-\(id.lowercased())"])
        #expect(active.map(TranscriptEntryRole.of) == [.delivered(id.lowercased())])
        #expect(withdrawn.map(TranscriptEntryRole.of) == [.withdrawn(id.lowercased())])
        #expect((active + withdrawn).allSatisfy { $0.isTechnical })
        #expect(Self.event(22, "turn.changed", #"{"turn":{"turn_id":"\#(id)","state":"completed"}}"#).timelineProjections().isEmpty)
        #expect(Self.event(23, "approval.retry_authorized", #"{"review_id":"x"}"#).timelineProjections().map(\.id) == ["23-retry"])
    }

    @Test
    func approvalsUsePlainCopy() throws {
        let requested = try #require(Self.event(30, "approval.requested", Self.request(action: #"{\"command\":\"make test\"}"#))
            .timelineProjections().first)
        #expect(requested.id == "approval-\(Self.run.uuidString.lowercased())-req-1")
        #expect(requested.text == "shell needs permission.")
        #expect(requested.approval?.target == "This task's working copy")
        #expect(requested.approval?.consequence == "This reply stays paused until the request is answered.")
        #expect(TranscriptEntryRole.of(requested) == .approval)

        let denied = try #require(Self.event(31, "approval.reviewed", Self.review(status: "denied")).timelineProjections().first)
        #expect(denied.id == requested.id)
        #expect(denied.text == "shell was blocked.")
        #expect(denied.approval?.target == "/tmp/project")
        #expect(denied.approval?.canAuthorizeRetry == true)
        #expect(denied.approval?.consequence == "The action stays blocked. You can ask the safety reviewer to check the unchanged request once more.")
    }

    @Test
    func longTextIsShortenedWithPlainCopy() throws {
        let text = String(repeating: "a", count: 9_000)
        let entry = try #require(Self.event(1, "turn.input", #"{"turn_id":"\#(Self.turnA.uuidString)","text":"\#(text)"}"#)
            .timelineProjections().first)
        #expect(entry.text.hasSuffix("\n\n[Jet shortened this output.]"))
        #expect(entry.text.utf8.count < 8_300)
    }

    @Test
    func casualTextNeverUsesJetWords() {
        let events = [
            Self.event(1, "turn.input", #"{"turn_id":"\#(Self.turnA.uuidString)","text":"Hi"}"#),
            Self.event(2, "run.lifecycle_changed", #"{"to":"failed"}"#),
            Self.event(3, "run.lifecycle_changed", #"{"to":"lost"}"#),
            Self.event(4, "run.lifecycle_changed", #"{"to":"canceled"}"#),
            Self.event(5, "run.terminated", #"{"termination":{"control":"interrupt_turn","stage":"kill"}}"#),
            Self.event(6, "run.terminated", #"{"termination":{"control":"stop_run","stage":"unobserved"}}"#),
            Self.event(7, "change.checkpoint_recorded", #"{"turn":1,"artifact":{"size":3}}"#),
            Self.event(8, "approval.requested", Self.request(action: "{}")),
            Self.event(9, "approval.reviewed", Self.review(status: "denied")),
            Self.event(10, "approval.reviewed", Self.review(status: "unavailable")),
            Self.event(11, "approval.reviewed", Self.review(status: "decided", decision: "allow")),
        ]
        var texts: [String] = []
        for entry in events.flatMap({ $0.timelineProjections() }) where !entry.isTechnical {
            texts.append(entry.text)
            if let approval = entry.approval {
                texts += [approval.consequence, approval.scope, approval.target]
            }
        }
        texts += TranscriptStatusKind.allCases.map(\.title)
        texts += [
            StepsRow(id: "s", items: [.reasoning("x")]).title,
            StepsRow(id: "s", items: [.tool("Edit")], phase: .editingFiles, isLive: true).title,
            TranscriptHistoryNotice.loading(progress: nil).title,
            TranscriptHistoryNotice.unavailable(summary: nil, canRetry: false).title,
            TranscriptHistoryNotice.unavailable(summary: nil, canRetry: true).title,
            TranscriptHistoryNotice.noMessages.title,
        ]
        for text in texts {
            #expect(JetCopy.foundAvoidWords(in: text).isEmpty, "\(text)")
        }
    }

    // MARK: - Approval display

    @Test(arguments: [
        (#"{"command":"make test"}"#, "make test"),
        (#"{"command":["sh","-c","npm publish"]}"#, "npm publish"),
        (#"{"command":["bash","-lc","git push"]}"#, "git push"),
        (#"{"command":["/bin/zsh","-c","ls -la"]}"#, "ls -la"),
        (#"{"command":["git","status","--short"]}"#, "git status --short"),
        (#"{"command":"make\n   test\t now"}"#, "make test now"),
    ])
    func approvalCommandsAreReadable(action: String, command: String) {
        #expect(ApprovalDisplay.command(from: action) == command)
    }

    @Test
    func approvalCommandsAreBoundedAndFallBackToTheTool() {
        let long = String(repeating: "x", count: 250)
        #expect(ApprovalDisplay.command(from: #"{"command":"\#(long)"}"#) == String(repeating: "x", count: 200) + "…")
        #expect(ApprovalDisplay.command(from: #"{"file_path":"/a"}"#) == nil)
        #expect(ApprovalDisplay.command(from: #"{"command":"   "}"#) == nil)
        #expect(ApprovalDisplay.command(from: "not json") == nil)
        #expect(ApprovalDisplay.subject(Self.approval(.requested, action: "{}")) == "Bash")
        #expect(ApprovalDisplay.subject(Self.approval(.requested, action: #"{"command":"ls"}"#)) == "ls")
    }

    // MARK: - Rows

    @Test
    func firstEntryPerIDWinsAndWithdrawnMessagesVanish() {
        let rows = TranscriptPresentation.rows(from: [
            Self.you(Self.turnA, "Hello", 1),
            Self.you(Self.turnA, "Duplicate", 2),
            Self.you(Self.turnB, "Never mind", 3),
            Self.marker("withdrawn", Self.turnB, 4),
        ], context: TranscriptContext())
        #expect(rows == [.you(YouRow(id: Self.id(Self.turnA), text: "Hello", recordedAt: 1, queue: nil))])
    }

    @Test
    func deliveredMessagesMoveToTheirMarkerAndWaitingOnesGoLast() {
        let waiting = JetTurnQueueEntry(id: UUID(), sequence: 9, position: 2, source: .user, state: .queued, runID: nil, withdrawable: true)
        let third = UUID()
        let entries = [
            Self.you(Self.turnA, "Fix it", 1),
            Self.marker("active", Self.turnA, 2),
            Self.text("Read", 3),
            Self.you(Self.turnB, "Also add a test", 4),
            Self.you(third, "And update the docs", 5),
            Self.markdown("Fixed.", 6),
            Self.marker("active", Self.turnB, 7),
            Self.markdown("Added the test.", 8),
        ]
        let context = TranscriptContext(queued: [Self.id(third): QueuePlacement(ordinal: 1, entry: waiting)])
        #expect(TranscriptPresentation.rows(from: entries, context: context) == [
            .you(YouRow(id: Self.id(Self.turnA), text: "Fix it", recordedAt: 1, queue: nil)),
            .steps(StepsRow(id: "steps-3-text-0", items: [.tool("Read")], recordedAt: 3, phase: .readingProject)),
            .assistant(AssistantRow(id: "6-output-0", text: "Fixed.", recordedAt: 6, showsLabel: false)),
            .you(YouRow(id: Self.id(Self.turnB), text: "Also add a test", recordedAt: 4, queue: nil)),
            .assistant(AssistantRow(id: "8-output-0", text: "Added the test.", recordedAt: 8)),
            .you(YouRow(id: Self.id(third), text: "And update the docs", recordedAt: 5, queue: QueuePlacement(ordinal: 1, entry: waiting))),
        ])
    }

    @Test
    func stepsFoldAcrossHiddenTechnicalLinesAndTheTrailingOneIsLive() {
        let entries = [
            Self.text("Read", 1),
            Self.technical("2-activity", 2),
            Self.text("I'll check the guard first.", 3),
            Self.markdown("Found it.", 4),
            Self.text("Edit", 5),
        ]
        let working = TranscriptContext(isReplyInProgress: true)
        #expect(TranscriptPresentation.rows(from: entries, context: working) == [
            .steps(StepsRow(id: "steps-1-text-0", items: [.tool("Read"), .reasoning("I'll check the guard first.")], recordedAt: 1, phase: .readingProject)),
            .assistant(AssistantRow(id: "4-output-0", text: "Found it.", recordedAt: 4, showsLabel: false)),
            .steps(StepsRow(id: "steps-5-text-0", items: [.tool("Edit")], recordedAt: 5, phase: .editingFiles, isLive: true, showsLabel: false)),
        ])
        let technical = TranscriptPresentation.rows(from: entries, context: TranscriptContext(showsTechnical: true))
        #expect(technical.map(\.id) == ["steps-1-text-0", "2-activity", "steps-3-text-0", "4-output-0", "steps-5-text-0"])
        // A visible technical line keeps the assistant's label from repeating.
        #expect(technical[2] == .steps(StepsRow(id: "steps-3-text-0", items: [.reasoning("I'll check the guard first.")], recordedAt: 3, showsLabel: false)))
    }

    @Test
    func stopsReplaceCanceledAndOnlyTheLastFailureOffersSendAgain() {
        let entries = [
            Self.you(Self.turnA, "Try this", 1),
            Self.status(.stopped, 2, run: Self.run),
            Self.status(.canceled, 3, run: Self.run),
            Self.status(.canceled, 4, run: Self.otherRun),
            Self.status(.failed, 5, run: Self.otherRun),
            Self.you(Self.turnB, "Try that", 6),
            Self.status(.failed, 7, run: Self.otherRun),
        ]
        #expect(TranscriptPresentation.rows(from: entries, context: TranscriptContext()) == [
            .you(YouRow(id: Self.id(Self.turnA), text: "Try this", recordedAt: 1, queue: nil)),
            .status(StatusRow(id: "2-status-stopped", kind: .stopped, runID: Self.run)),
            .status(StatusRow(id: "4-status-canceled", kind: .canceled, runID: Self.otherRun)),
            .status(StatusRow(id: "5-status-failed", kind: .failed, runID: Self.otherRun)),
            .you(YouRow(id: Self.id(Self.turnB), text: "Try that", recordedAt: 6, queue: nil)),
            .status(StatusRow(id: "7-status-failed", kind: .failed, runID: Self.otherRun, sendAgainText: "Try that")),
        ])
        let replying = TranscriptPresentation.rows(from: entries, context: TranscriptContext(isReplyInProgress: true))
        #expect(replying.last == .status(StatusRow(id: "7-status-failed", kind: .failed, runID: Self.otherRun)))
        let answered = TranscriptPresentation.rows(from: entries + [Self.markdown("Retrying.", 8)], context: TranscriptContext())
        #expect(answered[5] == .status(StatusRow(id: "7-status-failed", kind: .failed, runID: Self.otherRun)))

        // A waiting message is not a later message.
        let waiting = JetTurnQueueEntry(id: UUID(), sequence: 9, position: 1, source: .user, state: .queued, runID: nil, withdrawable: true)
        let queued = TranscriptPresentation.rows(
            from: entries + [Self.you(waiting.id, "Later", 9)],
            context: TranscriptContext(queued: [Self.id(waiting.id): QueuePlacement(ordinal: 1, entry: waiting)])
        )
        #expect(queued[5] == .status(StatusRow(id: "7-status-failed", kind: .failed, runID: Self.otherRun, sendAgainText: "Try that")))
    }

    @Test
    func onlyTheNewestChangesOfferKeepWhenNoReplyRuns() {
        let entries = [Self.changes(1, turn: 1), Self.markdown("More.", 2), Self.changes(3, turn: 2)]
        #expect(TranscriptPresentation.rows(from: entries, context: TranscriptContext()) == [
            .changes(ChangesRow(id: "1-changes", runID: Self.run, turn: 1)),
            .assistant(AssistantRow(id: "2-output-0", text: "More.", recordedAt: 2)),
            .changes(ChangesRow(id: "3-changes", runID: Self.run, turn: 2, offersKeep: true)),
        ])
        let running = TranscriptPresentation.rows(from: entries, context: TranscriptContext(isReplyInProgress: true))
        #expect(running.last == .changes(ChangesRow(id: "3-changes", runID: Self.run, turn: 2)))
    }

    @Test
    func onlyTheLatestRequestIsPendingOrRetryable() {
        let allowed = Self.approval(.allowed, id: "a")
        let deniedEarlier = Self.approval(.denied, id: "b", canRetry: true)
        let requested = Self.approval(.requested, id: "c", action: #"{"command":"npm install"}"#)
        let entries = [allowed, deniedEarlier, requested].enumerated().map { Self.approvalEntry($1, Int64($0 + 1)) }

        func presentations(_ entries: [JetTimelineEntry], _ context: TranscriptContext) -> [PermissionRow.Presentation] {
            TranscriptPresentation.rows(from: entries, context: context).compactMap {
                if case let .permission(row) = $0 { return row.presentation }
                return nil
            }
        }
        #expect(presentations(entries, TranscriptContext(needsPermission: true)) == [.allowed, .blocked(offersRetry: false), .pending])
        #expect(presentations(entries, TranscriptContext()) == [.allowed, .blocked(offersRetry: false), .asked])

        let deniedLast = [Self.approvalEntry(requested, 1), Self.approvalEntry(deniedEarlier, 2)]
        #expect(presentations(deniedLast, TranscriptContext(isReplyInProgress: true)) == [.asked, .blocked(offersRetry: true)])
        #expect(presentations(deniedLast, TranscriptContext(needsPermission: true)) == [.asked, .blocked(offersRetry: true)])
        #expect(presentations(deniedLast, TranscriptContext()) == [.asked, .blocked(offersRetry: false)])

        let row = TranscriptPresentation.rows(from: [Self.approvalEntry(requested, 1)], context: TranscriptContext(needsPermission: true))
        #expect(row == [.permission(PermissionRow(
            id: "approval-c", approval: requested, subject: "npm install", command: "npm install", presentation: .pending
        ))])
    }

    @Test
    func theAssistantIsNamedOncePerStretch() {
        let rows = TranscriptPresentation.rows(from: [
            Self.you(Self.turnA, "Go", 1),
            Self.text("Read", 2),
            Self.approvalEntry(Self.approval(.allowed, id: "a"), 3),
            Self.markdown("Done.", 4),
            Self.changes(5, turn: 1),
            Self.markdown("Anything else?", 6),
        ], context: TranscriptContext())
        let labels: [Bool] = rows.compactMap {
            switch $0 {
            case let .assistant(row): row.showsLabel
            case let .steps(row): row.showsLabel
            default: nil
            }
        }
        #expect(labels == [true, false, true])
    }

    @Test(arguments: [
        (true, JetConversationFreshness.failed, false, TranscriptTop.couldNotLoad),
        (true, .loading, false, .loadingTask),
        (true, .live, false, .loadingTask),
        (true, .failed, true, .none),
        (true, .live, true, .none),
        (false, .failed, false, .none),
        (false, .cached, true, .none),
    ])
    func topStates(rowsAreEmpty: Bool, freshness: JetConversationFreshness, hasSnapshot: Bool, top: TranscriptTop) {
        #expect(TranscriptPresentation.top(rowsAreEmpty: rowsAreEmpty, freshness: freshness, hasSnapshot: hasSnapshot) == top)
    }

    @Test
    func stepTitles() {
        func title(_ tools: [String], reasoning: Bool = false, live: Bool = false, phase: TaskPhase? = nil) -> String {
            let items = tools.map(TranscriptStep.tool) + (reasoning ? [.reasoning("Thinking")] : [])
            return StepsRow(id: "s", items: items, phase: phase, isLive: live).title
        }
        #expect(title(["Read"], live: true) == "Working…")
        #expect(title(["Task"], live: true, phase: .working) == "Working…")
        #expect(title(["Edit"], live: true, phase: .editingFiles) == "Working… · Editing files")
        #expect(title([], reasoning: true) == "Reasoning")
        #expect(title(["Read"], reasoning: true) == "Used 1 tool · Read")
        #expect(title(["Read", "Grep", "Read"]) == "Used 3 tools · Read, Grep")
        #expect(title(["Read", "Grep", "Read", "Edit", "Bash", "WebFetch"]) == "Used 6 tools · Read, Grep, Edit, +2")
    }

    // MARK: - Times and following

    @Test
    func timesShowTheDayOnlyWhenItDiffers() {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let locale = Locale(identifier: "en_US")
        let now = Date(timeIntervalSince1970: 1_790_433_120) // 2026-09-26 14:32 UTC
        func plain(_ value: String) -> String {
            value.replacingOccurrences(of: "\u{202F}", with: " ").replacingOccurrences(of: "\u{00A0}", with: " ")
        }
        let sameDay = plain(TranscriptFormat.time(1_790_413_500_000, now: now, calendar: calendar, locale: locale))
        #expect(sameDay == "9:05 AM")
        let yesterday = plain(TranscriptFormat.time(1_790_346_720_000, now: now, calendar: calendar, locale: locale))
        #expect(yesterday.hasPrefix("Sep 25"))
        #expect(yesterday.contains("2:32 PM"))
        #expect(!yesterday.contains("2026"))
        let lastYear = plain(TranscriptFormat.time(1_758_810_720_000, now: now, calendar: calendar, locale: locale))
        #expect(lastYear.hasPrefix("Sep 25, 2025"))
        #expect(TranscriptFormat.day(1_790_346_720_000, now: now, calendar: calendar, locale: locale) == "Sep 25")
        #expect(TranscriptFormat.day(1_758_810_720_000, now: now, calendar: calendar, locale: locale) == "Sep 25, 2025")
    }

    @Test
    func followingStopsOnlyWhenThePersonScrollsAway() {
        let bottom = ScrollSample(offsetY: 400, contentHeight: 1_000, containerHeight: 600, topInset: 0)
        let grown = ScrollSample(offsetY: 400, contentHeight: 1_100, containerHeight: 600, topInset: 0)
        let scrolledUp = ScrollSample(offsetY: 300, contentHeight: 1_000, containerHeight: 600, topInset: 0)
        #expect(bottom.isAtBottom)
        #expect(!grown.isAtBottom)
        // The inset toolbar counts: this is the bottom of a real transcript.
        #expect(ScrollSample(offsetY: 65, contentHeight: 802, containerHeight: 684, topInset: 52).isAtBottom)
        #expect(!ScrollSample(offsetY: 0, contentHeight: 802, containerHeight: 684, topInset: 52).isAtBottom)

        var follow = TranscriptFollow()
        follow.observe(previous: bottom, current: grown, userIsScrolling: false)
        #expect(follow.followsLatest)

        follow.observe(previous: grown, current: scrolledUp, userIsScrolling: true)
        #expect(!follow.followsLatest)
        follow.observe(previous: scrolledUp, current: bottom, userIsScrolling: false)
        #expect(follow.followsLatest)

        // A wheel scroll without a scroll phase still drops the offset.
        follow.observe(previous: bottom, current: scrolledUp, userIsScrolling: false)
        #expect(!follow.followsLatest)
        follow.jumpToLatest()
        #expect(follow.followsLatest)

        // A resize moves the offset without the person scrolling.
        let tall = ScrollSample(offsetY: 100, contentHeight: 2_000, containerHeight: 600, topInset: 0)
        let taller = ScrollSample(offsetY: 50, contentHeight: 2_000, containerHeight: 650, topInset: 0)
        follow.observe(previous: tall, current: taller, userIsScrolling: false)
        #expect(follow.followsLatest)
    }

    // MARK: - Rename and inline Markdown

    @Test(arguments: [
        ("", RenameValidation.empty),
        ("   ", .empty),
        ("Current", .unchanged),
        ("  Current \n", .unchanged),
        ("New name", .valid),
        (String(repeating: "a", count: 256), .valid),
        (String(repeating: "a", count: 257), .tooLong),
        (String(repeating: "é", count: 129), .tooLong),
    ])
    func renameValidation(name: String, result: RenameValidation) {
        #expect(RenameValidation.evaluate(name: name, currentTitle: "Current") == result)
    }

    @Test(arguments: ["[a](https://x)", "![i](file:///x)", "<https://x>", "**b** [c](javascript:alert(1)) `d`"])
    func inlineMarkdownIsInert(source: String) {
        let value = MessageInline.inert(source)
        #expect(value.runs.allSatisfy { $0.link == nil && $0.imageURL == nil })
        #expect(!String(value.characters).isEmpty)
    }

    @Test
    func inlineMarkdownKeepsEmphasis() {
        let value = MessageInline.inert("**bold** and [link](https://x)")
        #expect(String(value.characters) == "bold and link")
        #expect(value.runs.contains { $0.inlinePresentationIntent == .stronglyEmphasized })
    }

    // MARK: - Session

#if DEBUG
    @Test
    func sceneIDsAreUniqueAndPrefixed() {
        let ids = DesktopPreviewScenes.transcript.map(\.id)
        #expect(!ids.isEmpty)
        #expect(Set(ids).count == ids.count)
        #expect(ids.allSatisfy { $0.hasPrefix("transcript-") })
    }

    @Test
    func theWorkingTaskShowsItsWaitingMessageAndLiveSteps() {
        let session = DesktopSession.preview(configure: TranscriptPreviewData.working)
        let context = session.transcriptContext(showsTechnical: false)
        #expect(context.assistantName == "Claude Code")
        #expect(context.isReplyInProgress)
        #expect(!context.needsPermission)
        #expect(context.queued[Self.id(TranscriptPreviewData.followUpTurn)]?.ordinal == 1)

        let rows = session.transcriptRows(showsTechnical: false)
        guard case let .you(last) = rows.last, case let .steps(live) = rows[rows.count - 2] else {
            Issue.record("Expected live steps, then the waiting message: \(rows.map(\.id))")
            return
        }
        #expect(last.queue?.ordinal == 1)
        #expect(live.title == "Working… · Editing files")
        #expect(session.transcriptTop(rowsAreEmpty: rows.isEmpty) == .none)
        #expect(session.historyNotice == nil)
    }

    @Test
    func sendAgainNeverReplacesADraft() async {
        let session = DesktopSession.preview(configure: TranscriptPreviewData.stopped)
        session.draft = "My own words"
        let focus = session.composerFocusRequest
        await session.sendAgain("Run the tests")
        #expect(session.draft == "My own words")
        #expect(session.composerFocusRequest == focus + 1)

        // Offline, the text waits in the message box.
        session.draft = ""
        session.connectionState = .disconnected
        await session.sendAgain("Run the tests")
        #expect(session.draft == "Run the tests")
        #expect(session.composerFocusRequest == focus + 2)
    }

    @Test
    func aTaskWithoutRunsHasNoMessagesYet() {
        let session = DesktopSession.preview(configure: TranscriptPreviewData.finished)
        session.conversationSnapshot = JetConversationSnapshot(
            cursor: 1, conversation: DesktopPreviewData.loginRedirect, workspaceID: nil, workspaceRoot: nil, runs: []
        )
        session.timeline = []
        #expect(session.historyNotice == .noMessages)
    }
#endif

    // MARK: - Helpers

    private static func event(_ sequence: UInt64, _ kind: String, _ payload: String) -> JetEvent {
        JetEvent(
            sequence: sequence, eventID: UUID(), actor: JetRawJSON(source: "{}"), origin: nil,
            recordedAtUnixMilliseconds: 1_000, conversationID: UUID(), runID: run,
            kind: kind, payloadVersion: 1, payload: JetRawJSON(source: payload)
        )
    }

    private static func request(action: String) -> String {
        #"{"request":{"request_id":"req-1","tool":"shell","action":"\#(action)"}}"#
    }

    private static func review(status: String, decision: String? = nil) -> String {
        let decisionField = decision.map { #","decision":"\#($0)""# } ?? ""
        return #"{"review":{"review_id":"\#(UUID().uuidString)","request":{"request_id":"req-1","tool":"shell","action":"{\"cwd\":\"/tmp/project\",\"command\":\"make test\"}"},"outcome":{"status":"\#(status)"\#(decisionField),"reason":"Outside the policy."}}}"#
    }

    private static func id(_ turn: UUID) -> String { turn.uuidString.lowercased() }

    private static func entry(_ id: String, _ kind: JetTimelineKind, _ text: String, _ at: Int64) -> JetTimelineEntry {
        JetTimelineEntry(id: id, kind: kind, text: text, sequence: UInt64(at), rawCount: 0, recordedAtUnixMilliseconds: at, runID: run)
    }

    private static func you(_ turn: UUID, _ text: String, _ at: Int64) -> JetTimelineEntry {
        entry(id(turn), .user, text, at)
    }

    private static func text(_ text: String, _ at: Int64) -> JetTimelineEntry {
        entry("\(at)-text-0", .agent, text, at)
    }

    private static func markdown(_ text: String, _ at: Int64) -> JetTimelineEntry {
        entry("\(at)-output-0", .agent, text, at)
    }

    private static func technical(_ id: String, _ at: Int64) -> JetTimelineEntry {
        var value = entry(id, .activity, "Run is working.", at)
        value.isTechnical = true
        return value
    }

    private static func marker(_ state: String, _ turn: UUID, _ at: Int64) -> JetTimelineEntry {
        var value = entry("turn-\(state)-\(id(turn))", .activity, "Turn changed.", at)
        value.isTechnical = true
        return value
    }

    private static func changes(_ at: Int64, turn: UInt32) -> JetTimelineEntry {
        var value = entry("\(at)-changes", .result, "Changed files.", at)
        value.checkpointTurn = turn
        return value
    }

    private static func status(_ kind: TranscriptStatusKind, _ at: Int64, run: UUID) -> JetTimelineEntry {
        var value = entry("\(at)-status-\(kind.rawValue)", .result, kind.title, at)
        value.runID = run
        return value
    }

    private static func approval(
        _ state: JetApprovalState,
        id: String = "req",
        action: String = "{}",
        canRetry: Bool = false
    ) -> JetApprovalPresentation {
        JetApprovalPresentation(
            requestID: id, reviewID: canRetry ? UUID() : nil, runID: run, tool: "Bash", action: action,
            target: "This task's working copy", scope: "This action once", consequence: "", rationale: nil,
            state: state, canAuthorizeRetry: canRetry
        )
    }

    private static func approvalEntry(_ approval: JetApprovalPresentation, _ at: Int64) -> JetTimelineEntry {
        var value = entry("approval-\(approval.requestID)", .approval, "Bash needs permission.", at)
        value.approval = approval
        return value
    }
}
