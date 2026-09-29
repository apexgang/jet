import Foundation
import Testing
@testable import jet

@MainActor
struct TranscriptStoreTests {
    @Test
    func theLeastRecentlyUsedTaskLeavesAtThirtyThree() {
        let store = TranscriptStore()
        let ids = (0 ..< TranscriptStore.conversationCapacity).map { _ in UUID() }
        for id in ids {
            store.save([entry("m", .agent, "Hello")], for: id)
        }
        #expect(store.cachedConversationIDs.count == 32)

        // Using the oldest task again protects it; the next oldest leaves instead.
        store.save([entry("m", .agent, "Again")], for: ids[0])
        let newcomer = UUID()
        store.ingest(projections: [entry("n", .agent, "New")], rawSequence: 1, conversationID: newcomer)

        #expect(store.cachedConversationIDs.count == 32)
        #expect(store.entries(for: ids[0]).map(\.text) == ["Again"])
        #expect(store.entries(for: ids[1]).isEmpty)
        #expect(store.entries(for: newcomer).map(\.text) == ["New"])
    }

    @Test
    func eachTranscriptKeepsTheNewest256Entries() {
        let store = TranscriptStore()
        let id = UUID()
        store.markObservedStart(id)
        #expect(store.historyState(for: id) == .complete)

        for index in 0 ..< 300 {
            store.ingest(
                projections: [entry("e\(index)", .agent, "Entry \(index)", sequence: UInt64(index))],
                rawSequence: UInt64(index),
                conversationID: id
            )
        }

        let entries = store.entries(for: id)
        #expect(entries.count == 256)
        #expect(entries.first?.text == "Entry 44")
        #expect(entries.last?.text == "Entry 299")
        // The whole history was seen, but its oldest entries no longer fit.
        #expect(store.historyState(for: id) == .unavailable)

        let saved = UUID()
        store.save((0 ..< 260).map { entry("s\($0)", .agent, "\($0)") }, for: saved)
        #expect(store.entries(for: saved).count == 256)
        #expect(store.entries(for: saved).first?.text == "4")
    }

    @Test
    func mergeRulesMatchTheTranscriptBehaviour() {
        var timeline: [JetTimelineEntry] = []

        TranscriptStore.merge(entry("turn", .user, "Fix the "), into: &timeline)
        TranscriptStore.merge(entry("turn", .user, "redirect", sequence: 2), into: &timeline)
        #expect(timeline.count == 1)
        #expect(timeline[0].text == "Fix the redirect")
        #expect(timeline[0].sequence == 2)

        let requested = approval(.requested)
        TranscriptStore.merge(requested, into: &timeline)
        TranscriptStore.merge(approval(.denied), into: &timeline)
        #expect(timeline.count == 2)
        #expect(timeline[1].approval?.state == .denied)

        TranscriptStore.merge(entry("a1", .agent, "Done"), into: &timeline)
        TranscriptStore.merge(entry("a1", .agent, "Done"), into: &timeline)
        #expect(timeline.count == 4)

        TranscriptStore.groupRaw(sequence: 10, into: &timeline)
        TranscriptStore.groupRaw(sequence: 11, into: &timeline)
        #expect(timeline.count == 5)
        #expect(timeline[4].id == "raw-10")
        #expect(timeline[4].text == "2 background updates")
        #expect(timeline[4].rawCount == 2)
        #expect(timeline[4].sequence == 11)
        #expect(timeline[4].isTechnical)

        TranscriptStore.merge(entry("a2", .agent, "Next"), into: &timeline)
        TranscriptStore.groupRaw(sequence: 12, into: &timeline)
        #expect(timeline.last?.text == "1 background update")
        #expect(timeline.last?.id == "raw-12")
    }

    @Test
    func mergeAndGroupRawTakeALimit() {
        var timeline = (0 ..< 256).map { entry("e\($0)", .agent, "\($0)") }
        #expect(TranscriptStore.merge(entry("x", .agent, "x"), into: &timeline, limit: .max) == false)
        #expect(TranscriptStore.groupRaw(sequence: 9, into: &timeline, limit: .max) == false)
        #expect(timeline.count == 258)
        #expect(TranscriptStore.merge(entry("y", .agent, "y"), into: &timeline))
        #expect(timeline.count == 256)
    }

    @Test
    func tasksDoNotShareTranscripts() {
        let store = TranscriptStore()
        let first = UUID()
        let second = UUID()

        store.ingest(projections: [entry("a", .agent, "First only")], rawSequence: 1, conversationID: first)
        store.ingest(projections: [], rawSequence: 2, conversationID: second)

        #expect(store.entries(for: first).map(\.text) == ["First only"])
        #expect(store.entries(for: second).map(\.text) == ["1 background update"])

        store.remove(first)
        #expect(store.entries(for: first).isEmpty)
        #expect(store.entries(for: second).count == 1)

        store.reset(conversationIDs: [second])
        #expect(store.cachedConversationIDs.isEmpty)
    }

    @Test
    func snapshotsDecideWhetherHistoryIsComplete() {
        let store = TranscriptStore()
        let observed = UUID()
        let joinedLate = UUID()
        let fresh = UUID()
        store.markObservedStart(observed)

        store.didLoad(snapshot: snapshot(observed, runs: 1), planeRegistryID: UUID(), headCursor: 5)
        store.didLoad(snapshot: snapshot(joinedLate, runs: 1), planeRegistryID: UUID(), headCursor: 5)
        store.didLoad(snapshot: snapshot(fresh, runs: 0), planeRegistryID: UUID(), headCursor: 5)

        #expect(store.historyState(for: observed) == .complete)
        #expect(store.historyState(for: joinedLate) == .partial)
        #expect(store.historyState(for: fresh) == .notStarted)

        store.remove(observed)
        #expect(store.historyState(for: observed) == .partial)

        // A task followed from before its first Run keeps its whole history.
        store.didLoad(snapshot: snapshot(fresh, runs: 1), planeRegistryID: UUID(), headCursor: 6)
        #expect(store.historyState(for: fresh) == .complete)
    }

    // MARK: - Replay

    @Test
    func theReplayStopsAtTheTasksCreation() async {
        let journal = FakeJournal()
        let store = makeStore(journal)
        let task = UUID()
        let other = UUID()
        for sequence in UInt64(1) ... 50 {
            if sequence == 25 {
                journal.events.append(created(sequence, task))
            } else if sequence > 25, sequence.isMultiple(of: 2) {
                journal.events.append(output(sequence, task, "Ours \(sequence)"))
            } else {
                journal.events.append(output(sequence, other, "Theirs \(sequence)"))
            }
        }
        journal.head = 50

        store.didLoad(snapshot: snapshot(task, cursor: 50), planeRegistryID: Self.plane, headCursor: 40)
        #expect(store.historyState(for: task) == .loading(progress: 0))
        await store.waitForReplay()

        #expect(store.historyState(for: task) == .complete)
        let record = store.historyRecord(for: task)
        #expect(record?.phase == .finished(nil))
        #expect(record?.floor == 10)
        #expect(record?.fence == 50)
        let sequences = record?.entries.compactMap(\.sequence) ?? []
        #expect(sequences == [25] + Array(stride(from: UInt64(26), through: 50, by: 2)))
        #expect(record?.entries.dropFirst().allSatisfy { $0.text.hasPrefix("Ours") } == true)
        #expect(record?.entries.first?.isTechnical == true)
        // Windows (30, 50] and (10, 30]; nothing below the window that held the start.
        #expect(journal.afters == [30, 10])

        let presentation = store.presentation(for: task, live: [])
        #expect(presentation.entries == record?.entries)
        #expect(presentation.notice == nil)
    }

    @Test
    func turnInputSegmentsAcrossAWindowEdgeFormOneMessage() async {
        let journal = FakeJournal()
        let store = makeStore(journal)
        let task = UUID()
        let turn = UUID()
        journal.events = [
            created(5, task),
            input(19, task, turn: turn, "Fix the "),
            input(20, task, turn: turn, "login "),
            input(21, task, turn: turn, "redirect"),
            output(30, task, "On it"),
        ]
        journal.head = 40

        store.didLoad(snapshot: snapshot(task, cursor: 40), planeRegistryID: Self.plane, headCursor: 40)
        await store.waitForReplay()

        let entries = store.historyRecord(for: task)?.entries ?? []
        let messages = entries.filter { $0.kind == .user }
        #expect(messages.count == 1)
        #expect(messages.first?.text == "Fix the login redirect")
        #expect(messages.first?.sequence == 21)
        #expect(entries.last?.text == "On it")
        // The first window ends on an empty page after its last Event.
        #expect(journal.afters == [20, 30, 0])
    }

    @Test
    func theWindowBudgetStopsSixtySequencesBack() async {
        let journal = FakeJournal()
        let store = makeStore(journal)
        let task = UUID()
        journal.events = (1 ... 20).map { output(UInt64($0) * 5, task, "Step \($0 * 5)") }
        journal.head = 100

        store.didLoad(snapshot: snapshot(task, cursor: 100), planeRegistryID: Self.plane, headCursor: 100)
        await store.waitForReplay()

        let record = store.historyRecord(for: task)
        #expect(record?.phase == .finished(.beyondLimit))
        #expect(record?.floor == 40)
        #expect(record?.entries.compactMap(\.sequence) == [45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95, 100])
        #expect(journal.afters == [80, 60, 40])
        #expect(store.historyState(for: task) == .unavailable)
    }

    @Test
    func theByteBudgetDropsTheUnfinishedWindow() async {
        let journal = FakeJournal()
        journal.pageEvents = 5
        let task = UUID()
        journal.events = (1 ... 100).map { output(UInt64($0), task, "Line \($0)") }
        journal.head = 100
        let firstWindow = journal.events.filter { $0.sequence > 80 }.reduce(0) { $0 + Self.cost($1) }
        var budget = Self.small
        budget.maximumBytes = firstWindow + 1
        let store = makeStore(journal, budget: budget)

        store.didLoad(snapshot: snapshot(task, cursor: 100), planeRegistryID: Self.plane, headCursor: 100)
        await store.waitForReplay()

        let record = store.historyRecord(for: task)
        #expect(record?.phase == .finished(.beyondLimit))
        #expect(record?.floor == 80)
        #expect(record?.entries.compactMap(\.sequence) == Array(81 ... 100))
        // Four pages of the first window, then one page of the second before the check.
        #expect(journal.afters == [80, 85, 90, 95, 60])
    }

    @Test
    func theTimeBudgetDropsTheUnfinishedWindow() async {
        let journal = FakeJournal()
        journal.pageEvents = 5
        let clock = TranscriptManualClock()
        journal.onFetch = { _ in clock.now += .seconds(1) }
        let task = UUID()
        journal.events = (1 ... 100).map { output(UInt64($0), task, "Line \($0)") }
        journal.head = 100
        var budget = Self.small
        budget.maximumDuration = .seconds(5)
        let store = makeStore(journal, budget: budget, clock: clock)

        store.didLoad(snapshot: snapshot(task, cursor: 100), planeRegistryID: Self.plane, headCursor: 100)
        await store.waitForReplay()

        let record = store.historyRecord(for: task)
        #expect(record?.phase == .finished(.beyondLimit))
        #expect(record?.floor == 80)
        #expect(record?.entries.compactMap(\.sequence) == Array(81 ... 100))
        #expect(journal.afters == [80, 85, 90, 95, 60])
    }

    @Test
    func anExpiredCursorClampsToTheOldestAvailableEvent() async {
        let journal = FakeJournal()
        var budget = Self.small
        budget.windows = 5
        let store = makeStore(journal, budget: budget)
        let task = UUID()
        journal.events = (51 ... 100).map { output(UInt64($0), task, "Line \($0)") }
        journal.head = 100
        journal.minimumAvailable = 50

        store.didLoad(snapshot: snapshot(task, cursor: 100), planeRegistryID: Self.plane, headCursor: 100)
        await store.waitForReplay()

        let record = store.historyRecord(for: task)
        #expect(record?.phase == .finished(.journalTrimmed))
        #expect(record?.floor == 50)
        #expect(record?.entries.compactMap(\.sequence) == Array(51 ... 100))
        #expect(journal.afters == [80, 60, 40, 50])
        #expect(store.historyState(for: task) == .unavailable)
        #expect(store.presentation(for: task, live: []).notice?.title == "Earlier messages aren't available on this Mac.")
    }

    @Test
    func aShortJournalCompletesWithoutACreationEvent() async {
        let journal = FakeJournal()
        let store = makeStore(journal)
        let task = UUID()
        journal.events = [output(3, task, "a"), output(7, task, "b"), output(12, task, "c")]
        journal.head = 15

        // The live stream is ahead of the snapshot, so the fence follows it.
        store.didLoad(snapshot: snapshot(task, cursor: 9), planeRegistryID: Self.plane, headCursor: 15)
        await store.waitForReplay()

        let record = store.historyRecord(for: task)
        #expect(record?.phase == .finished(nil))
        #expect(record?.floor == 0)
        #expect(record?.fence == 15)
        #expect(record?.entries.map(\.text) == ["a", "b", "c"])
        #expect(journal.afters == [0, 12])
    }

    @Test
    func threeHundredOutputsKeepTheNewest256() async {
        let journal = FakeJournal()
        var budget = Self.small
        budget.windowSpan = 400
        let store = makeStore(journal, budget: budget)
        let task = UUID()
        journal.events = (1 ... 300).map { output(UInt64($0), task, "Line \($0)") }
        journal.head = 300

        store.didLoad(snapshot: snapshot(task, cursor: 300), planeRegistryID: Self.plane, headCursor: 300)
        await store.waitForReplay()

        let record = store.historyRecord(for: task)
        #expect(record?.phase == .finished(.truncated))
        #expect(record?.entries.count == 256)
        #expect(record?.entries.first?.sequence == 45)
        #expect(record?.entries.last?.sequence == 300)
        // Two pages: 256 Events, then the rest.
        #expect(journal.afters == [0, 256])
    }

    @Test
    func aMessageThatMayBeCutOffIsDroppedOnlyWhenTheReplayFallsShort() async {
        let turn = UUID()
        let task = UUID()

        let shortJournal = FakeJournal()
        var oneWindow = Self.small
        oneWindow.windows = 1
        let short = makeStore(shortJournal, budget: oneWindow)
        shortJournal.events = [
            input(21, task, turn: turn, "…and the tests"),
            output(25, task, "Done"),
        ]
        shortJournal.head = 40
        short.didLoad(snapshot: snapshot(task, cursor: 40), planeRegistryID: Self.plane, headCursor: 40)
        await short.waitForReplay()
        #expect(short.historyRecord(for: task)?.phase == .finished(.beyondLimit))
        #expect(short.historyRecord(for: task)?.floor == 20)
        #expect(short.historyRecord(for: task)?.entries.map(\.text) == ["Done"])

        let completeJournal = FakeJournal()
        let complete = makeStore(completeJournal)
        completeJournal.events = [
            input(1, task, turn: turn, "Fix the tests"),
            output(5, task, "Done"),
        ]
        completeJournal.head = 15
        complete.didLoad(snapshot: snapshot(task, cursor: 15), planeRegistryID: Self.plane, headCursor: 15)
        await complete.waitForReplay()
        #expect(complete.historyRecord(for: task)?.phase == .finished(nil))
        #expect(complete.historyRecord(for: task)?.entries.map(\.text) == ["Fix the tests", "Done"])
    }

    @Test
    func shortPagesAdvanceByTheirLastEvent() async {
        let journal = FakeJournal()
        journal.pageEvents = 3
        let store = makeStore(journal)
        let task = UUID()
        journal.events = (1 ... 12).map { output(UInt64($0), task, "Line \($0)") }
        journal.head = 12

        store.didLoad(snapshot: snapshot(task, cursor: 12), planeRegistryID: Self.plane, headCursor: 12)
        await store.waitForReplay()

        // The batch cursor is the journal head; following it would skip Events.
        #expect(journal.afters == [0, 3, 6, 9])
        #expect(store.historyRecord(for: task)?.entries.count == 12)
    }

    @Test
    func anEmptyPageEndsTheWindow() async {
        let journal = FakeJournal()
        let store = makeStore(journal)
        let task = UUID()
        journal.events = (1 ... 20).map { output(UInt64($0), task, "Line \($0)") }
        journal.head = 30

        store.didLoad(snapshot: snapshot(task, cursor: 30), planeRegistryID: Self.plane, headCursor: 30)
        await store.waitForReplay()

        #expect(journal.afters == [10, 20, 0])
        #expect(store.historyRecord(for: task)?.phase == .finished(nil))
        #expect(store.historyRecord(for: task)?.entries.count == 20)
    }

    @Test
    func aTransientFailureKeepsTheFinishedWindowsAndTryAgainCompletes() async {
        let journal = FakeJournal()
        let store = makeStore(journal)
        let task = UUID()
        journal.events = [created(1, task)] + (2 ... 60).map { output(UInt64($0), task, "Line \($0)") }
        journal.head = 60
        journal.failures[2] = JetClientFailure.presentation(.offline)

        store.didLoad(snapshot: snapshot(task, cursor: 60), planeRegistryID: Self.plane, headCursor: 60)
        await store.waitForReplay()

        let failed = store.historyRecord(for: task)
        #expect(failed?.phase == .finished(.unreachable))
        #expect(failed?.floor == 40)
        #expect(failed?.entries.compactMap(\.sequence) == Array(41 ... 60))
        #expect(store.historyState(for: task) == .unavailable)
        let notice = store.presentation(for: task, live: []).notice
        if case let .unavailable(summary, canRetry) = notice {
            #expect(summary != nil)
            #expect(canRetry)
        } else {
            Issue.record("Expected the retry card, got \(String(describing: notice)).")
        }
        #expect(notice?.title == "Couldn't load earlier messages.")

        store.retryReplay(for: task)
        await store.waitForReplay()
        let retried = store.historyRecord(for: task)
        #expect(retried?.phase == .finished(nil))
        #expect(retried?.entries.count == 60)
        #expect(retried?.automaticAttempts == 0)
        #expect(store.presentation(for: task, live: []).notice == nil)
    }

    @Test
    func automaticRetriesWaitFifteenSecondsAndStopAfterThree() async {
        let journal = FakeJournal()
        let clock = TranscriptManualClock()
        let store = makeStore(journal, clock: clock)
        let task = UUID()
        journal.head = 60
        journal.failsEveryCall = JetClientFailure.presentation(.offline)
        func load() async {
            store.didLoad(snapshot: snapshot(task, cursor: 60), planeRegistryID: Self.plane, headCursor: 60)
            await store.waitForReplay()
        }

        await load()
        #expect(journal.afters.count == 1)
        await load()
        clock.now = .seconds(10)
        await load()
        #expect(journal.afters.count == 1)

        for attempt in 1 ... 3 {
            clock.now += .seconds(15)
            await load()
            #expect(journal.afters.count == 1 + attempt)
            #expect(store.historyRecord(for: task)?.automaticAttempts == attempt)
        }
        clock.now += .seconds(60)
        await load()
        #expect(journal.afters.count == 4)
        #expect(store.historyRecord(for: task)?.phase == .finished(.unreachable))
    }

    @Test
    func nothingIsFetchedWhenTheHistoryIsKnown() async {
        let journal = FakeJournal()
        let store = makeStore(journal)
        let finished = UUID()
        journal.events = [created(1, finished), output(2, finished, "Hi")]
        journal.head = 10

        store.didLoad(snapshot: snapshot(finished, cursor: 10), planeRegistryID: Self.plane, headCursor: 10)
        await store.waitForReplay()
        #expect(journal.afters == [0, 2])
        #expect(store.historyRecord(for: finished)?.phase == .finished(nil))
        // Snapshot and supervision loads repeat; the finished replay is remembered.
        for head in UInt64(11) ... 15 {
            store.didLoad(snapshot: snapshot(finished, cursor: head), planeRegistryID: Self.plane, headCursor: head)
        }
        await store.waitForReplay()
        #expect(journal.afters == [0, 2])

        let observed = UUID()
        store.markObservedStart(observed)
        store.didLoad(snapshot: snapshot(observed, cursor: 10), planeRegistryID: Self.plane, headCursor: 10)
        #expect(store.historyState(for: observed) == .complete)

        let fresh = UUID()
        store.didLoad(snapshot: snapshot(fresh, runs: 0, cursor: 10), planeRegistryID: Self.plane, headCursor: 10)
        #expect(store.historyState(for: fresh) == .notStarted)
        #expect(store.presentation(for: fresh, live: []).notice == .noMessages)
        #expect(store.presentation(for: fresh, live: []).notice?.title == "No Messages Yet")

        let draft = [entry("u", .user, "Queued", sequence: 9)]
        store.save(draft, for: fresh)
        #expect(store.historyState(for: fresh) == .complete)
        #expect(store.presentation(for: fresh, live: draft) == .init(entries: draft, notice: nil))

        await store.waitForReplay()
        #expect(journal.afters == [0, 2])
    }

    @Test
    func aBurstOfLoadsRunsOneReplay() async {
        let journal = FakeJournal()
        journal.suspendAtCall = 1
        let store = makeStore(journal)
        let task = UUID()
        journal.events = [created(1, task), output(2, task, "Hi")]
        journal.head = 10

        store.didLoad(snapshot: snapshot(task, cursor: 10), planeRegistryID: Self.plane, headCursor: 10)
        await journal.waitUntilSuspended()
        for head in UInt64(11) ... 20 {
            store.didLoad(snapshot: snapshot(task, cursor: head), planeRegistryID: Self.plane, headCursor: head)
        }
        await journal.release()
        await store.waitForReplay()

        #expect(journal.afters == [0, 2])
        #expect(store.historyRecord(for: task)?.fence == 10)
        #expect(store.historyRecord(for: task)?.phase == .finished(nil))
    }

    @Test
    func cancellingDuringAFetchIgnoresTheLateBatch() async {
        let journal = FakeJournal()
        journal.suspendAtCall = 1
        let store = makeStore(journal)
        let task = UUID()
        journal.events = [created(1, task), output(2, task, "Hi")]
        journal.head = 10

        store.didLoad(snapshot: snapshot(task, cursor: 10), planeRegistryID: Self.plane, headCursor: 10)
        await journal.waitUntilSuspended()
        store.cancelReplay()
        #expect(store.historyState(for: task) == .partial)
        #expect(store.presentation(for: task, live: []).notice == .loading(progress: nil))

        await journal.release()
        let cancelled = store.historyRecord(for: task)
        #expect(cancelled?.phase == .idle)
        #expect(cancelled?.entries.isEmpty == true)
        #expect(cancelled?.revision == 0)

        store.didLoad(snapshot: snapshot(task, cursor: 10), planeRegistryID: Self.plane, headCursor: 10)
        await store.waitForReplay()
        #expect(journal.afters == [0, 0, 2])
        #expect(store.historyRecord(for: task)?.phase == .finished(nil))
        #expect(store.historyRecord(for: task)?.entries.map(\.text) == ["1 background update", "Hi"])
    }

    @Test
    func openingAnotherTaskCancelsTheReplay() async {
        let journal = FakeJournal()
        journal.suspendAtCall = 1
        let store = makeStore(journal)
        let first = UUID()
        let second = UUID()
        journal.events = [created(1, first), created(2, second), output(3, second, "Second")]
        journal.head = 10

        store.didLoad(snapshot: snapshot(first, cursor: 10), planeRegistryID: Self.plane, headCursor: 10)
        await journal.waitUntilSuspended()
        store.didLoad(snapshot: snapshot(second, cursor: 10), planeRegistryID: Self.plane, headCursor: 10)
        #expect(store.historyState(for: first) == .partial)

        await journal.release()
        await store.waitForReplay()
        #expect(store.historyRecord(for: first)?.phase == .idle)
        #expect(store.historyRecord(for: first)?.entries.isEmpty == true)
        #expect(store.historyRecord(for: second)?.phase == .finished(nil))
        #expect(store.historyRecord(for: second)?.entries.last?.text == "Second")
    }

    @Test(arguments: [false, true])
    func removingOrResettingATaskCancelsAndDropsItsReplay(_ resets: Bool) async {
        let journal = FakeJournal()
        journal.suspendAtCall = 1
        let store = makeStore(journal)
        let task = UUID()
        journal.events = [created(1, task), output(2, task, "Hi")]
        journal.head = 10

        store.didLoad(snapshot: snapshot(task, cursor: 10), planeRegistryID: Self.plane, headCursor: 10)
        await journal.waitUntilSuspended()
        if resets {
            store.reset(conversationIDs: [task])
        } else {
            store.remove(task)
        }
        #expect(store.historyRecord(for: task) == nil)

        await journal.release()
        await store.waitForReplay()
        #expect(store.historyRecord(for: task) == nil)
        #expect(store.historyState(for: task) == .partial)
        #expect(store.presentation(for: task, live: []) == .init(entries: [], notice: nil))
        #expect(!store.cachedConversationIDs.contains(task))
    }

    @Test
    func theOpenTaskSurvivesIngestsIntoOtherTasks() {
        let store = TranscriptStore()
        let open = UUID()
        store.save([entry("m", .agent, "Open")], for: open)
        store.didLoad(snapshot: snapshot(open), planeRegistryID: Self.plane, headCursor: 5)

        for _ in 0 ..< 40 {
            store.ingest(projections: [entry("n", .agent, "Other")], rawSequence: 1, conversationID: UUID())
        }
        #expect(store.cachedConversationIDs.count == 32)
        #expect(store.cachedConversationIDs.contains(open))
        #expect(store.entries(for: open).map(\.text) == ["Open"])
        #expect(store.historyRecord(for: open) != nil)

        // Leaving the task ends its protection.
        store.cancelReplay()
        for _ in 0 ..< 40 {
            store.ingest(projections: [entry("n", .agent, "Other")], rawSequence: 1, conversationID: UUID())
        }
        #expect(!store.cachedConversationIDs.contains(open))
        #expect(store.historyRecord(for: open) == nil)
    }

    // MARK: - Combine

    @Test(arguments: CombineRule.allCases)
    func combineRules(_ rule: CombineRule) {
        let example = combineExample(rule)
        let result = TranscriptStore.combine(
            live: example.live,
            history: example.history,
            floor: 1_000,
            fence: 1_010,
            limit: example.limit
        )
        #expect(result.entries == example.expected)
        #expect(result.droppedOlder == example.droppedOlder)
        #expect(Set(result.entries.map(\.id)).count == result.entries.count)

        // Combining the result again changes nothing.
        let again = TranscriptStore.combine(
            live: result.entries,
            history: example.history,
            floor: 1_000,
            fence: 1_010,
            limit: example.limit
        )
        #expect(again.entries == result.entries)
    }

    private func combineExample(_ rule: CombineRule) -> (
        live: [JetTimelineEntry],
        history: [JetTimelineEntry],
        expected: [JetTimelineEntry],
        droppedOlder: Bool,
        limit: Int
    ) {
        let below = entry("below", .agent, "Cached reply", sequence: 5)
        let after = entry("after", .agent, "New reply", sequence: 1_020)
        let first = entry("h1", .agent, "Replayed reply", sequence: 1_002)
        let second = entry("h2", .agent, "Read", sequence: 1_004)
        switch rule {
        case .liveSurroundsTheReplay:
            return ([below, after], [first, second], [below, first, second, after], false, 256)
        case .theReplayOwnsItsRange:
            let stray = entry("stray", .agent, "Cached only", sequence: 1_005)
            return ([stray], [first], [first], false, 256)
        case .theLongerMessageWins:
            let short = entry("turn", .user, "Fix the", sequence: 1_002)
            let long = entry("turn", .user, "Fix the redirect", sequence: 1_012)
            return ([long], [short, second], [long, second], false, 256)
        case .anEqualMessageKeepsTheFirst:
            let replayed = entry("turn", .user, "abc", sequence: 1_002)
            let cached = entry("turn", .user, "xyz", sequence: 1_012)
            return ([cached], [replayed], [replayed], false, 256)
        case .theLaterApprovalWins:
            let question = entry("turn", .user, "Update the SDK", sequence: 1_002)
            let requested = approval(.requested, sequence: 1_005)
            let allowed = approval(.allowed, sequence: 1_030)
            let reply = entry("reply", .agent, "Updated", sequence: 1_031)
            return ([allowed, reply], [question, requested], [question, allowed, reply], false, 256)
        case .anEarlierApprovalLoses:
            let allowed = approval(.allowed, sequence: 1_005)
            let requested = approval(.requested, sequence: 1_003)
            return ([requested], [allowed], [allowed], false, 256)
        case .otherKindsKeepTheFirst:
            let replayed = entry("a", .agent, "From the journal", sequence: 1_003)
            let cached = entry("a", .agent, "Cached", sequence: 1_003)
            return ([cached], [replayed], [replayed], false, 256)
        case .entriesWithoutSequenceStayLive:
            let pending = JetTimelineEntry(id: "pending", kind: .user, text: "Pending", sequence: nil, rawCount: 0)
            return ([pending], [first], [first, pending], false, 256)
        case .theNewestEntriesAreKept:
            return ([below, after], [first, second], [second, after], true, 2)
        }
    }

    // MARK: - Progress

    @Test
    func progressNeverDecreasesAndStaysBelowOne() async {
        let journal = FakeJournal()
        journal.pageEvents = 4
        let clock = TranscriptManualClock()
        let store = makeStore(journal, clock: clock)
        let task = UUID()
        journal.events = (1 ... 60).map { output(UInt64($0), task, "Line \($0)", recordedAt: Self.createdAt + Int64($0) * 60_000) }
        journal.head = 60
        var seen: [Double] = []
        journal.onFetch = { _ in
            clock.now += .milliseconds(400)
            if case let .running(progress) = store.historyRecord(for: task)?.phase { seen.append(progress) }
        }

        store.didLoad(snapshot: snapshot(task, cursor: 60), planeRegistryID: Self.plane, headCursor: 60)
        await store.waitForReplay()

        #expect(seen.count == journal.afters.count)
        #expect(seen.count > 10)
        #expect(zip(seen, seen.dropFirst()).allSatisfy { $0 <= $1 })
        #expect(seen.allSatisfy { $0 >= 0 && $0 <= 0.99 })
        #expect((seen.last ?? 0) > 0)
    }

    @Test
    func progressIsBoundedAndMonotonic() {
        let budget = TranscriptStore.ReplayBudget()
        func progress(previous: Double, bytes: Int, elapsed: Duration) -> Double {
            TranscriptStore.replayProgress(
                previous: previous,
                window: (done: 0, after: 10, start: 0, end: 2_000),
                bytes: bytes,
                elapsed: elapsed,
                oldestRecordedAtUnixMilliseconds: nil,
                createdAtUnixMilliseconds: 0,
                nowUnixMilliseconds: 1,
                budget: budget
            )
        }
        #expect(progress(previous: 0.5, bytes: 0, elapsed: .zero) == 0.5)
        #expect(progress(previous: 0, bytes: budget.maximumBytes * 2, elapsed: .zero) == 0.99)
        #expect(progress(previous: 0, bytes: 0, elapsed: .seconds(4)) == 0.5)
    }

    // MARK: - Copy

    @Test
    func theSummaryNamesTheStartAndTheLastRun() {
        let locale = Locale(identifier: "en_US")
        let gmt = TimeZone(identifier: "GMT")!
        let now = Date(timeIntervalSince1970: 1_790_600_000)

        #expect(TranscriptStore.summary(facts(), now: now, locale: locale, timeZone: gmt)
            == "Started Sep 26 · Last run ended Sep 27")
        #expect(TranscriptStore.summary(facts(endedAt: nil), now: now, locale: locale, timeZone: gmt)
            == "Started Sep 26")
        #expect(TranscriptStore.summary(facts(createdAt: 1_758_888_000_000), now: now, locale: locale, timeZone: gmt)
            == "Started Sep 26, 2025 · Last run ended Sep 27")
    }

    @Test
    func aLiveLastRunHasNoEndInTheSummary() {
        let store = TranscriptStore()
        let task = UUID()
        let conversation = JetConversationSummary(
            id: task, revision: 1, title: "Task", createdAtUnixMilliseconds: Self.createdAt, projectID: nil
        )
        let ended = JetRunSummary(
            id: UUID(), conversationID: task, revision: 1, lifecycle: .completed, title: "Task",
            createdAtUnixMilliseconds: Self.createdAt, endedAtUnixMilliseconds: Self.endedAt
        )
        let live = JetRunSummary(
            id: UUID(), conversationID: task, revision: 1, lifecycle: .active, title: "Task",
            createdAtUnixMilliseconds: Self.endedAt, endedAtUnixMilliseconds: nil
        )
        let snapshot = JetConversationSnapshot(
            cursor: 5, conversation: conversation, workspaceID: nil, workspaceRoot: nil, runs: [ended, live]
        )
        store.didLoad(snapshot: snapshot, planeRegistryID: Self.plane, headCursor: 5)
        #expect(store.historyRecord(for: task)?.facts?.lastRunEndedAtUnixMilliseconds == nil)
        #expect(store.historyRecord(for: task)?.facts?.createdAtUnixMilliseconds == Self.createdAt)
        #expect(store.historyRecord(for: task)?.facts?.hasRuns == true)
    }

    @Test
    func noHistoryCopyUsesAJetDomainWord() {
        let notices: [TranscriptStore.HistoryNotice] = [
            .loading(progress: nil),
            .loading(progress: 0.4),
            .unavailable(summary: nil, canRetry: false),
            .unavailable(summary: nil, canRetry: true),
            .noMessages,
        ]
        for notice in notices {
            #expect(JetCopy.foundAvoidWords(in: notice.title).isEmpty, "\(notice.title)")
        }
        let summary = TranscriptStore.summary(
            facts(), now: Date(), locale: Locale(identifier: "en_US"), timeZone: .gmt
        )
        #expect(JetCopy.foundAvoidWords(in: summary).isEmpty)
        #expect(notices.map(\.title) == [
            "Loading earlier messages…",
            "Loading earlier messages…",
            "Earlier messages aren't available on this Mac.",
            "Couldn't load earlier messages.",
            "No Messages Yet",
        ])
    }

    #if DEBUG
    @Test
    func aPreviewSeedFeedsTheSessionTranscript() {
        let session = DesktopSession.preview { DesktopPreviewData.waitingWithChanges($0) }
        guard let task = session.selectedConversationID else {
            Issue.record("The preview has no open task.")
            return
        }
        // The preview follows the task from its start, so it has no history row.
        #expect(session.historyNotice == nil)
        #expect(session.transcriptEntries == session.timeline)

        let history = [
            entry("h-u", .user, "Sign-in is slow on staging.", sequence: 202),
            entry("h-a", .agent, "The session is read twice per request.", sequence: 204),
        ]
        session.transcripts.seedPreviewHistory(
            task,
            facts: facts(),
            phase: .finished(.journalTrimmed),
            entries: history,
            floor: 200,
            fence: 300
        )

        if case let .unavailable(summary, canRetry) = session.historyNotice {
            #expect(summary != nil)
            #expect(!canRetry)
        } else {
            Issue.record("Expected the history card, got \(String(describing: session.historyNotice)).")
        }
        let combined = TranscriptStore.combine(
            live: session.timeline, history: history, floor: 200, fence: 300
        ).entries
        #expect(session.transcriptEntries == combined)
        #expect(session.transcriptEntries.prefix(2).map(\.id) == ["h-u", "h-a"])
        #expect(session.transcriptEntries.count == history.count + session.timeline.count)
    }
    #endif

    // MARK: - Helpers

    nonisolated static let plane = UUID()
    /// Sep 26, 2026 12:00 UTC.
    nonisolated static let createdAt: Int64 = 1_790_424_000_000
    /// Sep 27, 2026 12:00 UTC.
    nonisolated static let endedAt: Int64 = 1_790_510_400_000
    static let small = TranscriptStore.ReplayBudget(windows: 3, windowSpan: 20)

    private func makeStore(
        _ journal: FakeJournal,
        budget: TranscriptStore.ReplayBudget? = nil,
        clock: TranscriptManualClock? = nil
    ) -> TranscriptStore {
        let store = TranscriptStore(
            budget: budget ?? Self.small,
            clock: (clock ?? TranscriptManualClock()).replayClock,
            now: { Date(timeIntervalSince1970: 1_790_600_000) }
        )
        store.configure { planeRegistryID, after in
            try await journal.fetch(planeRegistryID, after: after)
        }
        return store
    }

    private func facts(
        createdAt: Int64 = createdAt,
        endedAt: Int64? = endedAt
    ) -> TranscriptStore.HistoryFacts {
        TranscriptStore.HistoryFacts(
            planeRegistryID: Self.plane,
            createdAtUnixMilliseconds: createdAt,
            lastRunEndedAtUnixMilliseconds: endedAt,
            hasRuns: true,
            fence: 300
        )
    }

    /// What the replay counts for one Event.
    static func cost(_ event: JetEvent) -> Int {
        event.payload.source.utf8.count + event.actor.source.utf8.count
            + (event.origin?.source.utf8.count ?? 0) + 256
    }

    private func event(
        _ sequence: UInt64,
        _ conversationID: UUID,
        kind: String,
        payload: String,
        recordedAt: Int64? = nil
    ) -> JetEvent {
        JetEvent(
            sequence: sequence,
            eventID: UUID(),
            actor: JetRawJSON(source: #"{"interactive_client":{}}"#),
            origin: nil,
            recordedAtUnixMilliseconds: recordedAt ?? Self.createdAt + Int64(sequence) * 1_000,
            conversationID: conversationID,
            runID: nil,
            kind: kind,
            payloadVersion: 1,
            payload: JetRawJSON(source: payload)
        )
    }

    private func output(
        _ sequence: UInt64,
        _ conversationID: UUID,
        _ text: String,
        recordedAt: Int64? = nil
    ) -> JetEvent {
        event(
            sequence,
            conversationID,
            kind: "run.output",
            payload: #"{"presentation_json":["{\"kind\":\"markdown\",\"text\":\"\#(text)\"}"]}"#,
            recordedAt: recordedAt
        )
    }

    private func input(_ sequence: UInt64, _ conversationID: UUID, turn: UUID, _ text: String) -> JetEvent {
        event(
            sequence,
            conversationID,
            kind: "turn.input",
            payload: #"{"turn_id":"\#(turn.uuidString)","text":"\#(text)"}"#
        )
    }

    private func created(_ sequence: UInt64, _ conversationID: UUID) -> JetEvent {
        event(sequence, conversationID, kind: "conversation.created", payload: "{}")
    }

    private func entry(
        _ id: String,
        _ kind: JetTimelineKind,
        _ text: String,
        sequence: UInt64 = 1
    ) -> JetTimelineEntry {
        JetTimelineEntry(id: id, kind: kind, text: text, sequence: sequence, rawCount: 0)
    }

    private func approval(_ state: JetApprovalState, sequence: UInt64 = 3) -> JetTimelineEntry {
        JetTimelineEntry(
            id: "approval-run-req-1",
            kind: .approval,
            text: "shell needs approval.",
            sequence: sequence,
            rawCount: 0,
            approval: JetApprovalPresentation(
                requestID: "req-1", reviewID: nil, runID: nil, tool: "shell", action: "{}",
                target: "Current Run", scope: "This action once", consequence: "",
                rationale: nil, state: state, canAuthorizeRetry: false
            )
        )
    }

    private func snapshot(_ id: UUID, runs: Int = 1, cursor: UInt64 = 5) -> JetConversationSnapshot {
        let conversation = JetConversationSummary(
            id: id, revision: 1, title: "Task", createdAtUnixMilliseconds: Self.createdAt, projectID: nil
        )
        return JetConversationSnapshot(
            cursor: cursor,
            conversation: conversation,
            workspaceID: nil,
            workspaceRoot: nil,
            runs: (0 ..< runs).map { _ in
                JetRunSummary(
                    id: UUID(), conversationID: id, revision: 1, lifecycle: .completed,
                    title: "Task", createdAtUnixMilliseconds: Self.createdAt,
                    endedAtUnixMilliseconds: Self.endedAt
                )
            }
        )
    }
}

/// The rules of `TranscriptStore.combine` (WP13 spec §7).
nonisolated enum CombineRule: String, CaseIterable, Sendable {
    case liveSurroundsTheReplay
    case theReplayOwnsItsRange
    case theLongerMessageWins
    case anEqualMessageKeepsTheFirst
    case theLaterApprovalWins
    case anEarlierApprovalLoses
    case otherKindsKeepTheFirst
    case entriesWithoutSequenceStayLive
    case theNewestEntriesAreKept
}

/// A replay clock the test moves by hand.
@MainActor
final class TranscriptManualClock {
    var now: Duration = .zero

    var replayClock: TranscriptStore.ReplayClock {
        TranscriptStore.ReplayClock { self.now }
    }
}

/// An Event journal: ascending Events cut into pages at 256 Events or 512 KiB.
/// It records every `after`, can fail at a given call, and can hold one call
/// until the test releases it.
@MainActor
final class FakeJournal {
    var events: [JetEvent] = []
    var head: UInt64 = 0
    var minimumAvailable: UInt64 = 0
    var pageEvents = 256
    var pageBytes = 512 * 1_024
    /// Call number (from 1) → the error that call throws.
    var failures: [Int: any Error] = [:]
    var failsEveryCall: (any Error)?
    var suspendAtCall: Int?
    var onFetch: ((Int) -> Void)?
    private(set) var afters: [UInt64] = []
    private var suspended: CheckedContinuation<Void, Never>?
    private var suspensionWaiter: CheckedContinuation<Void, Never>?
    private var returnWaiter: CheckedContinuation<Void, Never>?

    func fetch(_ planeRegistryID: UUID, after: UInt64) async throws -> JetEventBatch {
        afters.append(after)
        let call = afters.count
        onFetch?(call)
        if call == suspendAtCall {
            await withCheckedContinuation { continuation in
                suspended = continuation
                suspensionWaiter?.resume()
                suspensionWaiter = nil
            }
        }
        defer {
            if call == suspendAtCall {
                returnWaiter?.resume()
                returnWaiter = nil
            }
        }
        if let failure = failures[call] ?? failsEveryCall { throw failure }
        if after < minimumAvailable {
            throw JetClientFailure.presentation(JetPresentationError(
                category: .unavailable,
                code: "events.cursor_expired",
                message: "Event cursor expired.",
                retryable: false,
                restart: .cursorExpired(minimumAvailable: minimumAvailable, snapshotRevision: head)
            ))
        }
        var page: [JetEvent] = []
        var bytes = 0
        for event in events where event.sequence > after {
            let size = event.payload.source.utf8.count + event.actor.source.utf8.count
            if page.count == pageEvents || (!page.isEmpty && bytes + size > pageBytes) { break }
            page.append(event)
            bytes += size
        }
        return JetEventBatch(cursor: max(head, after), events: page)
    }

    /// Returns once the held call is waiting.
    func waitUntilSuspended() async {
        guard suspended == nil else { return }
        await withCheckedContinuation { suspensionWaiter = $0 }
    }

    /// Lets the held call return, then returns after the replay has handled it.
    func release() async {
        await withCheckedContinuation { continuation in
            returnWaiter = continuation
            let held = suspended
            suspended = nil
            held?.resume()
        }
    }
}
