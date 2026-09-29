import Foundation
import Testing
@testable import jet

@MainActor
struct Wave33RecoveryTests {
    @Test
    func protectedTaskCannotBeForgottenButDeleteEverywhereCanBeStaged() async {
        let conversationID = UUID()
        let fake = RecoveryAccessFake(conversationID: conversationID, protections: ["active_run"])
        let model = JetRecoveryModel(makeAccess: { _ in fake })
        await model.load(planeRegistryID: UUID(), conversationID: conversationID)

        await model.stage(.forget)
        #expect(await fake.stagedActions.isEmpty)

        await model.stage(.deleteEverywhere)
        #expect(await fake.stagedActions == [.deleteEverywhere])
        #expect(model.trash?.entries.first?.reason == "delete_everywhere")
        #expect(model.preview?.trash != nil)
    }

    @Test
    func approvalUsesTheReviewedDaysAndDoesNotApproveACompilingRule() async {
        let conversationID = UUID()
        let fake = RecoveryAccessFake(conversationID: conversationID, protections: [])
        let model = JetRecoveryModel(makeAccess: { _ in fake })
        await model.load(planeRegistryID: UUID(), conversationID: conversationID)
        let draft = try! #require(model.autodelete?.rules.first)

        await model.approveRule(draft)
        #expect(await fake.approvedDays == [30])

        let compiling = JetAutodeleteRule(
            id: draft.id, prompt: draft.prompt, scope: "forget",
            state: .compiling, candidates: []
        )
        await model.approveRule(compiling)
        #expect(await fake.approvedDays == [30])
    }

    @Test
    func switchingPlanesClearsCachedAuditAndTrashBeforeLoadingTheNewPlane() async {
        let conversationID = UUID()
        let first = RecoveryAccessFake(conversationID: conversationID, protections: [])
        let second = RecoveryAccessFake(conversationID: UUID(), protections: [])
        var current: any JetRecoveryAccess = first
        let model = JetRecoveryModel(makeAccess: { _ in current })
        await model.load(planeRegistryID: UUID(), conversationID: conversationID)
        #expect(model.preview?.conversationID == conversationID)

        current = second
        await model.load(planeRegistryID: UUID(), conversationID: nil)
        #expect(model.preview == nil)
        #expect(model.trash?.entries.isEmpty == true)
        #expect(model.auditEntries.isEmpty)
    }

    @Test
    func recoveryCommandsRequireTheReportedState() async {
        let fake = RecoveryAccessFake(conversationID: UUID(), protections: [])
        let model = JetRecoveryModel(makeAccess: { _ in fake })
        await model.load(planeRegistryID: UUID(), conversationID: nil)
        let snapshot = JetRecoverySnapshot(
            name: "plane-1-daily.sqlite3", reason: "daily",
            takenAt: Date(timeIntervalSince1970: 1), bytes: 1_024
        )

        await model.restoreSnapshot(snapshot)
        #expect(await fake.restoredSnapshots.isEmpty)
        await model.purgeSnapshots()
        #expect(await fake.purgeCount == 1)
    }

    @Test
    func anInFlightCommandCannotReplaceAnotherPlanesPresentation() async {
        let firstPlane = UUID()
        let secondPlane = UUID()
        let conversationID = UUID()
        let first = RecoveryAccessFake(conversationID: conversationID, protections: [])
        let second = RecoveryAccessFake(conversationID: UUID(), protections: [])
        await first.holdNextStage()
        let model = JetRecoveryModel(makeAccess: { planeID in
            planeID == firstPlane ? first : second
        })
        await model.load(planeRegistryID: firstPlane, conversationID: conversationID)

        let stage = Task { await model.stage(.deleteEverywhere) }
        await first.waitForStageStart()
        await model.load(planeRegistryID: secondPlane, conversationID: nil)
        await first.releaseStage()
        await stage.value

        #expect(model.trash?.entries.isEmpty == true)
        #expect(model.selectedConversationID == nil)
        #expect(model.operation == nil)
        #expect(await first.stagedActions == [.deleteEverywhere])
        #expect(await second.stagedActions.isEmpty)
    }
}

@MainActor
struct Wave33CleanUpTests {
    @Test
    func cleanUpCompilesItsRuleThenSetsTheChosenDays() async throws {
        let fake = CleanUpAccessFake(compiles: .toDraft(45))
        let model = await loadedModel(fake)

        let result = await model.prepareCleanUp(days: 30)

        let compiled = await fake.compiled
        #expect(compiled.count == 1)
        #expect(compiled.first?.prompt == JetRecoveryModel.cleanUpPrompt(days: 30))
        #expect(compiled.first?.prompt == "Move tasks to Jet Trash after they have been idle for 30 days.")
        #expect(await fake.daysSet == [30])
        guard case let .ready(rule) = result else {
            Issue.record("Expected a rule ready for review, got \(result)")
            return
        }
        #expect(rule.state == .draft(days: 30))
        #expect(rule.id == compiled.first?.ruleID)
        #expect(model.managedCleanUpRule?.id == rule.id)
        #expect(model.createdCleanUpRuleIDs == [rule.id])
        #expect(model.operation == nil)

        await model.turnOnCleanUp(rule)
        #expect(await fake.approvedDays == [30])
        #expect(model.createdCleanUpRuleIDs.isEmpty)
        #expect(model.notice == "Clean up updated.")
    }

    @Test
    func aRuleThatAlreadyHasTheDaysIsNotChangedAgain() async {
        let fake = CleanUpAccessFake(compiles: .toDraft(30))
        let model = await loadedModel(fake)

        let result = await model.prepareCleanUp(days: 30)

        #expect(await fake.daysSet.isEmpty)
        guard case let .ready(rule) = result else {
            Issue.record("Expected a ready rule")
            return
        }
        #expect(rule.state == .draft(days: 30))
    }

    @Test
    func aRefusedCompileGetsItsDaysByHand() async {
        let fake = CleanUpAccessFake(compiles: .refused)
        let model = await loadedModel(fake)

        let result = await model.prepareCleanUp(days: 14)

        #expect(await fake.daysSet == [14])
        guard case let .ready(rule) = result else {
            Issue.record("Expected a ready rule")
            return
        }
        #expect(rule.state == .draft(days: 14))
    }

    @Test
    func aRuleStillCompilingAfterTwentyChecksIsNeverEdited() async {
        let fake = CleanUpAccessFake(compiles: .never)
        let model = await loadedModel(fake)
        let readsBefore = await fake.ruleReads

        let result = await model.prepareCleanUp(days: 30)

        #expect(result == .stillChecking)
        #expect(await fake.daysSet.isEmpty)
        #expect(await fake.approvedDays.isEmpty)
        // One read before compiling, one after, then twenty checks.
        #expect(await fake.ruleReads == readsBefore + 22)
        #expect(model.operation == nil)
    }

    @Test
    func anUnknownCompileKeepsItsCommandAndRuleIDsForTheExplicitRetry() async {
        let fake = CleanUpAccessFake(compiles: .toDraft(30))
        let model = await loadedModel(fake)
        await fake.failNextCompile(.commandOutcomeUnknown(commandID: UUID()))

        let first = await model.prepareCleanUp(days: 30)
        #expect(first == .failed)
        #expect(model.notice == "Jet couldn't confirm this change. Check the current state before trying again.")
        // Nothing was sent again on its own.
        #expect(await fake.compiled.count == 1)

        let retry = await model.prepareCleanUp(days: 30)
        let compiled = await fake.compiled
        #expect(compiled.count == 2)
        #expect(compiled[0].commandID == compiled[1].commandID)
        #expect(compiled[0].ruleID == compiled[1].ruleID)
        guard case .ready = retry else {
            Issue.record("Expected the retry to finish")
            return
        }

        // A later, different clean-up compiles as a new Command.
        #expect(model.commandIDs.isEmpty)
        #expect(model.compileRuleIDs.isEmpty)
    }

    @Test
    func changingAnApprovedRuleSetsDaysWithoutCompiling() async {
        let ruleID = UUID()
        let approved = JetAutodeleteRule(
            id: ruleID,
            prompt: JetRecoveryModel.cleanUpPrompt(days: 30),
            scope: "forget",
            state: .approved(days: 30, at: Date(timeIntervalSince1970: 1)),
            candidates: []
        )
        let fake = CleanUpAccessFake(rules: [approved])
        let model = await loadedModel(fake)

        let same = await model.prepareCleanUp(days: 30)
        #expect(same == .ready(approved))
        #expect(await fake.daysSet.isEmpty)

        let changed = await model.prepareCleanUp(days: 60)
        #expect(await fake.compiled.isEmpty)
        #expect(await fake.daysSet == [60])
        guard case let .ready(rule) = changed else {
            Issue.record("Expected a ready rule")
            return
        }
        #expect(rule.state == .draft(days: 60))
    }

    @Test
    func cancelDiscardsOnlyADraftThisModelCreated() async {
        // A rule that was already there stays.
        let existing = JetAutodeleteRule(
            id: UUID(),
            prompt: JetRecoveryModel.cleanUpPrompt(days: 30),
            scope: "forget",
            state: .draft(days: 30),
            candidates: []
        )
        let kept = CleanUpAccessFake(rules: [existing])
        let keptModel = await loadedModel(kept)
        _ = await keptModel.prepareCleanUp(days: 30)
        await keptModel.discardCleanUpDraft()
        #expect(await kept.deletedRuleIDs.isEmpty)

        // A draft this sheet compiled goes away.
        let fresh = CleanUpAccessFake(compiles: .toDraft(30))
        let freshModel = await loadedModel(fresh)
        guard case let .ready(rule) = await freshModel.prepareCleanUp(days: 30) else {
            Issue.record("Expected a ready rule")
            return
        }
        await freshModel.discardCleanUpDraft()
        #expect(await fresh.deletedRuleIDs == [rule.id])
        #expect(freshModel.managedCleanUpRule == nil)

        // Once turned on, Cancel leaves it on.
        let approved = CleanUpAccessFake(compiles: .toDraft(30))
        let approvedModel = await loadedModel(approved)
        guard case let .ready(onRule) = await approvedModel.prepareCleanUp(days: 30) else {
            Issue.record("Expected a ready rule")
            return
        }
        await approvedModel.turnOnCleanUp(onRule)
        await approvedModel.discardCleanUpDraft()
        #expect(await approved.deletedRuleIDs.isEmpty)
    }

    @Test
    func customRulesAreEveryRuleButTheManagedOne() async {
        let custom = JetAutodeleteRule(
            id: UUID(), prompt: "Forget experiments after two weeks", scope: "forget",
            state: .draft(days: 14), candidates: []
        )
        let managed = JetAutodeleteRule(
            id: UUID(), prompt: JetRecoveryModel.cleanUpPrompt(days: 30), scope: "forget",
            state: .approved(days: 30, at: Date(timeIntervalSince1970: 1)), candidates: []
        )
        let fake = CleanUpAccessFake(rules: [custom, managed])
        let model = await loadedModel(fake)

        #expect(model.managedCleanUpRule == managed)
        #expect(model.customCleanUpRules == [custom])
    }

    private func loadedModel(_ fake: CleanUpAccessFake) async -> JetRecoveryModel {
        let model = JetRecoveryModel(makeAccess: { _ in fake }, sleep: { _ in })
        await model.load(planeRegistryID: UUID(), conversationID: nil)
        return model
    }
}

/// Clean-up rules on one computer. A compiled rule stays compiling for two
/// reads, then becomes what `compiles` says.
private actor CleanUpAccessFake: JetRecoveryAccess {
    enum Compiles { case toDraft(UInt32), refused, never }

    struct Compiled: Sendable {
        let ruleID: UUID
        let prompt: String
        let commandID: UUID
    }

    private let compiles: Compiles
    private var readsUntilCompiled = 0
    private var nextCompileFailure: JetClientFailure?
    private(set) var rules: [JetAutodeleteRule]
    private(set) var compiled: [Compiled] = []
    private(set) var daysSet: [UInt32] = []
    private(set) var approvedDays: [UInt32] = []
    private(set) var deletedRuleIDs: [UUID] = []
    private(set) var ruleReads = 0

    init(rules: [JetAutodeleteRule] = [], compiles: Compiles = .toDraft(30)) {
        self.rules = rules
        self.compiles = compiles
    }

    func failNextCompile(_ failure: JetClientFailure) {
        nextCompileFailure = failure
    }

    func autodeleteRules() async throws -> JetAutodeleteSnapshot {
        ruleReads += 1
        if let index = rules.firstIndex(where: { $0.state == .compiling }) {
            readsUntilCompiled -= 1
            if readsUntilCompiled <= 0 {
                switch compiles {
                case let .toDraft(days): rules[index] = rule(rules[index], state: .draft(days: days))
                case .refused: rules[index] = rule(rules[index], state: .refused("utility.disabled"))
                case .never: break
                }
            }
        }
        return JetAutodeleteSnapshot(cursor: UInt64(ruleReads), rules: rules)
    }

    func compileAutodeleteRule(ruleID: UUID, prompt: String, commandID: UUID) async throws {
        compiled.append(Compiled(ruleID: ruleID, prompt: prompt, commandID: commandID))
        if let failure = nextCompileFailure {
            nextCompileFailure = nil
            throw failure
        }
        rules.append(JetAutodeleteRule(id: ruleID, prompt: prompt, scope: "forget", state: .compiling, candidates: []))
        readsUntilCompiled = 2
    }

    func setAutodeleteDays(ruleID: UUID, days: UInt32, commandID: UUID) async throws {
        daysSet.append(days)
        guard let index = rules.firstIndex(where: { $0.id == ruleID }) else { return }
        rules[index] = rule(rules[index], state: .draft(days: days))
    }

    func approveAutodelete(ruleID: UUID, days: UInt32, commandID: UUID) async throws {
        approvedDays.append(days)
        guard let index = rules.firstIndex(where: { $0.id == ruleID }) else { return }
        rules[index] = rule(rules[index], state: .approved(days: days, at: Date(timeIntervalSince1970: 2)))
    }

    func authorizeAutodeleteEverywhere(ruleID: UUID, commandID: UUID) async throws {}

    func deleteAutodeleteRule(ruleID: UUID, commandID: UUID) async throws {
        deletedRuleIDs.append(ruleID)
        rules.removeAll { $0.id == ruleID }
    }

    func systemHealth() async throws -> JetSystemHealth {
        JetSystemHealth(
            planeID: UUID(), daemonVersion: "1.0", daemonStarts: 1,
            daemonStartedAt: Date(timeIntervalSince1970: 1), platform: "macOS",
            capabilitiesAvailable: true, externalTools: [], crafts: [],
            degradedCapabilities: [], credentialStore: .available,
            recoveryState: "serving", recoveryReason: nil, snapshots: [],
            deletionLedger: "verified", auditIntegrity: .trusted
        )
    }

    func conversationTrash() async throws -> JetTrashSnapshot {
        JetTrashSnapshot(cursor: 1, entries: [])
    }

    func retentionPreview(conversationID: UUID) async throws -> JetRetentionPreview {
        JetRetentionPreview(conversationID: conversationID, protections: [], auditRecords: 0, trash: nil)
    }

    func stageConversation(_ conversationID: UUID, action: JetRetentionAction, commandID: UUID) async throws -> JetTrashEntry {
        JetTrashEntry(
            conversationID: conversationID, reason: "manual_forget",
            trashedAt: Date(timeIntervalSince1970: 1), expiresAt: Date(timeIntervalSince1970: 2)
        )
    }

    func restoreConversation(_ conversationID: UUID, commandID: UUID) async throws {}

    func securityAudit(after sequence: UInt64) async throws -> JetAuditPage {
        JetAuditPage(cursor: 0, entries: [])
    }

    func restoreRecoverySnapshot(_ name: String, commandID: UUID) async throws {}

    func purgeRecoverySnapshots(commandID: UUID) async throws -> [String] { [] }

    private func rule(_ rule: JetAutodeleteRule, state: JetAutodeleteState) -> JetAutodeleteRule {
        JetAutodeleteRule(id: rule.id, prompt: rule.prompt, scope: rule.scope, state: state, candidates: rule.candidates)
    }
}

private actor RecoveryAccessFake: JetRecoveryAccess {
    let conversationID: UUID
    let protections: [String]
    let ruleID = UUID()
    var stagedActions: [JetRetentionAction] = []
    var approvedDays: [UInt32] = []
    var restoredSnapshots: [String] = []
    var purgeCount = 0
    var trashEntries: [JetTrashEntry] = []
    private var stageIsHeld = false
    private var stageDidStart = false
    private var stageStarted: CheckedContinuation<Void, Never>?
    private var stageRelease: CheckedContinuation<Void, Never>?

    init(conversationID: UUID, protections: [String]) {
        self.conversationID = conversationID
        self.protections = protections
    }

    func systemHealth() async throws -> JetSystemHealth {
        JetSystemHealth(
            planeID: UUID(), daemonVersion: "1.0", daemonStarts: 1,
            daemonStartedAt: Date(timeIntervalSince1970: 1), platform: "macOS",
            capabilitiesAvailable: true, externalTools: [], crafts: [],
            degradedCapabilities: [], credentialStore: .available,
            recoveryState: "serving", recoveryReason: nil, snapshots: [],
            deletionLedger: "verified", auditIntegrity: .trusted
        )
    }

    func conversationTrash() async throws -> JetTrashSnapshot {
        JetTrashSnapshot(cursor: 1, entries: trashEntries)
    }

    func retentionPreview(conversationID: UUID) async throws -> JetRetentionPreview {
        JetRetentionPreview(
            conversationID: conversationID, protections: protections,
            auditRecords: 2,
            trash: trashEntries.first(where: { $0.conversationID == conversationID })
        )
    }

    func stageConversation(_ conversationID: UUID, action: JetRetentionAction, commandID: UUID) async throws -> JetTrashEntry {
        if stageIsHeld {
            stageDidStart = true
            stageStarted?.resume()
            stageStarted = nil
            await withCheckedContinuation { stageRelease = $0 }
            stageIsHeld = false
        }
        stagedActions.append(action)
        let entry = JetTrashEntry(
            conversationID: conversationID,
            reason: action == .forget ? "manual_forget" : "delete_everywhere",
            trashedAt: Date(timeIntervalSince1970: 1),
            expiresAt: Date(timeIntervalSince1970: 10)
        )
        trashEntries.append(entry)
        return entry
    }

    func holdNextStage() { stageIsHeld = true }

    func waitForStageStart() async {
        if stageDidStart { return }
        await withCheckedContinuation { stageStarted = $0 }
    }

    func releaseStage() {
        stageRelease?.resume()
        stageRelease = nil
    }

    func restoreConversation(_ conversationID: UUID, commandID: UUID) async throws {
        trashEntries.removeAll { $0.conversationID == conversationID }
    }

    func autodeleteRules() async throws -> JetAutodeleteSnapshot {
        JetAutodeleteSnapshot(cursor: 1, rules: [JetAutodeleteRule(
            id: ruleID, prompt: "Forget after a month", scope: "forget",
            state: .draft(days: 30), candidates: []
        )])
    }

    func compileAutodeleteRule(ruleID: UUID, prompt: String, commandID: UUID) async throws {}
    func setAutodeleteDays(ruleID: UUID, days: UInt32, commandID: UUID) async throws {}
    func approveAutodelete(ruleID: UUID, days: UInt32, commandID: UUID) async throws {
        approvedDays.append(days)
    }
    func authorizeAutodeleteEverywhere(ruleID: UUID, commandID: UUID) async throws {}
    func deleteAutodeleteRule(ruleID: UUID, commandID: UUID) async throws {}
    func securityAudit(after sequence: UInt64) async throws -> JetAuditPage {
        JetAuditPage(cursor: 0, entries: [])
    }
    func restoreRecoverySnapshot(_ name: String, commandID: UUID) async throws {
        restoredSnapshots.append(name)
    }
    func purgeRecoverySnapshots(commandID: UUID) async throws -> [String] {
        purgeCount += 1
        return []
    }
}
