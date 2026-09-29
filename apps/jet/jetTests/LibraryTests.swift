import Foundation
import Testing
@testable import jet

@MainActor
struct LibraryTests {
    // MARK: 1. Presets

    @Test
    func presetsRoundTripAndOnlyFourCombinationsArePresets() {
        for preset in ReplyFinishPreset.allCases {
            #expect(ReplyFinishPreset.matching(preset.flags) == preset)
        }
        let combinations = (0 ..< 16).map { bits in
            ReplyFinishFlags(values: (0 ..< 4).map { bits & (1 << $0) != 0 })
        }
        #expect(Set(combinations).count == 16)
        #expect(combinations.compactMap(ReplyFinishPreset.matching).count == 4)
    }

    // MARK: 2. Planner

    @Test
    func plannerTurnsOnInPolicyOrderAndOffInReverseFirst() {
        let keys = ReplyFinishFlags.keys
        let off = ReplyFinishScopeState(effective: ReplyFinishFlags())
        #expect(ReplyFinishModel.writes(current: off, target: .projectPreset(ReplyFinishPreset.saveAndOpenPullRequest.flags)) == [
            .set(keys[0], .flag(true)), .set(keys[1], .flag(true)), .set(keys[2], .flag(true)), .set(keys[3], .flag(true)),
        ])

        let all = ReplyFinishScopeState(effective: ReplyFinishPreset.saveAndOpenPullRequest.flags)
        #expect(ReplyFinishModel.writes(current: all, target: .projectPreset(ReplyFinishFlags())) == [
            .set(keys[3], .flag(false)), .set(keys[2], .flag(false)), .set(keys[1], .flag(false)), .set(keys[0], .flag(false)),
        ])

        // A custom mix: pushing without committing. Offs come before ons.
        let custom = ReplyFinishScopeState(effective: ReplyFinishFlags(branch: true, push: true))
        #expect(ReplyFinishModel.writes(current: custom, target: .projectPreset(ReplyFinishPreset.saveToBranch.flags)) == [
            .set(keys[2], .flag(false)), .set(keys[1], .flag(true)),
        ])
    }

    @Test
    func taskPresetsPinAllKeysAndProjectDefaultClearsOnlyExplicitKeys() {
        let keys = ReplyFinishFlags.keys
        let following = ReplyFinishScopeState(effective: ReplyFinishPreset.saveToBranch.flags)
        #expect(ReplyFinishModel.writes(current: following, target: .taskPreset(ReplyFinishPreset.saveAndPush.flags)) == [
            .set(keys[2], .flag(true)),
            .set(keys[0], .flag(true)), .set(keys[1], .flag(true)), .set(keys[3], .flag(false)),
        ])

        let pinned = ReplyFinishScopeState(
            effective: ReplyFinishPreset.saveAndPush.flags,
            explicit: [true, true, true, false]
        )
        #expect(ReplyFinishModel.writes(current: pinned, target: .projectDefault(ReplyFinishPreset.saveToBranch.flags)) == [
            .clear(keys[2]), .clear(keys[0]), .clear(keys[1]),
        ])
    }

    @Test
    func theCurrentPresetWritesNothing() {
        let project = ReplyFinishScopeState(effective: ReplyFinishPreset.saveToBranch.flags)
        #expect(ReplyFinishModel.writes(current: project, target: .projectPreset(ReplyFinishPreset.saveToBranch.flags)).isEmpty)
        let task = ReplyFinishScopeState(effective: ReplyFinishPreset.saveToBranch.flags, explicit: [true, true, true, true])
        #expect(ReplyFinishModel.writes(current: task, target: .taskPreset(ReplyFinishPreset.saveToBranch.flags)).isEmpty)
    }

    // MARK: 3. Push confirmation

    @Test(arguments: [
        (ReplyFinishPreset.doNothing, ReplyFinishPreset.saveToBranch, false),
        (.saveToBranch, .saveAndPush, true),
        (.saveAndPush, .saveAndOpenPullRequest, true),
        (.doNothing, .saveAndOpenPullRequest, true),
        (.saveAndOpenPullRequest, .saveAndPush, false),
        (.saveAndPush, .saveToBranch, false),
        (.saveAndPush, .saveAndPush, false),
    ])
    func pushConfirmationIsNeededOnlyWhenPushingNewlyStarts(
        from: ReplyFinishPreset,
        to: ReplyFinishPreset,
        needed: Bool
    ) {
        #expect(ReplyFinishModel.needsPushConfirmation(from: from.flags, to: to.flags) == needed)
    }

    @Test
    func choosingAPushingPresetAsksFirst() async {
        let fake = LibraryAccessFake()
        let model = projectModel(fake)
        await model.load()
        await model.choose(.preset(.saveAndPush))
        #expect(model.pendingConfirmation == .preset(.saveAndPush))
        #expect(await fake.commands.isEmpty)
        await model.confirmPending()
        #expect(model.choice == .preset(.saveAndPush))
        #expect(await fake.commands.count == 3)
    }

    // MARK: 4. Failures and uncertain writes

    @Test
    func aFailureStopsAndTryAgainResumes() async {
        let fake = LibraryAccessFake()
        await fake.failCommand(1, with: .refuse(.overloaded))
        let model = projectModel(fake)
        await model.load()

        await model.choose(.preset(.saveToBranch))
        #expect(model.choice == .custom)
        #expect(model.issue?.text == "Couldn't finish changing this setting. Some steps were saved.")
        #expect(model.issue?.action == .tryAgain)
        #expect(await fake.commands.count == 2)

        await model.tryAgain()
        let commands = await fake.commands
        #expect(model.choice == .preset(.saveToBranch))
        #expect(model.issue == nil)
        #expect(commands.map(\.call) == [
            .set("git.auto_branch", .flag(true), .project(fake.projectID)),
            .set("git.auto_commit", .flag(true), .project(fake.projectID)),
            .set("git.auto_commit", .flag(true), .project(fake.projectID)),
        ])
        #expect(commands[1].commandID != commands[2].commandID)
    }

    @Test
    func aConfirmedUncertainWriteContinues() async {
        let fake = LibraryAccessFake()
        await fake.failCommand(0, with: .unknownApplied)
        let model = projectModel(fake)
        await model.load()

        await model.choose(.preset(.saveToBranch))
        #expect(model.choice == .preset(.saveToBranch))
        #expect(model.issue == nil)
        #expect(await fake.commands.count == 2)
    }

    @Test
    func anUnconfirmedWriteStopsAndSendSameRequestAgainReusesItsIDAndBody() async {
        let fake = LibraryAccessFake()
        await fake.failCommand(0, with: .unknownNotApplied)
        let model = projectModel(fake)
        await model.load()

        await model.choose(.preset(.saveToBranch))
        #expect(model.choice == .preset(.doNothing))
        #expect(model.issue?.action == .sendSameRequestAgain)
        #expect(await fake.commands.count == 1)

        await model.sendSameRequestAgain()
        let commands = await fake.commands
        #expect(commands.count == 3)
        #expect(commands[1].commandID == commands[0].commandID)
        #expect(commands[1].call == commands[0].call)
        #expect(model.choice == .preset(.saveToBranch))
        #expect(model.uncertain == nil)
    }

    @Test
    func aTaskFollowsItsProjectUntilItChoosesAPreset() async {
        let fake = LibraryAccessFake()
        await fake.setProjectFlags(ReplyFinishPreset.saveToBranch.flags.values)
        let model = ReplyFinishModel(
            planeRegistryID: UUID(),
            scope: .task(conversationID: fake.conversationID, projectID: fake.projectID),
            makeAccess: { _ in fake }
        )
        await model.load()
        #expect(model.choice == .projectDefault(.saveToBranch))
        #expect(model.choices.first == .projectDefault(.saveToBranch))
        #expect(model.title(for: .projectDefault(.saveToBranch)) == "Use Project Default (Save to a Branch)")

        await model.choose(.preset(.doNothing))
        #expect(model.choice == .preset(.doNothing))
        #expect(await fake.commands.count == 4)

        await model.choose(.projectDefault(.saveToBranch))
        #expect(model.choice == .projectDefault(.saveToBranch))
        let clears = await fake.commands.dropFirst(4).map(\.call)
        #expect(clears.allSatisfy { if case .clear = $0 { true } else { false } })
    }

    // MARK: 5. Branch prefix

    @Test(arguments: ["jet/", "alex/", "feature-", ""])
    func validBranchPrefixes(_ prefix: String) {
        #expect(ReplyFinishModel.branchPrefixProblem(prefix) == nil)
    }

    @Test(arguments: ["my branch/", "a~b", "a^b", "a:b", "a?", "a*", "a[b", "a\\b", "tab\tname"])
    func invalidBranchPrefixes(_ prefix: String) {
        #expect(ReplyFinishModel.branchPrefixProblem(prefix) == "Branch names can't contain spaces or ~ ^ : ? * [ \\.")
    }

    @Test
    func anInvalidPrefixSendsNothing() async {
        let fake = LibraryAccessFake()
        let model = projectModel(fake)
        await model.load()
        await model.setBranchPrefix("my branch/")
        #expect(model.branchPrefixError != nil)
        #expect(await fake.commands.isEmpty)

        await model.setBranchPrefix("alex/")
        #expect(model.branchPrefixError == nil)
        #expect(model.branchPrefix == "alex/")
        #expect(model.branchPrefixIsExplicit)
    }

    // MARK: 6. Jet Trash rows

    @Test
    func trashRowsUseTheTitleOrderNotesAndNewestFirst() {
        let fetchedID = UUID(), listedID = UUID(), rememberedID = UUID(), untitledID = UUID(), transferID = UUID()
        let project = UUID()
        let entries = [
            trashEntry(untitledID, "autodelete_rule", day: 1),
            trashEntry(fetchedID, "manual_forget", day: 5),
            trashEntry(listedID, "delete_everywhere", day: 3),
            trashEntry(rememberedID, "automatic_forget", day: 4),
            trashEntry(transferID, "plane_transfer", day: 2),
        ]
        let rows = JetTrashRow.rows(
            entries: entries,
            fetched: [fetchedID: summary(fetchedID, "From the computer", project: project)],
            listed: [
                summary(fetchedID, "From the list", project: nil),
                summary(listedID, "From the list", project: nil),
            ],
            remembered: { $0 == rememberedID || $0 == listedID ? "Remembered" : nil },
            projects: [project: "web-app"]
        )
        #expect(rows.map(\.id) == [fetchedID, rememberedID, listedID, transferID, untitledID])
        #expect(rows.map(\.title) == ["From the computer", "Remembered", "From the list", "Untitled task", "Untitled task"])
        #expect(rows.map(\.note) == [nil, "Cleaned up automatically", "Deleted everywhere", "Moved to another computer", "Cleaned up automatically"])
        #expect(rows.map(\.projectName) == ["web-app", nil, nil, nil, nil])
        #expect(rows.map(\.canRestore) == [true, true, true, false, true])
    }

    // MARK: 7. Restore

    @Test
    func restoreSendsOneCommandAndForgetsTheTitle() async {
        let fake = LibraryAccessFake()
        let id = UUID()
        await fake.setTrash([trashEntry(id, "manual_forget", day: 1)])
        let memory = testMemory()
        memory.recordTrashedTitle("Fix login redirect", for: id)
        let model = JetTrashModel(makeAccess: { _ in fake }, memory: memory)
        await model.load(planeRegistryID: UUID(), computer: "This Mac", isLocal: true)
        let row = try! #require(model.rows(listed: [], projects: [:]).first)
        #expect(row.title == "Fix login redirect")

        #expect(await model.restore(row, computer: "This Mac", isLocal: true))
        #expect(await fake.commands.map(\.call) == [.restore(id)])
        #expect(memory.trashedTitle(id) == nil)
        #expect(model.notice == LibraryIssue(kind: .confirmation, text: "Restored “Fix login redirect”.", action: .openTask(id)))
        #expect(model.entries.isEmpty)
    }

    @Test
    func anUncertainRestoreKeepsItsCommandID() async {
        let fake = LibraryAccessFake()
        let id = UUID()
        await fake.setTrash([trashEntry(id, "manual_forget", day: 1)])
        await fake.failCommand(0, with: .unknownNotApplied)
        let model = JetTrashModel(makeAccess: { _ in fake }, memory: testMemory())
        await model.load(planeRegistryID: UUID(), computer: "This Mac", isLocal: true)
        let rows = model.rows(listed: [], projects: [:])

        #expect(await model.restore(rows[0], computer: "This Mac", isLocal: true) == false)
        #expect(model.issue?.action == .sendSameRequestAgain)
        #expect(model.uncertainRestore == id)

        #expect(await model.sendSameRequestAgain(rows: rows, computer: "This Mac", isLocal: true))
        let commands = await fake.commands
        #expect(commands.count == 2)
        #expect(commands[0].commandID == commands[1].commandID)
    }

    @Test
    func readOnlyBlocksRestoreAndNotTrashedCountsAsRestored() async {
        let fake = LibraryAccessFake()
        let id = UUID()
        await fake.setTrash([trashEntry(id, "manual_forget", day: 1)])
        await fake.setRecoveryState("read_only")
        let model = JetTrashModel(makeAccess: { _ in fake }, memory: testMemory())
        await model.load(planeRegistryID: UUID(), computer: "This Mac", isLocal: true)
        let row = model.rows(listed: [], projects: [:])[0]
        #expect(model.canChange == false)
        #expect(await model.restore(row, computer: "This Mac", isLocal: true) == false)
        #expect(await fake.commands.isEmpty)
        #expect(model.issue?.text == "Jet paused changes to protect your data.")

        await fake.setRecoveryState("serving")
        await fake.failCommand(0, with: .refuse(JetPresentationError(
            category: .notFound, code: "retention.not_trashed", message: "not in Jet Trash", retryable: false
        )))
        await model.load(planeRegistryID: model.planeRegistryID!, computer: "This Mac", isLocal: true)
        #expect(await model.restore(row, computer: "This Mac", isLocal: true))
    }

    // MARK: 8. Protections

    @Test
    func protectionsReadAsSentencesAndUnknownCodesGoUnderDetails() {
        let result = MoveToTrashModel.protectionLines([
            "active_run", "pending_turn", "dirty_workspace", "unpushed_work",
            "unresolved_effect", "enabled_schedule", "legal_hold",
        ])
        #expect(result.lines.map(\.text) == [
            "Its working copy has changes that aren't saved to a branch.",
            "It has commits that weren't pushed.",
            "A Git step hasn't finished or couldn't be confirmed.",
            "It still repeats a message daily. The message keeps arriving until the task is removed for good.",
        ])
        #expect(result.lines.map(\.offersStopRepeating) == [false, false, false, true])
        #expect(result.details == ["legal_hold"])
    }

    // MARK: 9. Forget

    @Test
    func forgetRecordsTheTitleBeforeStaging() async {
        let fake = LibraryAccessFake()
        let memory = testMemory()
        let seen = TitleBox()
        await fake.setStageObserver { id in
            await MainActor.run { seen.value = memory.trashedTitle(id) }
        }
        let model = moveModel(fake, memory: memory)
        await model.load()

        #expect(await model.moveToTrash())
        #expect(seen.value == "Fix login redirect")
        #expect(memory.trashedTitle(fake.conversationID) == "Fix login redirect")
        #expect(await fake.commands.map(\.call) == [.stage(fake.conversationID, .forget)])
    }

    @Test
    func aRefusalForgetsTheTitleAndAlreadyTrashedShowsJetTrash() async {
        let fake = LibraryAccessFake()
        let memory = testMemory()
        await fake.failCommand(0, with: .refuse(.overloaded))
        let model = moveModel(fake, memory: memory)
        await model.load()
        #expect(await model.moveToTrash() == false)
        #expect(memory.trashedTitle(fake.conversationID) == nil)
        #expect(model.phase == .ready)

        await fake.failCommand(1, with: .refuse(JetPresentationError(
            category: .conflict, code: "retention.already_trashed", message: "", retryable: false
        )))
        await fake.setTrash([trashEntry(fake.conversationID, "manual_forget", day: 1)])
        #expect(await model.moveToTrash() == false)
        if case .alreadyInTrash = model.phase {} else { Issue.record("expected alreadyInTrash, got \(model.phase)") }
    }

    // MARK: 10–11. Stop and Move

    @Test
    func stopAndMoveSendsOneStopPollsThenStagesOnce() async {
        let fake = LibraryAccessFake()
        let runID = UUID()
        await fake.setSnapshot(snapshot(fake.conversationID, runs: [run(runID, fake.conversationID, .active)]), for: fake.conversationID)
        await fake.setPreviews(
            initial: ["active_run", "dirty_workspace"],
            polls: [["active_run", "dirty_workspace"], ["active_run", "dirty_workspace"], ["dirty_workspace"]]
        )
        let sleeps = SleepCounter()
        let model = moveModel(fake, sleep: { _ in await sleeps.tick() })
        await model.load()
        #expect(model.blocker == .activeRun)

        #expect(await model.stopAndMove())
        #expect(await sleeps.count == 3)
        #expect(await fake.commands.map(\.call) == [.stop(runID), .stage(fake.conversationID, .forget)])
    }

    @Test
    func aTimeoutStagesNothing() async {
        let fake = LibraryAccessFake()
        let runID = UUID()
        await fake.setSnapshot(snapshot(fake.conversationID, runs: [run(runID, fake.conversationID, .active)]), for: fake.conversationID)
        await fake.setPreviews(initial: ["active_run"], polls: [])
        let sleeps = SleepCounter()
        let model = moveModel(fake, sleep: { _ in await sleeps.tick() })
        await model.load()

        #expect(await model.stopAndMove() == false)
        #expect(await sleeps.count == MoveToTrashModel.pollLimit)
        #expect(await fake.commands.map(\.call) == [.stop(runID)])
        #expect(model.issue?.text == "Claude Code hasn't stopped yet. Jet is still stopping it.")
        #expect(model.issue?.action == .checkAgain)
    }

    @Test
    func aNewProtectionNeedsAnotherLook() async {
        let fake = LibraryAccessFake()
        let runID = UUID()
        await fake.setSnapshot(snapshot(fake.conversationID, runs: [run(runID, fake.conversationID, .active)]), for: fake.conversationID)
        await fake.setPreviews(initial: ["active_run"], polls: [["unpushed_work"]])
        let model = moveModel(fake, sleep: { _ in })
        await model.load()

        #expect(await model.stopAndMove() == false)
        #expect(await fake.commands.map(\.call) == [.stop(runID)])
        #expect(model.phase == .ready)
        #expect(model.blocker == nil)
        #expect(model.issue?.text == "Claude Code stopped. Check the details, then move the task.")
    }

    // MARK: 12. Waiting messages, starting Runs, uncertain stages

    @Test
    func waitingMessagesBlockForgetButNotDeleteEverywhere() async {
        let fake = LibraryAccessFake()
        await fake.setPreviews(initial: ["pending_turn"], polls: [])
        let model = moveModel(fake)
        await model.load()
        #expect(model.blocker == .pendingTurn)
        #expect(model.headline == "Messages are still waiting to send.")
        #expect(await model.moveToTrash() == false)
        #expect(await fake.commands.isEmpty)

        model.reviewDeleteEverywhere()
        #expect(model.blocker == nil)
        #expect(await model.moveToTrash())
        #expect(await fake.commands.map(\.call) == [.stage(fake.conversationID, .deleteEverywhere)])
    }

    @Test
    func aStartingRunReadsStillStarting() async {
        let fake = LibraryAccessFake()
        await fake.failCommand(0, with: .refuse(JetPresentationError(
            category: .conflict, code: "retention.run_starting", message: "", retryable: false
        )))
        let model = moveModel(fake, mode: .deleteEverywhere)
        await model.load()
        #expect(await model.moveToTrash() == false)
        #expect(model.issue?.text == "Claude Code is still starting. Try again in a moment.")
    }

    @Test
    func thePreviewConfirmsAnUncertainStage() async {
        let fake = LibraryAccessFake()
        await fake.failCommand(0, with: .unknownApplied)
        let model = moveModel(fake)
        await model.load()
        #expect(await model.moveToTrash())

        let unconfirmed = LibraryAccessFake()
        await unconfirmed.failCommand(0, with: .unknownNotApplied)
        let second = moveModel(unconfirmed)
        await second.load()
        #expect(await second.moveToTrash() == false)
        #expect(second.issue?.action == .sendSameRequestAgain)
        #expect(await second.sendSameRequestAgain())
        let commands = await unconfirmed.commands
        #expect(commands.count == 2)
        #expect(commands[0].commandID == commands[1].commandID)
    }

    // MARK: 13. Restore date

    @Test
    func theRestoreDateComesFromTheGracePeriod() async {
        let fake = LibraryAccessFake()
        let model = moveModel(fake)
        await model.load()
        let utc = TimeZone(identifier: "UTC")!
        let english = Locale(identifier: "en_US")
        let date = MoveToTrashModel.dateText(model.restoreDate, now: Self.now, locale: english, timeZone: utc)
        #expect(date == "Oct 28")
        #expect(MoveToTrashModel.forgetMessage(owner: "Claude Code", date: date) == "You can restore it from Jet Trash until Oct 28. After that its working copy and Jet history are deleted. Claude Code's own history isn't deleted.")
        #expect(MoveToTrashModel.forgetMessage(owner: "The assistant", date: nil) == "You can restore it from Jet Trash until it's removed for good. After that its working copy and Jet history are deleted. The assistant's own history isn't deleted.")

        await fake.setGraceDays(nil)
        let unknown = moveModel(fake)
        await unknown.load()
        #expect(unknown.restoreDate == nil)
        #expect(unknown.message?.contains("until it's removed for good") == true)
    }

    // MARK: 14. Selection after moving

    @Test
    func theNextTaskIsBelowThenAboveThenNewTask() {
        let a = UUID(), b = UUID(), c = UUID()
        #expect(DesktopSession.taskToSelect(afterRemoving: b, orderedIDs: [a, b, c]) == c)
        #expect(DesktopSession.taskToSelect(afterRemoving: c, orderedIDs: [a, b, c]) == b)
        #expect(DesktopSession.taskToSelect(afterRemoving: a, orderedIDs: [a]) == nil)
    }

    // MARK: 15. Repeat Daily

    @Test
    func scheduleCopyAndLocalTime() {
        let english = Locale(identifier: "en_US")
        let model = RepeatDailyModel(ref: ConversationRef(conversationID: UUID(), planeRegistryID: UUID()), makeAccess: { _ in LibraryAccessFake() })
        #expect(model.localTime == "09:00:00")
        model.hour = 17
        model.minute = 5
        #expect(model.localTime == "17:05:00")
        #expect(spaced(ScheduleCopy.summary(localTime: "09:00:00", timeZone: "Europe/Berlin", locale: english)) == "Every day at 9:00 AM (Berlin)")
        #expect(ScheduleCopy.city("America/New_York") == "New York")
        #expect(spaced(ScheduleCopy.savedNotice(localTime: "17:30:00", timeZone: "America/New_York", locale: english)) == "Repeats daily at 5:30 PM (New York).")
    }

    @Test
    func theTimeSurvivesTimeZoneChanges() {
        let model = RepeatDailyModel(ref: ConversationRef(conversationID: UUID(), planeRegistryID: UUID()), makeAccess: { _ in LibraryAccessFake() })
        let berlin = TimeZone(identifier: "Europe/Berlin")!
        let tokyo = TimeZone(identifier: "Asia/Tokyo")!
        model.setTime(model.time(in: berlin).addingTimeInterval(90 * 60), in: berlin)
        #expect(model.localTime == "10:30:00")
        model.timeZoneID = tokyo.identifier
        #expect(model.localTime == "10:30:00")
        model.setTime(model.time(in: tokyo), in: tokyo)
        #expect(model.localTime == "10:30:00")
    }

    @Test
    func repeatDailyLimits() async {
        let fake = LibraryAccessFake()
        let model = RepeatDailyModel(ref: ConversationRef(conversationID: fake.conversationID, planeRegistryID: UUID()), makeAccess: { _ in fake })
        await model.load()
        #expect(model.canSave == false)
        model.message = "Check CI"
        #expect(model.canSave)
        model.message = String(repeating: "a", count: RepeatDailyModel.maximumBytes + 1)
        #expect(model.canSave == false)
        #expect(model.showsByteCount)
        model.message = "Check CI"
        await fake.setScheduleCount(RepeatDailyModel.maximumSchedules, conversationID: fake.conversationID)
        await model.load()
        #expect(model.canSave == false)
    }

    @Test
    func anUncertainCreateIsConfirmedByANewMatchingSchedule() async {
        let fake = LibraryAccessFake()
        await fake.failCommand(0, with: .unknownApplied)
        let model = RepeatDailyModel(ref: ConversationRef(conversationID: fake.conversationID, planeRegistryID: UUID()), makeAccess: { _ in fake })
        await model.load()
        model.message = "Check CI"
        model.timeZoneID = "Europe/Berlin"
        #expect(await model.save() != nil)
        #expect(model.schedules.count == 1)

        await fake.failCommand(1, with: .unknownNotApplied)
        model.message = "Update the changelog"
        #expect(await model.save() == nil)
        #expect(model.issue?.action == .sendSameRequestAgain)
        #expect(await model.sendSameRequestAgain() != nil)
        let commands = await fake.commands
        #expect(commands.count == 3)
        #expect(commands[1].commandID == commands[2].commandID)
        #expect(commands[1].call == commands[2].call)
    }

    @Test
    func stopRepeatingSendsOneCancel() async {
        let fake = LibraryAccessFake()
        await fake.setScheduleCount(2, conversationID: fake.conversationID)
        let model = RepeatDailyModel(ref: ConversationRef(conversationID: fake.conversationID, planeRegistryID: UUID()), makeAccess: { _ in fake })
        await model.load()
        let schedule = model.schedules[0]
        model.pendingStop = schedule
        await model.confirmStop()
        #expect(await fake.commands.map(\.call) == [.cancelSchedule(schedule.id)])
        #expect(model.schedules.count == 1)
        #expect(model.schedules.contains { $0.id == schedule.id } == false)
        #expect(model.pendingStop == nil)
    }

    // MARK: 16. Project removal

    @Test
    func removalObstaclesReadAsSentences() {
        let preview = JetProjectRemovalPreview(
            projectID: UUID(), root: "/Users/alex/code/api-server", diskUseBytes: 1_024,
            liveRuns: 1, schedules: 2, dirtyFiles: 0, unpushedCommits: 0, workspaceCount: 1,
            obstacles: [ProjectRemovalSheet.liveRunsLabel, ProjectRemovalSheet.schedulesLabel, "Remove nested Projects first"],
            permanentWarning: "", binding: JetRawJSON(source: "{}")
        )
        #expect(ProjectRemovalSheet.obstacleSentences(preview) == [
            "An assistant is still working on one of its tasks. Stop it first.",
            "Some of its tasks repeat a message daily. Stop repeating first.",
            "Remove nested Projects first",
        ])
    }

    // MARK: 17. Copy

    @Test
    func casualLibraryCopyAvoidsJetWords() {
        for text in LibraryCopy.casual {
            #expect(JetCopy.foundAvoidWords(in: text).isEmpty, "\(text)")
        }
    }

    @Test
    func issuesMapToCasualCopy() {
        let offline = LibraryIssue.from(JetClientFailure.presentation(.offline), computer: "Studio Mac", isLocal: false)
        #expect(offline.text == "Can't reach Studio Mac. Try again when it's connected.")
        #expect(offline.action == .tryAgain)
        let recovery = LibraryIssue.from(
            JetClientFailure.presentation(JetPresentationError(category: .unavailable, code: "recovery.read_only", message: "", retryable: false)),
            computer: "This Mac", isLocal: true
        )
        #expect(recovery.text == "Jet paused changes to protect your data.")
        if case .review = recovery.action {} else { Issue.record("expected Review…") }
        let disk = LibraryIssue.from(
            JetClientFailure.presentation(JetPresentationError(category: .unavailable, code: "storage.disk_pressure", message: "", retryable: false)),
            computer: "This Mac", isLocal: true
        )
        #expect(disk.text == "Your Mac is almost out of disk space. Jet paused new work.")
        #expect(disk.action == nil)
        let other = LibraryIssue.from(JetClientFailure.presentation(.invalidResponse), computer: "This Mac", isLocal: true)
        #expect(other.text == "Jet couldn't finish this. Try again.")
    }

    // MARK: - Helpers

    static let now = Date(timeIntervalSince1970: 1_790_596_800)

    private func projectModel(_ fake: LibraryAccessFake) -> ReplyFinishModel {
        ReplyFinishModel(planeRegistryID: UUID(), scope: .project(fake.projectID), makeAccess: { _ in fake })
    }

    private func moveModel(
        _ fake: LibraryAccessFake,
        memory: ClientMemory? = nil,
        mode: MoveToTrashModel.Mode = .forget,
        sleep: @escaping @Sendable (Duration) async throws -> Void = { _ in }
    ) -> MoveToTrashModel {
        MoveToTrashModel(
            ref: ConversationRef(conversationID: fake.conversationID, planeRegistryID: UUID()),
            title: "Fix login redirect",
            assistantName: "Claude Code",
            mode: mode,
            makeAccess: { _ in fake },
            memory: memory ?? testMemory(),
            now: { Self.now },
            sleep: sleep
        )
    }

    private func testMemory() -> ClientMemory {
        ClientMemory(defaults: UserDefaults(suiteName: "jet.library-tests.\(UUID().uuidString)")!)
    }

    private func trashEntry(_ id: UUID, _ reason: String, day: Int) -> JetTrashEntry {
        JetTrashEntry(
            conversationID: id,
            reason: reason,
            trashedAt: Self.now.addingTimeInterval(TimeInterval(day) * 86_400 - 30 * 86_400),
            expiresAt: Self.now.addingTimeInterval(TimeInterval(day) * 86_400)
        )
    }

    private func summary(_ id: UUID, _ title: String, project: UUID?) -> JetConversationSummary {
        JetConversationSummary(id: id, revision: 1, title: title, createdAtUnixMilliseconds: 1, projectID: project)
    }

    private func run(_ id: UUID, _ conversationID: UUID, _ lifecycle: JetRunLifecycle) -> JetRunSummary {
        JetRunSummary(
            id: id, conversationID: conversationID, revision: 1, lifecycle: lifecycle,
            title: "Fix login redirect", createdAtUnixMilliseconds: 1, endedAtUnixMilliseconds: nil
        )
    }

    private func snapshot(_ conversationID: UUID, runs: [JetRunSummary]) -> JetConversationSnapshot {
        JetConversationSnapshot(
            cursor: 1,
            conversation: summary(conversationID, "Fix login redirect", project: nil),
            workspaceID: nil,
            workspaceRoot: nil,
            runs: runs
        )
    }

    /// Time formats may use a narrow no-break space before AM/PM.
    private func spaced(_ text: String) -> String {
        text.replacingOccurrences(of: "\u{202F}", with: " ")
    }
}

@MainActor
private final class TitleBox {
    var value: String?
}

private actor SleepCounter {
    private(set) var count = 0
    func tick() { count += 1 }
}

/// A recording library boundary. Settings resolve task → project → computer → built-in.
actor LibraryAccessFake: JetLibraryAccess {
    enum Call: Equatable, Sendable {
        case set(String, JetSettingValue, JetSettingScope)
        case clear(String, JetSettingScope)
        case stage(UUID, JetRetentionAction)
        case restore(UUID)
        case stop(UUID)
        case createSchedule(String, String, String)
        case cancelSchedule(UUID)
    }

    struct Command: Equatable, Sendable {
        let call: Call
        let commandID: UUID
    }

    enum Failure: Sendable {
        case refuse(JetPresentationError)
        /// The Command took effect but its outcome is unknown.
        case unknownApplied
        /// The Command didn't take effect and its outcome is unknown.
        case unknownNotApplied
    }

    nonisolated let projectID = UUID()
    nonisolated let conversationID = UUID()
    private(set) var commands: [Command] = []
    private var failures: [Int: Failure] = [:]
    private var projectValues: [String: JetSettingValue] = [:]
    private var conversationValues: [String: JetSettingValue] = [:]
    private var graceDays: UInt32? = 30
    private var recoveryState = "serving"
    private var trash: [JetTrashEntry] = []
    private var snapshots: [UUID: JetConversationSnapshot] = [:]
    private var initialProtections: [String] = []
    private var pollProtections: [[String]] = []
    private var previewReads = 0
    private var schedules: [JetScheduledTask] = []
    private var stageObserver: (@Sendable (UUID) async -> Void)?

    private static let builtIns: [(String, JetSettingValue)] = [
        ("git.auto_branch", .flag(false)), ("git.auto_commit", .flag(false)),
        ("git.auto_push", .flag(false)), ("git.auto_draft_pull_request", .flag(false)),
        ("git.branch_prefix", .text("jet/")), ("utility.automatic_naming", .flag(true)),
    ]

    func failCommand(_ index: Int, with failure: Failure) { failures[index] = failure }
    func setTrash(_ entries: [JetTrashEntry]) { trash = entries }
    func setRecoveryState(_ state: String) { recoveryState = state }
    func setGraceDays(_ days: UInt32?) { graceDays = days }
    func setSnapshot(_ snapshot: JetConversationSnapshot, for conversationID: UUID) { snapshots[conversationID] = snapshot }
    func setStageObserver(_ observer: @escaping @Sendable (UUID) async -> Void) { stageObserver = observer }

    /// Branch, commit, push and draft pull request, in policy order.
    func setProjectFlags(_ values: [Bool]) {
        for (key, value) in zip(["git.auto_branch", "git.auto_commit", "git.auto_push", "git.auto_draft_pull_request"], values) {
            projectValues[key] = .flag(value)
        }
    }

    /// The first preview Query answers `initial`; each later one takes the next of `polls`,
    /// then repeats the last.
    func setPreviews(initial: [String], polls: [[String]]) {
        initialProtections = initial
        pollProtections = polls
        previewReads = 0
    }

    func setScheduleCount(_ count: Int, conversationID: UUID) {
        schedules = (0 ..< count).map { index in
            JetScheduledTask(
                id: UUID(), conversationID: conversationID, timeZone: "Europe/Berlin",
                localTime: String(format: "%02d:00:00", index % 24), prompt: "Message \(index)",
                nextDueAtUnixMilliseconds: 1, nextIntendedLocal: ""
            )
        }
    }

    /// Records a Command and applies its failure, if any. Returns whether to apply it.
    private func record(_ call: Call, _ commandID: UUID) throws -> Bool {
        let index = commands.count
        commands.append(Command(call: call, commandID: commandID))
        switch failures.removeValue(forKey: index) {
        case let .refuse(error)?: throw JetClientFailure.presentation(error)
        case .unknownNotApplied?: throw JetClientFailure.commandOutcomeUnknown(commandID: commandID)
        case .unknownApplied?: return false
        case nil: return true
        }
    }

    private func unknownAfterApplying(_ call: Call, _ commandID: UUID, apply: () -> Void) throws {
        let complete = try record(call, commandID)
        apply()
        if !complete { throw JetClientFailure.commandOutcomeUnknown(commandID: commandID) }
    }

    // MARK: Queries

    func conversation(_ conversationID: UUID) async throws -> JetConversationSnapshot {
        guard let snapshot = snapshots[conversationID] else {
            throw JetClientFailure.presentation(JetPresentationError(
                category: .notFound, code: "conversation.not_found", message: "", retryable: false
            ))
        }
        return snapshot
    }

    func settings(scope: JetSettingScope) async throws -> JetSettingSnapshot {
        var settings = Self.builtIns.map { key, value -> JetResolvedSetting in
            let resolved: (JetSettingValue, JetSettingSource)
            switch scope {
            case .conversation:
                if let value = conversationValues[key] {
                    resolved = (value, .scope(scope))
                } else if let value = projectValues[key] {
                    resolved = (value, .scope(.project(projectID)))
                } else {
                    resolved = (value, .builtIn)
                }
            case .project:
                resolved = projectValues[key].map { ($0, .scope(scope)) } ?? (value, .builtIn)
            case .plane:
                resolved = (value, .builtIn)
            }
            return JetResolvedSetting(key: SettingKey(rawValue: key)!, value: resolved.0, source: resolved.1)
        }
        if let graceDays {
            settings.append(JetResolvedSetting(
                key: SettingKey(rawValue: "retention.trash_grace_days")!, value: .count(graceDays), source: .builtIn
            ))
        }
        return JetSettingSnapshot(cursor: UInt64(commands.count), scope: scope, settings: settings)
    }

    func scheduledTasks(conversationID: UUID) async throws -> JetScheduledTaskSnapshot {
        JetScheduledTaskSnapshot(cursor: 1, tasks: schedules.filter { $0.conversationID == conversationID })
    }

    func systemHealth() async throws -> JetSystemHealth {
        JetSystemHealth(
            planeID: UUID(), daemonVersion: "1.0", daemonStarts: 1,
            daemonStartedAt: Date(timeIntervalSince1970: 1), platform: "macOS",
            capabilitiesAvailable: true, externalTools: [], crafts: [],
            degradedCapabilities: [], credentialStore: .available,
            recoveryState: recoveryState, recoveryReason: nil, snapshots: [],
            deletionLedger: "verified", auditIntegrity: .trusted
        )
    }

    func conversationTrash() async throws -> JetTrashSnapshot {
        JetTrashSnapshot(cursor: 1, entries: trash)
    }

    func retentionPreview(conversationID: UUID) async throws -> JetRetentionPreview {
        let protections: [String]
        if previewReads == 0 || pollProtections.isEmpty {
            protections = previewReads == 0 ? initialProtections : (pollProtections.last ?? initialProtections)
        } else {
            protections = pollProtections[min(previewReads - 1, pollProtections.count - 1)]
        }
        previewReads += 1
        return JetRetentionPreview(
            conversationID: conversationID,
            protections: protections,
            auditRecords: 0,
            trash: trash.first { $0.conversationID == conversationID }
        )
    }

    // MARK: Commands

    func setSetting(_ key: SettingKey, value: JetSettingValue, scope: JetSettingScope, commandID: UUID) async throws -> JetSettingSet {
        try unknownAfterApplying(.set(key.rawValue, value, scope), commandID) {
            if case .conversation = scope { conversationValues[key.rawValue] = value } else { projectValues[key.rawValue] = value }
        }
        return JetSettingSet(key: key, scope: scope, value: value)
    }

    func clearSetting(_ key: SettingKey, scope: JetSettingScope, commandID: UUID) async throws -> JetSettingCleared {
        try unknownAfterApplying(.clear(key.rawValue, scope), commandID) {
            if case .conversation = scope { conversationValues[key.rawValue] = nil } else { projectValues[key.rawValue] = nil }
        }
        return JetSettingCleared(key: key, scope: scope)
    }

    func createSchedule(conversationID: UUID, timeZone: String, localTime: String, prompt: String, commandID: UUID) async throws -> JetScheduledTask {
        let task = JetScheduledTask(
            id: UUID(), conversationID: conversationID, timeZone: timeZone, localTime: localTime,
            prompt: prompt, nextDueAtUnixMilliseconds: 1, nextIntendedLocal: ""
        )
        try unknownAfterApplying(.createSchedule(timeZone, localTime, prompt), commandID) { schedules.append(task) }
        return task
    }

    func cancelSchedule(_ scheduleID: UUID, commandID: UUID) async throws {
        try unknownAfterApplying(.cancelSchedule(scheduleID), commandID) { schedules.removeAll { $0.id == scheduleID } }
    }

    func stageConversation(_ conversationID: UUID, action: JetRetentionAction, commandID: UUID) async throws -> JetTrashEntry {
        if let stageObserver { await stageObserver(conversationID) }
        let entry = JetTrashEntry(
            conversationID: conversationID,
            reason: action == .forget ? "manual_forget" : "delete_everywhere",
            trashedAt: Date(timeIntervalSince1970: 1),
            expiresAt: Date(timeIntervalSince1970: 2)
        )
        try unknownAfterApplying(.stage(conversationID, action), commandID) { trash.append(entry) }
        return entry
    }

    func restoreConversation(_ conversationID: UUID, commandID: UUID) async throws {
        try unknownAfterApplying(.restore(conversationID), commandID) {
            trash.removeAll { $0.conversationID == conversationID }
        }
    }

    func stopRun(runID: UUID, commandID: UUID) async throws {
        _ = try record(.stop(runID), commandID)
    }
}
