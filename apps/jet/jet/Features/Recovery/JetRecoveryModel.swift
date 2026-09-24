import Foundation
import Observation

nonisolated protocol JetRecoveryAccess: Sendable {
    func systemHealth() async throws -> JetSystemHealth
    func conversationTrash() async throws -> JetTrashSnapshot
    func retentionPreview(conversationID: UUID) async throws -> JetRetentionPreview
    func stageConversation(_ conversationID: UUID, action: JetRetentionAction, commandID: UUID) async throws -> JetTrashEntry
    func restoreConversation(_ conversationID: UUID, commandID: UUID) async throws
    func autodeleteRules() async throws -> JetAutodeleteSnapshot
    func compileAutodeleteRule(ruleID: UUID, prompt: String, commandID: UUID) async throws
    func setAutodeleteDays(ruleID: UUID, days: UInt32, commandID: UUID) async throws
    func approveAutodelete(ruleID: UUID, days: UInt32, commandID: UUID) async throws
    func authorizeAutodeleteEverywhere(ruleID: UUID, commandID: UUID) async throws
    func deleteAutodeleteRule(ruleID: UUID, commandID: UUID) async throws
    func securityAudit(after sequence: UInt64) async throws -> JetAuditPage
    func restoreRecoverySnapshot(_ name: String, commandID: UUID) async throws
    func purgeRecoverySnapshots(commandID: UUID) async throws -> [String]
}

extension JetClient: JetRecoveryAccess {}
typealias JetRecoveryAccessProvider = @MainActor (UUID) async throws -> any JetRecoveryAccess

nonisolated enum JetRecoveryArea: String, Sendable, Hashable {
    case health, trash, preview, autodelete, audit
}

@MainActor
@Observable
final class JetRecoveryModel {
    private let makeAccess: JetRecoveryAccessProvider
    private let onConversationChange: @MainActor (_ staged: Bool) async -> Void
    private var generation = 0
    private var planeRegistryID: UUID?
    private var auditAfter: UInt64 = 0
    private var auditFence: UInt64?

    var health: JetSystemHealth?
    var trash: JetTrashSnapshot?
    var preview: JetRetentionPreview?
    var autodelete: JetAutodeleteSnapshot?
    var auditEntries: [JetAuditEntry] = []
    var issues: [JetRecoveryArea: JetPresentationError] = [:]
    var isLoading = false
    var isLoadingAudit = false
    var operation: String?
    var notice: String?
    var selectedConversationID: UUID?
    var rulePrompt = ""
    var ruleDays: [UUID: String] = [:]

    init(
        makeAccess: @escaping JetRecoveryAccessProvider,
        onConversationChange: @escaping @MainActor (_ staged: Bool) async -> Void = { _ in }
    ) {
        self.makeAccess = makeAccess
        self.onConversationChange = onConversationChange
    }

    var hasMoreAudit: Bool {
        guard let auditFence else { return false }
        return auditAfter < auditFence
    }

    func load(planeRegistryID: UUID, conversationID: UUID?) async {
        generation += 1
        let current = generation
        operation = nil
        if self.planeRegistryID != planeRegistryID {
            self.planeRegistryID = planeRegistryID
            health = nil
            trash = nil
            preview = nil
            autodelete = nil
            auditEntries = []
            auditAfter = 0
            auditFence = nil
            issues = [:]
            operation = nil
            rulePrompt = ""
            ruleDays = [:]
        }
        selectedConversationID = conversationID
        if preview?.conversationID != conversationID { preview = nil }
        isLoading = true
        notice = nil
        do {
            let access = try await makeAccess(planeRegistryID)
            await loadHealth(access, generation: current)
            await loadTrash(access, generation: current)
            await loadAutodelete(access, generation: current)
            if let conversationID {
                await loadPreview(access, conversationID: conversationID, generation: current)
            } else if current == generation {
                issues.removeValue(forKey: .preview)
            }
            await loadAudit(access, after: 0, generation: current)
        } catch {
            guard current == generation else { return }
            let issue = presentationError(error)
            for area in [JetRecoveryArea.health, .trash, .autodelete, .audit] {
                issues[area] = issue
            }
            if conversationID != nil { issues[.preview] = issue }
        }
        if current == generation { isLoading = false }
    }

    func refresh() async {
        guard let planeRegistryID else { return }
        await load(planeRegistryID: planeRegistryID, conversationID: selectedConversationID)
    }

    func loadMoreAudit() async {
        guard !isLoadingAudit, hasMoreAudit else { return }
        let current = generation
        let after = auditAfter
        do {
            let access = try await activeAccess()
            await loadAudit(access, after: after, generation: current)
        } catch {
            if current == generation { issues[.audit] = presentationError(error) }
        }
    }

    func stage(_ action: JetRetentionAction) async {
        guard operation == nil, let conversationID = selectedConversationID,
              let preview, preview.conversationID == conversationID,
              preview.trash == nil, issues[.preview] == nil
        else { return }
        if action == .forget && preview.blocksForget { return }
        let current = generation
        operation = action.rawValue
        notice = nil
        defer { if current == generation { operation = nil } }
        do {
            let access = try await activeAccess()
            _ = try await access.stageConversation(conversationID, action: action, commandID: UUID())
            guard current == generation else { return }
            await loadTrash(access, generation: current)
            await loadPreview(access, conversationID: conversationID, generation: current)
            guard current == generation else { return }
            notice = "The task is in Jet Trash until its shown expiry."
            await onConversationChange(true)
        } catch {
            guard current == generation else { return }
            issues[.preview] = presentationError(error)
            await refreshAfterUncertain(error)
        }
    }

    func restore(_ conversationID: UUID) async {
        guard operation == nil,
              trash?.entries.contains(where: { $0.conversationID == conversationID && $0.canRestore }) == true,
              issues[.trash] == nil
        else { return }
        let current = generation
        operation = "restore-conversation"
        notice = nil
        defer { if current == generation { operation = nil } }
        do {
            let access = try await activeAccess()
            try await access.restoreConversation(conversationID, commandID: UUID())
            guard current == generation else { return }
            await loadTrash(access, generation: current)
            if selectedConversationID == conversationID {
                await loadPreview(access, conversationID: conversationID, generation: current)
            }
            guard current == generation else { return }
            notice = "Restored the task from Jet Trash."
            await onConversationChange(false)
        } catch {
            guard current == generation else { return }
            issues[.trash] = presentationError(error)
            await refreshAfterUncertain(error)
        }
    }

    func compileRule() async {
        guard operation == nil, !rulePrompt.isEmpty, rulePrompt.utf8.count <= 4_096 else { return }
        let current = generation
        let prompt = rulePrompt
        operation = "compile-rule"
        defer { if current == generation { operation = nil } }
        do {
            let access = try await activeAccess()
            try await access.compileAutodeleteRule(ruleID: UUID(), prompt: prompt, commandID: UUID())
            guard current == generation else { return }
            if rulePrompt == prompt { rulePrompt = "" }
            await loadAutodelete(access, generation: current)
            notice = "Jet is compiling the rule. Review its interpretation before approval."
        } catch {
            guard current == generation else { return }
            issues[.autodelete] = presentationError(error)
            await refreshAfterUncertain(error)
        }
    }

    func setRuleDays(_ rule: JetAutodeleteRule) async {
        guard issues[.autodelete] == nil,
              let days = UInt32(ruleDays[rule.id] ?? rule.state.days.map(String.init) ?? ""),
              (1 ... 36_500).contains(days)
        else { return }
        await changeRule("edit-rule") { access, commandID in
            try await access.setAutodeleteDays(ruleID: rule.id, days: days, commandID: commandID)
        }
    }

    func approveRule(_ rule: JetAutodeleteRule) async {
        guard issues[.autodelete] == nil,
              case let .draft(days) = rule.state
        else { return }
        await changeRule("approve-rule") { access, commandID in
            try await access.approveAutodelete(ruleID: rule.id, days: days, commandID: commandID)
        }
    }

    func authorizeEverywhere(_ rule: JetAutodeleteRule) async {
        guard issues[.autodelete] == nil,
              case .approved = rule.state, rule.scope == "forget"
        else { return }
        await changeRule("authorize-everywhere") { access, commandID in
            try await access.authorizeAutodeleteEverywhere(ruleID: rule.id, commandID: commandID)
        }
    }

    func deleteRule(_ rule: JetAutodeleteRule) async {
        guard issues[.autodelete] == nil else { return }
        await changeRule("delete-rule") { access, commandID in
            try await access.deleteAutodeleteRule(ruleID: rule.id, commandID: commandID)
        }
    }

    func restoreSnapshot(_ snapshot: JetRecoverySnapshot) async {
        guard operation == nil, health?.recoveryState == "read_only",
              health?.snapshots.contains(snapshot) == true,
              issues[.health] == nil
        else { return }
        let current = generation
        operation = "restore-snapshot"
        defer { if current == generation { operation = nil } }
        do {
            let access = try await activeAccess()
            try await access.restoreRecoverySnapshot(snapshot.name, commandID: UUID())
            guard current == generation else { return }
            await loadHealth(access, generation: current)
            notice = "The Plane reopened from the selected snapshot. Review its audit state."
        } catch {
            guard current == generation else { return }
            issues[.health] = presentationError(error)
            await refreshAfterUncertain(error)
        }
    }

    func purgeSnapshots() async {
        guard operation == nil, health?.recoveryState == "serving",
              health?.auditIntegrity == .trusted,
              issues[.health] == nil
        else { return }
        let current = generation
        operation = "purge-snapshots"
        defer { if current == generation { operation = nil } }
        do {
            let access = try await activeAccess()
            let removed = try await access.purgeRecoverySnapshots(commandID: UUID())
            guard current == generation else { return }
            await loadHealth(access, generation: current)
            notice = "Jet created a new snapshot and removed \(removed.count) older snapshots."
        } catch {
            guard current == generation else { return }
            issues[.health] = presentationError(error)
            await refreshAfterUncertain(error)
        }
    }

    private func changeRule(
        _ name: String,
        action: (any JetRecoveryAccess, UUID) async throws -> Void
    ) async {
        guard operation == nil else { return }
        let current = generation
        operation = name
        notice = nil
        defer { if current == generation { operation = nil } }
        do {
            let access = try await activeAccess()
            try await action(access, UUID())
            guard current == generation else { return }
            await loadAutodelete(access, generation: current)
            notice = "The Plane updated the rule."
        } catch {
            guard current == generation else { return }
            issues[.autodelete] = presentationError(error)
            await refreshAfterUncertain(error)
        }
    }

    private func refreshAfterUncertain(_ error: Error) async {
        guard case JetClientFailure.commandOutcomeUnknown = error else { return }
        operation = nil
        await refresh()
        notice = "The command outcome is unknown. Check the refreshed Plane state before another action."
    }

    private func loadHealth(_ access: any JetRecoveryAccess, generation current: Int) async {
        do {
            let value = try await access.systemHealth()
            guard current == generation else { return }
            health = value
            issues.removeValue(forKey: .health)
        } catch {
            if current == generation { issues[.health] = presentationError(error) }
        }
    }

    private func loadTrash(_ access: any JetRecoveryAccess, generation current: Int) async {
        do {
            let value = try await access.conversationTrash()
            guard current == generation else { return }
            trash = value
            issues.removeValue(forKey: .trash)
        } catch {
            if current == generation { issues[.trash] = presentationError(error) }
        }
    }

    private func loadPreview(
        _ access: any JetRecoveryAccess,
        conversationID: UUID,
        generation current: Int
    ) async {
        do {
            let value = try await access.retentionPreview(conversationID: conversationID)
            guard current == generation, selectedConversationID == conversationID else { return }
            preview = value
            issues.removeValue(forKey: .preview)
        } catch {
            if current == generation { issues[.preview] = presentationError(error) }
        }
    }

    private func loadAutodelete(_ access: any JetRecoveryAccess, generation current: Int) async {
        do {
            let value = try await access.autodeleteRules()
            guard current == generation else { return }
            autodelete = value
            issues.removeValue(forKey: .autodelete)
        } catch {
            if current == generation { issues[.autodelete] = presentationError(error) }
        }
    }

    private func loadAudit(
        _ access: any JetRecoveryAccess,
        after: UInt64,
        generation current: Int
    ) async {
        isLoadingAudit = true
        defer { isLoadingAudit = false }
        do {
            let page = try await access.securityAudit(after: after)
            guard current == generation else { return }
            if after == 0 { auditEntries = page.entries }
            else { auditEntries.append(contentsOf: page.entries) }
            auditAfter = page.entries.last?.sequence ?? after
            auditFence = page.entries.isEmpty ? auditAfter : page.cursor
            issues.removeValue(forKey: .audit)
        } catch {
            if current == generation { issues[.audit] = presentationError(error) }
        }
    }

    private func presentationError(_ error: Error) -> JetPresentationError {
        switch error {
        case let JetClientFailure.presentation(error): error
        case let JetClientFailure.commandOutcomeUnknown(commandID):
            JetPresentationError(
                category: .outcomeUnknown,
                code: "command.outcome_unknown",
                message: "Jet could not confirm the change. Command \(commandID.uuidString.prefix(8)).",
                retryable: false
            )
        default: .invalidResponse
        }
    }

    private func activeAccess() async throws -> any JetRecoveryAccess {
        guard let planeRegistryID else { throw JetClientFailure.presentation(.offline) }
        return try await makeAccess(planeRegistryID)
    }
}
