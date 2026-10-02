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
    /// One recovery Command, named by what it asks for. Its Command ID is kept
    /// while the outcome is unknown, so the person's explicit retry replays the
    /// same Command. Jet never retries on its own.
    enum RecoveryIntent: Hashable, Sendable {
        case stage(conversationID: UUID, action: JetRetentionAction)
        case restore(conversationID: UUID)
        /// Compiling also keeps the new rule's ID, so a retry names the same rule.
        case compile(prompt: String)
        case setDays(ruleID: UUID, days: UInt32)
        case approve(ruleID: UUID, days: UInt32)
        case authorize(ruleID: UUID)
        case deleteRule(ruleID: UUID)
        case restoreSnapshot(name: String)
        case purge
    }

    /// An intent on one computer.
    struct IntentKey: Hashable, Sendable {
        let planeRegistryID: UUID?
        let intent: RecoveryIntent
    }

    /// Where preparing the clean-up rule ended.
    enum CleanUpPreparation: Equatable, Sendable {
        /// The rule has the chosen days and can be reviewed; its candidates are current.
        case ready(JetAutodeleteRule)
        /// Jet is still compiling the rule. Nothing was changed.
        case stillChecking
        /// See `issues[.autodelete]`.
        case failed
    }

    private let makeAccess: JetRecoveryAccessProvider
    private let onConversationChange: @MainActor (_ staged: Bool) async -> Void
    private let onSnapshotRestored: @MainActor () async -> Void
    private let sleep: @Sendable (Duration) async throws -> Void
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

    /// Command IDs under the rule in `RecoveryIntent`.
    var commandIDs: [IntentKey: UUID] = [:]
    /// Rule IDs of compile Commands whose ID is kept.
    var compileRuleIDs: [IntentKey: UUID] = [:]
    /// Clean-up rules this model compiled and that aren't turned on yet. Only
    /// these can be discarded.
    var createdCleanUpRuleIDs: Set<UUID> = []

    init(
        makeAccess: @escaping JetRecoveryAccessProvider,
        onConversationChange: @escaping @MainActor (_ staged: Bool) async -> Void = { _ in },
        onSnapshotRestored: @escaping @MainActor () async -> Void = {},
        sleep: @escaping @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
    ) {
        self.makeAccess = makeAccess
        self.onConversationChange = onConversationChange
        self.onSnapshotRestored = onSnapshotRestored
        self.sleep = sleep
    }

    var hasMoreAudit: Bool {
        guard let auditFence else { return false }
        return auditAfter < auditFence
    }

    /// The store is read-only until a backup is restored.
    var isReadOnly: Bool { health?.recoveryState == "read_only" }

    // MARK: - Loading

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

    /// Reloads only the clean-up rules.
    func refreshAutodelete() async {
        let current = generation
        do {
            let access = try await activeAccess()
            await loadAutodelete(access, generation: current)
        } catch {
            if current == generation { issues[.autodelete] = presentationError(error) }
        }
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

    // MARK: - Jet Trash

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
            _ = try await withCommandID(.stage(conversationID: conversationID, action: action)) { commandID in
                try await access.stageConversation(conversationID, action: action, commandID: commandID)
            }
            guard current == generation else { return }
            await loadTrash(access, generation: current)
            await loadPreview(access, conversationID: conversationID, generation: current)
            guard current == generation else { return }
            notice = String(localized: "Moved to Jet Trash.")
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
            try await withCommandID(.restore(conversationID: conversationID)) { commandID in
                try await access.restoreConversation(conversationID, commandID: commandID)
            }
            guard current == generation else { return }
            await loadTrash(access, generation: current)
            if selectedConversationID == conversationID {
                await loadPreview(access, conversationID: conversationID, generation: current)
            }
            guard current == generation else { return }
            notice = String(localized: "Restored from Jet Trash.")
            await onConversationChange(false)
        } catch {
            guard current == generation else { return }
            issues[.trash] = presentationError(error)
            await refreshAfterUncertain(error)
        }
    }

    // MARK: - Clean up

    /// The prompt of the clean-up rule Jet manages. Its prefix identifies the
    /// rule, so the text is never localized.
    nonisolated static func cleanUpPrompt(days: UInt32) -> String {
        cleanUpPromptPrefix + "\(days) days."
    }

    nonisolated static let cleanUpPromptPrefix = "Move tasks to Jet Trash after they have been idle for "

    /// The clean-up rule Settings › Tasks manages: the first rule with Jet's prompt.
    var managedCleanUpRule: JetAutodeleteRule? {
        autodelete?.rules.first { $0.prompt.hasPrefix(Self.cleanUpPromptPrefix) }
    }

    /// Every other rule, shown under Custom Rules.
    var customCleanUpRules: [JetAutodeleteRule] {
        let managedID = managedCleanUpRule?.id
        return (autodelete?.rules ?? []).filter { $0.id != managedID }
    }

    /// Gets the managed rule ready for review with `days`. Without a rule, it
    /// compiles one and waits up to 20 checks, a second apart, for the compile to
    /// finish. It then sets the days unless the rule already has them. A rule that
    /// is still compiling is never edited.
    func prepareCleanUp(days: UInt32) async -> CleanUpPreparation {
        guard operation == nil, (1 ... 36_500).contains(days) else { return .failed }
        let current = generation
        operation = "prepare-cleanup"
        notice = nil
        defer { if current == generation { operation = nil } }
        do {
            let access = try await activeAccess()
            let rules = try await access.autodeleteRules()
            guard current == generation else { return .failed }
            autodelete = rules
            issues.removeValue(forKey: .autodelete)

            let ruleID: UUID
            if let managed = managedCleanUpRule {
                ruleID = managed.id
            } else {
                ruleID = try await compileCleanUpRule(days: days, access: access)
                guard current == generation else { return .failed }
                await loadAutodelete(access, generation: current)
            }

            var rule = autodelete?.rules.first { $0.id == ruleID }
            var checks = 0
            while rule == nil || rule?.state == .compiling {
                guard checks < 20 else { return .stillChecking }
                try await self.sleep(.seconds(1))
                let snapshot = try await access.autodeleteRules()
                guard current == generation else { return .failed }
                autodelete = snapshot
                rule = snapshot.rules.first { $0.id == ruleID }
                checks += 1
            }
            guard var ready = rule else { return .failed }
            if case .approved(let approvedDays, _) = ready.state, approvedDays == days {
                return .ready(ready)
            }
            if ready.state != .draft(days: days) {
                try await withCommandID(.setDays(ruleID: ruleID, days: days)) { commandID in
                    try await access.setAutodeleteDays(ruleID: ruleID, days: days, commandID: commandID)
                }
                guard current == generation else { return .failed }
                await loadAutodelete(access, generation: current)
                guard let updated = autodelete?.rules.first(where: { $0.id == ruleID }) else {
                    return .failed
                }
                ready = updated
            }
            return .ready(ready)
        } catch {
            guard current == generation else { return .failed }
            issues[.autodelete] = presentationError(error)
            await refreshAfterUncertain(error)
            return .failed
        }
    }

    /// Turns on the reviewed clean-up rule with its draft days.
    func turnOnCleanUp(_ rule: JetAutodeleteRule) async {
        await approveRule(rule)
    }

    /// Cancel in the clean-up sheet: removes the rule only when this model
    /// compiled it and it isn't turned on. Rules that existed before stay.
    func discardCleanUpDraft() async {
        guard operation == nil,
              let rule = managedCleanUpRule,
              createdCleanUpRuleIDs.contains(rule.id)
        else { return }
        if case .approved = rule.state { return }
        let current = generation
        operation = "discard-cleanup"
        defer { if current == generation { operation = nil } }
        do {
            let access = try await activeAccess()
            try await withCommandID(.deleteRule(ruleID: rule.id)) { commandID in
                try await access.deleteAutodeleteRule(ruleID: rule.id, commandID: commandID)
            }
            createdCleanUpRuleIDs.remove(rule.id)
            guard current == generation else { return }
            await loadAutodelete(access, generation: current)
        } catch {
            guard current == generation else { return }
            issues[.autodelete] = presentationError(error)
            await refreshAfterUncertain(error)
        }
    }

    private func compileCleanUpRule(days: UInt32, access: any JetRecoveryAccess) async throws -> UUID {
        let prompt = Self.cleanUpPrompt(days: days)
        let key = IntentKey(planeRegistryID: planeRegistryID, intent: .compile(prompt: prompt))
        let ruleID = compileRuleIDs[key] ?? UUID()
        compileRuleIDs[key] = ruleID
        do {
            try await withCommandID(.compile(prompt: prompt)) { commandID in
                try await access.compileAutodeleteRule(ruleID: ruleID, prompt: prompt, commandID: commandID)
            }
            compileRuleIDs.removeValue(forKey: key)
            createdCleanUpRuleIDs.insert(ruleID)
            return ruleID
        } catch {
            if !Self.isUnknownOutcome(error) { compileRuleIDs.removeValue(forKey: key) }
            throw error
        }
    }

    // MARK: - Custom rules

    func compileRule() async {
        guard operation == nil, !rulePrompt.isEmpty, rulePrompt.utf8.count <= 4_096 else { return }
        let current = generation
        let prompt = rulePrompt
        operation = "compile-rule"
        defer { if current == generation { operation = nil } }
        let key = IntentKey(planeRegistryID: planeRegistryID, intent: .compile(prompt: prompt))
        let ruleID = compileRuleIDs[key] ?? UUID()
        compileRuleIDs[key] = ruleID
        do {
            let access = try await activeAccess()
            try await withCommandID(.compile(prompt: prompt)) { commandID in
                try await access.compileAutodeleteRule(ruleID: ruleID, prompt: prompt, commandID: commandID)
            }
            compileRuleIDs.removeValue(forKey: key)
            guard current == generation else { return }
            if rulePrompt == prompt { rulePrompt = "" }
            await loadAutodelete(access, generation: current)
            notice = String(localized: "Jet is checking the rule. Review it before turning it on.")
        } catch {
            if !Self.isUnknownOutcome(error) { compileRuleIDs.removeValue(forKey: key) }
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
        await changeRule(.setDays(ruleID: rule.id, days: days)) { access, commandID in
            try await access.setAutodeleteDays(ruleID: rule.id, days: days, commandID: commandID)
        }
    }

    func approveRule(_ rule: JetAutodeleteRule) async {
        guard issues[.autodelete] == nil,
              case let .draft(days) = rule.state
        else { return }
        await changeRule(.approve(ruleID: rule.id, days: days)) { access, commandID in
            try await access.approveAutodelete(ruleID: rule.id, days: days, commandID: commandID)
        }
        if case .approved = autodelete?.rules.first(where: { $0.id == rule.id })?.state {
            createdCleanUpRuleIDs.remove(rule.id)
        }
    }

    func authorizeEverywhere(_ rule: JetAutodeleteRule) async {
        guard issues[.autodelete] == nil,
              case .approved = rule.state, rule.scope == "forget"
        else { return }
        await changeRule(.authorize(ruleID: rule.id)) { access, commandID in
            try await access.authorizeAutodeleteEverywhere(ruleID: rule.id, commandID: commandID)
        }
    }

    func deleteRule(_ rule: JetAutodeleteRule) async {
        guard issues[.autodelete] == nil else { return }
        await changeRule(.deleteRule(ruleID: rule.id)) { access, commandID in
            try await access.deleteAutodeleteRule(ruleID: rule.id, commandID: commandID)
        }
    }

    // MARK: - Backups

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
            try await withCommandID(.restoreSnapshot(name: snapshot.name)) { commandID in
                try await access.restoreRecoverySnapshot(snapshot.name, commandID: commandID)
            }
            guard current == generation else { return }
            await loadHealth(access, generation: current)
            notice = String(localized: "Backup restored. Check Security Audit for anything to review.")
            await onSnapshotRestored()
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
            let removed = try await withCommandID(.purge) { commandID in
                try await access.purgeRecoverySnapshots(commandID: commandID)
            }
            guard current == generation else { return }
            await loadHealth(access, generation: current)
            notice = String(localized: "Removed \(removed.count) older backups.")
        } catch {
            guard current == generation else { return }
            issues[.health] = presentationError(error)
            await refreshAfterUncertain(error)
        }
    }

    // MARK: - Commands

    private func changeRule(
        _ intent: RecoveryIntent,
        action: (any JetRecoveryAccess, UUID) async throws -> Void
    ) async {
        guard operation == nil else { return }
        let current = generation
        operation = "rule"
        notice = nil
        defer { if current == generation { operation = nil } }
        do {
            let access = try await activeAccess()
            try await withCommandID(intent) { commandID in
                try await action(access, commandID)
            }
            guard current == generation else { return }
            await loadAutodelete(access, generation: current)
            notice = String(localized: "Clean up updated.")
        } catch {
            guard current == generation else { return }
            issues[.autodelete] = presentationError(error)
            await refreshAfterUncertain(error)
        }
    }

    /// Runs one Command with the ID kept for `intent` on this computer.
    private func withCommandID<T>(
        _ intent: RecoveryIntent,
        _ send: (UUID) async throws -> T
    ) async throws -> T {
        let key = IntentKey(planeRegistryID: planeRegistryID, intent: intent)
        let commandID = commandIDs[key] ?? UUID()
        commandIDs[key] = commandID
        do {
            let value = try await send(commandID)
            commandIDs.removeValue(forKey: key)
            return value
        } catch {
            if !Self.isUnknownOutcome(error) { commandIDs.removeValue(forKey: key) }
            throw error
        }
    }

    /// After an unknown outcome, reloads the state (queries only) and says so.
    /// The Command itself is never sent again automatically.
    private func refreshAfterUncertain(_ error: Error) async {
        guard Self.isUnknownOutcome(error) else { return }
        operation = nil
        await refresh()
        notice = String(localized: "Jet couldn't confirm this change. Check the current state before trying again.")
    }

    nonisolated private static func isUnknownOutcome(_ error: Error) -> Bool {
        if case JetClientFailure.commandOutcomeUnknown = error { return true }
        return false
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
        case let error as JetPresentationError: error
        case JetClientFailure.commandOutcomeUnknown:
            JetPresentationError(
                category: .outcomeUnknown,
                code: "command.outcome_unknown",
                message: String(localized: "Jet couldn't confirm this change. Check the current state before trying again."),
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
