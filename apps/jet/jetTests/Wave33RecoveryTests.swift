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
