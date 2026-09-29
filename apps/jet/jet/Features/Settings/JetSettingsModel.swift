import Foundation
import Observation

nonisolated protocol JetSettingsAccess: Sendable {
    func settings(scope: JetSettingScope) async throws -> JetSettingSnapshot
    func setSetting(
        _ key: SettingKey,
        value: JetSettingValue,
        scope: JetSettingScope,
        commandID: UUID
    ) async throws -> JetSettingSet
    func clearSetting(
        _ key: SettingKey,
        scope: JetSettingScope,
        commandID: UUID
    ) async throws -> JetSettingCleared
    func accountBindings(
        _ observation: JetCapabilityObservation
    ) async throws -> JetAccountBindingList
    func bindHarnessAccount(
        _ option: JetAuthProvider,
        commandID: UUID
    ) async throws -> JetAccountBindingSummary
    func unbindHarnessAccount(_ bindingID: UUID, commandID: UUID) async throws
    func usage() async throws -> JetUsageSnapshot
    func usageHistory(
        fromUnixMilliseconds: Int64,
        untilUnixMilliseconds: Int64
    ) async throws -> JetUsageHistorySnapshot
    func extensionCatalog(craftID: String) async throws -> JetExtensionCatalogSummary
    func inspectExtension(
        craftID: String,
        extensionID: String,
        action: JetExtensionAction
    ) async throws -> JetExtensionProposal
    func changeExtension(
        _ proposal: JetExtensionProposal,
        commandID: UUID
    ) async throws -> UUID
    func extensionChange(_ changeID: UUID) async throws -> JetExtensionChangeSummary
    func scheduledTasks(conversationID: UUID) async throws -> JetScheduledTaskSnapshot
    func createSchedule(
        conversationID: UUID,
        timeZone: String,
        localTime: String,
        prompt: String,
        commandID: UUID
    ) async throws -> JetScheduledTask
    func cancelSchedule(_ scheduleID: UUID, commandID: UUID) async throws
}

extension JetClient: JetSettingsAccess {}

typealias JetSettingsAccessProvider = @MainActor () async throws -> any JetSettingsAccess
/// Settings access for one computer, by its plane registry ID.
typealias JetSettingsPlaneAccessProvider = @MainActor (UUID) async throws -> any JetSettingsAccess

nonisolated enum JetSettingsArea: String, Sendable, Hashable, Identifiable {
    case plane
    case project
    case conversation
    case accounts
    case usage
    case extensions
    case schedules

    var id: String { rawValue }
}

/// What the last save of one setting row came to. A row that is saving has no
/// status; `isSaving(_:scope:)` covers it.
enum SettingRowStatus: Equatable, Sendable {
    case saved
    case failed(JetPresentationError)
    /// The outcome is unknown. The Command ID is kept, so saving the same value
    /// again replays the same Command. Jet never retries on its own.
    case unconfirmed
}

@MainActor
@Observable
final class JetSettingsModel {
    private let accessForPlane: @MainActor (UUID?) async throws -> any JetSettingsAccess
    private let now: @Sendable () -> Date
    private var loadGeneration = 0
    /// Bumps when the computer changes, so late results for the previous
    /// computer never reach the new one's rows.
    private var planeGeneration = 0

    /// The computer these settings belong to, when the model is scoped by one.
    var planeRegistryID: UUID?
    var isLoading = false
    /// Account, extension and schedule operations. Setting rows use
    /// `savingKeys` instead, so one row saving never blocks another.
    var operation: String?
    var notice: String?
    var issues: [JetSettingsArea: JetPresentationError] = [:]

    var planeSettings: JetSettingSnapshot?
    var projectSettings: JetSettingSnapshot?
    var conversationSettings: JetSettingSnapshot?
    var accounts: JetAccountBindingList?
    var usage: JetUsageSnapshot?
    var usageHistory: JetUsageHistorySnapshot?
    var extensionCatalogs: [JetExtensionCatalogSummary] = []
    var schedules: JetScheduledTaskSnapshot?
    var latestExtensionChange: JetExtensionChangeSummary?

    var conversationID: UUID?
    var projectID: UUID?
    var crafts: [JetInstalledCraft] = []
    var authProviders: [JetAuthProvider] = []

    var schedulePrompt = ""
    var scheduleTime = Date()
    var scheduleTimeZone = TimeZone.current.identifier
    var pendingScheduleCancellation: JetScheduledTask?

    var selectedExtensionCraftID = ""
    var extensionID = ""
    var extensionAction = JetExtensionAction.install
    var pendingExtensionProposal: JetExtensionProposal?
    var pendingAccountRemoval: JetAccountBindingSummary?

    /// Rows being saved, keyed by `rowKey(_:scope:)` ("computer|scope|key").
    var savingKeys: Set<String> = []
    /// Rows whose outcome is being checked again after an unknown outcome.
    var checkingKeys: Set<String> = []
    /// Each row's last save outcome, keyed like `savingKeys`.
    var rowStatuses: [String: SettingRowStatus] = [:]
    /// Command IDs of setting changes, keyed "computer|scope|key|value". An ID is
    /// dropped on success or a definite error and kept after an unknown outcome,
    /// so the person's explicit retry of the same change reuses it.
    var settingCommandIDs: [String: UUID] = [:]
    /// Command IDs of account, extension and schedule changes, under the same rule.
    var commandIDs: [String: UUID] = [:]

    init(
        makeAccess: @escaping JetSettingsAccessProvider,
        now: @escaping @Sendable () -> Date = Date.init
    ) {
        accessForPlane = { _ in try await makeAccess() }
        self.now = now
    }

    /// Settings for the computer passed to `load(planeRegistryID:crafts:authProviders:)`.
    init(
        planeAccess: @escaping JetSettingsPlaneAccessProvider,
        now: @escaping @Sendable () -> Date = Date.init
    ) {
        accessForPlane = { planeRegistryID in
            guard let planeRegistryID else { throw JetClientFailure.presentation(.offline) }
            return try await planeAccess(planeRegistryID)
        }
        self.now = now
    }

    /// The computer's connection failed on the last load or save.
    var isUnreachable: Bool { issues[.plane]?.category == .offline }

    /// Loads one computer's settings. Switching computers first clears everything
    /// shown for the previous one.
    func load(
        planeRegistryID: UUID,
        crafts: [JetInstalledCraft],
        authProviders: [JetAuthProvider]
    ) async {
        if self.planeRegistryID != planeRegistryID {
            self.planeRegistryID = planeRegistryID
            planeGeneration += 1
            planeSettings = nil
            projectSettings = nil
            conversationSettings = nil
            accounts = nil
            usage = nil
            usageHistory = nil
            extensionCatalogs = []
            schedules = nil
            latestExtensionChange = nil
            pendingExtensionProposal = nil
            pendingAccountRemoval = nil
            pendingScheduleCancellation = nil
            issues = [:]
            rowStatuses = [:]
            notice = nil
            operation = nil
        }
        await load(conversationID: nil, projectID: nil, crafts: crafts, authProviders: authProviders)
    }

    /// Loads again with the current computer and scopes.
    func refresh() async {
        await load(
            conversationID: conversationID,
            projectID: projectID,
            crafts: crafts,
            authProviders: authProviders
        )
    }

    func load(
        conversationID: UUID?,
        projectID: UUID?,
        crafts: [JetInstalledCraft],
        authProviders: [JetAuthProvider]
    ) async {
        self.conversationID = conversationID
        self.projectID = projectID
        self.crafts = crafts
        self.authProviders = authProviders
        if selectedExtensionCraftID.isEmpty
            || !crafts.contains(where: { $0.id == selectedExtensionCraftID })
        {
            selectedExtensionCraftID = crafts.first?.id ?? ""
        }

        loadGeneration += 1
        let generation = loadGeneration
        isLoading = true
        notice = nil
        do {
            let access = try await activeAccess()
            await loadPlaneSettings(access, generation: generation)
            if let projectID {
                await loadSettings(
                    access,
                    scope: .project(projectID),
                    area: .project,
                    generation: generation
                )
            } else if generation == loadGeneration {
                projectSettings = nil
                issues.removeValue(forKey: .project)
            }
            if let conversationID {
                await loadSettings(
                    access,
                    scope: .conversation(conversationID),
                    area: .conversation,
                    generation: generation
                )
                await loadSchedules(access, conversationID: conversationID, generation: generation)
            } else if generation == loadGeneration {
                conversationSettings = nil
                schedules = nil
                issues.removeValue(forKey: .conversation)
                issues.removeValue(forKey: .schedules)
            }
            await loadAccounts(access, generation: generation)
            await loadUsage(access, generation: generation)
            await loadExtensions(access, crafts: crafts, generation: generation)
        } catch {
            guard generation == loadGeneration else { return }
            let failure = presentationError(error)
            for area in JetSettingsArea.allCases {
                issues[area] = failure
            }
        }
        guard generation == loadGeneration else { return }
        isLoading = false
    }

    func settingValue(_ key: String, scope: JetSettingScope) -> JetSettingValue? {
        snapshot(for: scope)?.settings.first { $0.key.rawValue == key }?.value
    }

    func settingSource(_ key: String, scope: JetSettingScope) -> JetSettingSource? {
        snapshot(for: scope)?.settings.first { $0.key.rawValue == key }?.source
    }

    /// The value was set in this scope, so Reset to Default can clear it.
    func isExplicit(_ key: String, scope: JetSettingScope) -> Bool {
        settingSource(key, scope: scope) == .scope(scope)
    }

    // MARK: - Row saves

    /// The key of one row's saving state and status.
    func rowKey(_ key: String, scope: JetSettingScope) -> String {
        "\(planeToken)|\(Self.scopeToken(scope))|\(key)"
    }

    func isSaving(_ key: String, scope: JetSettingScope) -> Bool {
        savingKeys.contains(rowKey(key, scope: scope))
    }

    func isChecking(_ key: String, scope: JetSettingScope) -> Bool {
        checkingKeys.contains(rowKey(key, scope: scope))
    }

    /// Any row of this computer is saving, or an account, extension or schedule
    /// operation is running.
    var isBusy: Bool {
        let prefix = "\(planeToken)|"
        return operation != nil || savingKeys.contains { $0.hasPrefix(prefix) }
    }

    func rowStatus(_ key: String, scope: JetSettingScope) -> SettingRowStatus? {
        rowStatuses[rowKey(key, scope: scope)]
    }

    func clearRowStatus(_ key: String, scope: JetSettingScope) {
        rowStatuses.removeValue(forKey: rowKey(key, scope: scope))
    }

    /// Saves one setting. Only the same row is guarded; other rows can save at
    /// the same time.
    func setSetting(_ rawKey: String, value: JetSettingValue, scope: JetSettingScope) async {
        guard let key = SettingKey(rawValue: rawKey) else { return }
        await save(rawKey, scope: scope, intent: Self.valueToken(value)) { access, commandID in
            // ASVS 2.3.1 and 8.3.1: each edit is one authenticated command.
            // The client never treats the local control value as policy truth.
            _ = try await access.setSetting(key, value: value, scope: scope, commandID: commandID)
        }
    }

    /// Clears the value set in this scope, so the default applies again.
    func clearSetting(_ rawKey: String, scope: JetSettingScope) async {
        guard let key = SettingKey(rawValue: rawKey) else { return }
        await save(rawKey, scope: scope, intent: Self.defaultToken) { access, commandID in
            _ = try await access.clearSetting(key, scope: scope, commandID: commandID)
        }
    }

    /// Check Again after an unknown outcome: reloads the scope and compares it with
    /// the change that couldn't be confirmed. It never sends the change again.
    func checkAgain(_ rawKey: String, scope: JetSettingScope) async {
        let row = rowKey(rawKey, scope: scope)
        guard !savingKeys.contains(row), !checkingKeys.contains(row) else { return }
        let generation = planeGeneration
        checkingKeys.insert(row)
        defer { checkingKeys.remove(row) }
        do {
            let access = try await activeAccess()
            await reloadSettings(access, scope: scope)
        } catch {
            guard generation == planeGeneration else { return }
            rowStatuses[row] = .failed(presentationError(error))
            return
        }
        guard generation == planeGeneration else { return }
        let pending = settingCommandIDs.keys.filter { $0.hasPrefix("\(row)|") }
        let applied = pending.contains { intentKey in
            let intent = String(intentKey.dropFirst(row.count + 1))
            return intentMatchesCurrent(intent, key: rawKey, scope: scope)
        }
        if applied {
            for intentKey in pending { settingCommandIDs.removeValue(forKey: intentKey) }
            rowStatuses[row] = .saved
        } else {
            // The change didn't apply as far as Jet can see. The row shows the
            // current value, and the kept ID lets the person's retry replay it.
            rowStatuses.removeValue(forKey: row)
        }
    }

    private func save(
        _ rawKey: String,
        scope: JetSettingScope,
        intent: String,
        send: (any JetSettingsAccess, UUID) async throws -> Void
    ) async {
        let row = rowKey(rawKey, scope: scope)
        guard !savingKeys.contains(row), !checkingKeys.contains(row) else { return }
        let generation = planeGeneration
        let intentKey = "\(row)|\(intent)"
        let commandID = settingCommandIDs[intentKey] ?? UUID()
        settingCommandIDs[intentKey] = commandID
        savingKeys.insert(row)
        rowStatuses.removeValue(forKey: row)
        defer { savingKeys.remove(row) }
        do {
            let access = try await activeAccess()
            try await send(access, commandID)
            settingCommandIDs.removeValue(forKey: intentKey)
            guard generation == planeGeneration else { return }
            await reloadSettings(access, scope: scope)
            guard generation == planeGeneration else { return }
            rowStatuses[row] = .saved
        } catch {
            if Self.isUnknownOutcome(error) {
                guard generation == planeGeneration else { return }
                rowStatuses[row] = .unconfirmed
                return
            }
            settingCommandIDs.removeValue(forKey: intentKey)
            guard generation == planeGeneration else { return }
            let failure = presentationError(error)
            if failure.category == .conflict, let access = try? await activeAccess() {
                // Show the value that won before saying what happened.
                await reloadSettings(access, scope: scope)
                guard generation == planeGeneration else { return }
            }
            if failure.category == .offline {
                issues[.plane] = failure
            }
            rowStatuses[row] = .failed(failure)
        }
    }

    private func intentMatchesCurrent(_ intent: String, key: String, scope: JetSettingScope) -> Bool {
        if intent == Self.defaultToken { return !isExplicit(key, scope: scope) }
        guard isExplicit(key, scope: scope), let value = settingValue(key, scope: scope) else {
            return false
        }
        return Self.valueToken(value) == intent
    }

    private var planeToken: String {
        planeRegistryID?.uuidString.lowercased() ?? "current"
    }

    private static let defaultToken = "default"

    static func scopeToken(_ scope: JetSettingScope) -> String {
        switch scope {
        case .plane: "plane"
        case let .project(id): "project:\(id.uuidString.lowercased())"
        case let .conversation(id): "conversation:\(id.uuidString.lowercased())"
        }
    }

    static func valueToken(_ value: JetSettingValue) -> String {
        switch value {
        case let .flag(flag): "flag:\(flag)"
        case let .count(count): "count:\(count)"
        case let .text(text): "text:\(text)"
        }
    }

    // MARK: - Sign-ins, schedules and extensions

    func bindAccount(_ option: JetAuthProvider) async {
        guard operation == nil else { return }
        let generation = planeGeneration
        operation = "account-bind-\(option.provider)"
        notice = nil
        defer { if generation == planeGeneration { operation = nil } }
        do {
            let access = try await activeAccess()
            _ = try await withCommandID("\(planeToken)|bind|\(option.provider)") { commandID in
                try await access.bindHarnessAccount(option, commandID: commandID)
            }
            guard generation == planeGeneration else { return }
            issues.removeValue(forKey: .accounts)
            await loadAccounts(access, generation: loadGeneration)
        } catch {
            guard generation == planeGeneration else { return }
            issues[.accounts] = presentationError(error)
        }
    }

    func confirmAccountRemoval() async {
        guard operation == nil, let binding = pendingAccountRemoval else { return }
        let generation = planeGeneration
        operation = "account-remove-\(binding.provider)"
        notice = nil
        defer { if generation == planeGeneration { operation = nil } }
        do {
            let access = try await activeAccess()
            try await withCommandID("\(planeToken)|unbind|\(binding.id.uuidString)") { commandID in
                try await access.unbindHarnessAccount(binding.id, commandID: commandID)
            }
            guard generation == planeGeneration else { return }
            pendingAccountRemoval = nil
            issues.removeValue(forKey: .accounts)
            await loadAccounts(access, generation: loadGeneration)
        } catch {
            guard generation == planeGeneration else { return }
            pendingAccountRemoval = nil
            issues[.accounts] = presentationError(error)
        }
    }

    func createSchedule() async {
        guard operation == nil, let conversationID else { return }
        let generation = planeGeneration
        operation = "schedule-create"
        notice = nil
        defer { if generation == planeGeneration { operation = nil } }
        do {
            let access = try await activeAccess()
            let localTime = formattedScheduleTime()
            let timeZone = scheduleTimeZone
            let prompt = schedulePrompt
            let intent = "\(planeToken)|schedule|\(conversationID)|\(timeZone)|\(localTime)|\(prompt)"
            _ = try await withCommandID(intent) { commandID in
                try await access.createSchedule(
                    conversationID: conversationID,
                    timeZone: timeZone,
                    localTime: localTime,
                    prompt: prompt,
                    commandID: commandID
                )
            }
            guard generation == planeGeneration else { return }
            schedulePrompt = ""
            await loadSchedules(access, conversationID: conversationID, generation: loadGeneration)
            notice = String(localized: "Repeats daily.")
        } catch {
            guard generation == planeGeneration else { return }
            issues[.schedules] = presentationError(error)
        }
    }

    func confirmScheduleCancellation() async {
        guard operation == nil,
              let schedule = pendingScheduleCancellation,
              let conversationID
        else { return }
        let generation = planeGeneration
        operation = "schedule-cancel"
        notice = nil
        defer { if generation == planeGeneration { operation = nil } }
        do {
            let access = try await activeAccess()
            try await withCommandID("\(planeToken)|cancel-schedule|\(schedule.id.uuidString)") { commandID in
                try await access.cancelSchedule(schedule.id, commandID: commandID)
            }
            guard generation == planeGeneration else { return }
            pendingScheduleCancellation = nil
            await loadSchedules(access, conversationID: conversationID, generation: loadGeneration)
            notice = String(localized: "Stopped repeating.")
        } catch {
            guard generation == planeGeneration else { return }
            issues[.schedules] = presentationError(error)
        }
    }

    func inspectExtension() async {
        guard operation == nil, !selectedExtensionCraftID.isEmpty else { return }
        let generation = planeGeneration
        operation = "extension-inspect"
        notice = nil
        defer { if generation == planeGeneration { operation = nil } }
        do {
            let access = try await activeAccess()
            let proposal = try await access.inspectExtension(
                craftID: selectedExtensionCraftID,
                extensionID: extensionID,
                action: extensionAction
            )
            guard generation == planeGeneration else { return }
            pendingExtensionProposal = proposal
            issues.removeValue(forKey: .extensions)
        } catch {
            guard generation == planeGeneration else { return }
            issues[.extensions] = presentationError(error)
        }
    }

    func confirmExtensionChange() async {
        guard operation == nil, let proposal = pendingExtensionProposal else { return }
        let generation = planeGeneration
        operation = "extension-change"
        notice = nil
        defer { if generation == planeGeneration { operation = nil } }
        do {
            let access = try await activeAccess()
            let changeID = try await withCommandID("\(planeToken)|extension|\(proposal.id)") { commandID in
                try await access.changeExtension(proposal, commandID: commandID)
            }
            let change = try await access.extensionChange(changeID)
            guard generation == planeGeneration else { return }
            latestExtensionChange = change
            pendingExtensionProposal = nil
            await loadExtensions(access, crafts: crafts, generation: loadGeneration)
            notice = String(localized: "Extension change accepted. It applies the next time the assistant starts.")
        } catch {
            guard generation == planeGeneration else { return }
            issues[.extensions] = presentationError(error)
        }
    }

    /// Runs one Command with the ID kept for `intent`. The ID is dropped on
    /// success or a definite error and kept after an unknown outcome.
    private func withCommandID<T>(
        _ intent: String,
        _ send: (UUID) async throws -> T
    ) async throws -> T {
        let commandID = commandIDs[intent] ?? UUID()
        commandIDs[intent] = commandID
        do {
            let value = try await send(commandID)
            commandIDs.removeValue(forKey: intent)
            return value
        } catch {
            if !Self.isUnknownOutcome(error) { commandIDs.removeValue(forKey: intent) }
            throw error
        }
    }

    // MARK: - Loading

    private func activeAccess() async throws -> any JetSettingsAccess {
        try await accessForPlane(planeRegistryID)
    }

    private func loadPlaneSettings(
        _ access: any JetSettingsAccess,
        generation: Int
    ) async {
        await loadSettings(access, scope: .plane, area: .plane, generation: generation)
    }

    private func loadSettings(
        _ access: any JetSettingsAccess,
        scope: JetSettingScope,
        area: JetSettingsArea,
        generation: Int
    ) async {
        do {
            let value = try await access.settings(scope: scope)
            guard generation == loadGeneration else { return }
            switch area {
            case .plane: planeSettings = value
            case .project: projectSettings = value
            case .conversation: conversationSettings = value
            default: break
            }
            issues.removeValue(forKey: area)
        } catch {
            guard generation == loadGeneration else { return }
            issues[area] = presentationError(error)
        }
    }

    private func reloadSettings(
        _ access: any JetSettingsAccess,
        scope: JetSettingScope
    ) async {
        let area: JetSettingsArea = switch scope {
        case .plane: .plane
        case .project: .project
        case .conversation: .conversation
        }
        await loadSettings(access, scope: scope, area: area, generation: loadGeneration)
    }

    private func loadAccounts(
        _ access: any JetSettingsAccess,
        generation: Int
    ) async {
        do {
            let value = try await access.accountBindings(.fresh)
            guard generation == loadGeneration else { return }
            accounts = value
            issues.removeValue(forKey: .accounts)
        } catch {
            guard generation == loadGeneration else { return }
            issues[.accounts] = presentationError(error)
        }
    }

    private func loadUsage(
        _ access: any JetSettingsAccess,
        generation: Int
    ) async {
        do {
            let current = try await access.usage()
            guard generation == loadGeneration else { return }
            usage = current
            issues.removeValue(forKey: .usage)
        } catch {
            guard generation == loadGeneration else { return }
            issues[.usage] = presentationError(error)
            return
        }

        do {
            let end = now()
            let start = end.addingTimeInterval(-30 * 24 * 60 * 60)
            let history = try await access.usageHistory(
                fromUnixMilliseconds: milliseconds(start),
                untilUnixMilliseconds: milliseconds(end)
            )
            guard generation == loadGeneration else { return }
            usageHistory = history
            issues.removeValue(forKey: .usage)
        } catch {
            guard generation == loadGeneration else { return }
            usageHistory = nil
            issues[.usage] = presentationError(error)
        }
    }

    private func loadExtensions(
        _ access: any JetSettingsAccess,
        crafts: [JetInstalledCraft],
        generation: Int
    ) async {
        guard !crafts.isEmpty else {
            if generation == loadGeneration {
                extensionCatalogs = []
                issues.removeValue(forKey: .extensions)
            }
            return
        }
        var catalogs: [JetExtensionCatalogSummary] = []
        var firstFailure: JetPresentationError?
        for craft in crafts {
            do {
                catalogs.append(try await access.extensionCatalog(craftID: craft.id))
            } catch {
                if firstFailure == nil { firstFailure = presentationError(error) }
            }
        }
        guard generation == loadGeneration else { return }
        extensionCatalogs = catalogs
        if let firstFailure {
            issues[.extensions] = firstFailure
        } else {
            issues.removeValue(forKey: .extensions)
        }
    }

    private func loadSchedules(
        _ access: any JetSettingsAccess,
        conversationID: UUID,
        generation: Int
    ) async {
        do {
            let value = try await access.scheduledTasks(conversationID: conversationID)
            guard generation == loadGeneration else { return }
            schedules = value
            issues.removeValue(forKey: .schedules)
        } catch {
            guard generation == loadGeneration else { return }
            issues[.schedules] = presentationError(error)
        }
    }

    private func snapshot(for scope: JetSettingScope) -> JetSettingSnapshot? {
        switch scope {
        case .plane: planeSettings
        case .project: projectSettings
        case .conversation: conversationSettings
        }
    }

    private func formattedScheduleTime() -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: scheduleTimeZone) ?? .current
        let components = calendar.dateComponents([.hour, .minute, .second], from: scheduleTime)
        return String(
            format: "%02d:%02d:%02d",
            components.hour ?? 0,
            components.minute ?? 0,
            components.second ?? 0
        )
    }

    private func milliseconds(_ date: Date) -> Int64 {
        let seconds = date.timeIntervalSince1970
        guard seconds.isFinite,
              seconds <= Double(Int64.max) / 1_000,
              seconds >= Double(Int64.min) / 1_000
        else { return 0 }
        return Int64(seconds * 1_000)
    }

    private static func isUnknownOutcome(_ error: Error) -> Bool {
        if case JetClientFailure.commandOutcomeUnknown = error { return true }
        return false
    }

    private func presentationError(_ error: Error) -> JetPresentationError {
        switch error {
        case let JetClientFailure.presentation(error): error
        case let error as JetPresentationError: error
        case JetClientFailure.commandOutcomeUnknown:
            JetPresentationError(
                category: .outcomeUnknown,
                code: "command.outcome_unknown",
                message: String(localized: "Jet couldn't confirm this change."),
                retryable: false
            )
        default: .invalidResponse
        }
    }
}

extension JetSettingsArea: CaseIterable {}

// MARK: - Pure presentation helpers

/// The core catalog's defaults, for "Reset to Default (…)".
enum JetSettingDefaults {
    static func value(_ key: String) -> JetSettingValue? {
        switch key {
        case "review.automatic", "craft.developer_mode", "energy.constrained", "energy.foreground_override":
            .flag(false)
        case "energy.concurrency": .count(8)
        case "energy.low_power_concurrency": .count(1)
        case "retention.trash_grace_days": .count(30)
        case "security.audit_retention_days": .count(365)
        case "storage.disposable_mib": .count(5_120)
        case "artifact.max_mib": .count(512)
        case "artifact.run_mib": .count(2_048)
        default: nil
        }
    }

    /// The default as a person reads it: "Off", "8", "30 days" or "5 GB".
    static func label(_ key: String) -> String? {
        switch key {
        case "review.automatic", "craft.developer_mode", "energy.constrained", "energy.foreground_override":
            return String(localized: "Off")
        case "retention.trash_grace_days", "security.audit_retention_days":
            guard case let .count(days) = value(key) else { return nil }
            return String(localized: "\(Int(days)) days")
        case "storage.disposable_mib", "artifact.max_mib", "artifact.run_mib":
            guard case let .count(mebibytes) = value(key) else { return nil }
            return StorageSize(mebibytes: mebibytes).label
        default:
            guard case let .count(count) = value(key) else { return nil }
            return count.formatted()
        }
    }
}

/// One quota window as a Gauge and a line of text.
struct UsageWindowPresentation: Equatable {
    /// How full the window is, from 0 to 1, when the Provider stated a limit.
    let fraction: Double?
    let text: String
    /// The gauge turns orange from 90%.
    let isNearLimit: Bool

    init(
        _ window: JetQuotaWindowSummary,
        now: Date = .now,
        calendar: Calendar = .autoupdatingCurrent,
        locale: Locale = .autoupdatingCurrent
    ) {
        if case .unreachable = window.freshness {
            fraction = nil
            text = String(localized: "Couldn't check usage")
            isNearLimit = false
            return
        }
        // Whole percents come from integers, so 42% never reads as 41%.
        let percent: Int?
        let share: Double?
        if window.unit == "share" {
            let used = min(window.used, 10_000)
            percent = Int(used / 100)
            share = Double(used) / 10_000
        } else if let limit = window.limit, limit > 0 {
            let used = min(window.used, limit)
            let (product, overflow) = used.multipliedReportingOverflow(by: 100)
            percent = overflow
                ? Int((Double(used) / Double(limit) * 100).rounded(.down))
                : Int(product / limit)
            share = Double(used) / Double(limit)
        } else {
            percent = nil
            share = nil
        }
        let isSecondary = window.window == "secondary"
        var parts: [String] = []
        if let percent {
            parts.append(isSecondary
                ? String(localized: "\(percent)% of second usage limit used")
                : String(localized: "\(percent)% of usage limit used"))
            if let resets = window.resetsAtUnixMilliseconds {
                let date = Date(timeIntervalSince1970: TimeInterval(resets) / 1_000)
                parts.append(String(localized: "Resets \(Self.resetText(date, now: now, calendar: calendar, locale: locale))"))
            }
        } else {
            let used = window.used.formatted(.number.locale(locale))
            let line: String = switch window.unit {
            case "requests": String(localized: "\(used) requests used")
            case "credits": String(localized: "\(used) credits used")
            default: String(localized: "\(used) tokens used")
            }
            parts.append(line)
        }
        if window.freshness == .stale {
            parts.append(String(localized: "May be out of date"))
        }
        fraction = share.map { min(max($0, 0), 1) }
        text = parts.joined(separator: " · ")
        isNearLimit = (percent ?? 0) >= 90
    }

    /// "14:30" today, otherwise "Sep 30, 14:30".
    private static func resetText(_ date: Date, now: Date, calendar: Calendar, locale: Locale) -> String {
        var style = Date.FormatStyle(locale: locale, calendar: calendar, timeZone: calendar.timeZone)
            .hour().minute()
        if !calendar.isDate(date, inSameDayAs: now) {
            style = style.month(.abbreviated).day()
        }
        return date.formatted(style)
    }
}

/// One sentence for a failed save; the message and code go under Details.
enum SettingsErrorCopy {
    static func sentence(_ error: JetPresentationError) -> String {
        switch error.category {
        case .conflict: String(localized: "This setting changed somewhere else. Jet reloaded it.")
        case .invalidInput: error.message
        case .outcomeUnknown: String(localized: "Jet couldn't confirm this change.")
        default: String(localized: "Couldn't save this setting.")
        }
    }
}

/// A size in MiB as a number and an MB or GB unit. Whole gigabytes show as GB.
struct StorageSize: Equatable {
    enum Unit: String, CaseIterable, Identifiable {
        case megabytes
        case gigabytes

        var id: Self { self }
        var title: String {
            switch self {
            case .megabytes: String(localized: "MB")
            case .gigabytes: String(localized: "GB")
            }
        }
        var mebibytes: UInt32 { self == .gigabytes ? 1_024 : 1 }
    }

    var amount: UInt32
    var unit: Unit

    init(amount: UInt32, unit: Unit) {
        self.amount = amount
        self.unit = unit
    }

    init(mebibytes: UInt32) {
        if mebibytes > 0, mebibytes.isMultiple(of: 1_024) {
            self.init(amount: mebibytes / 1_024, unit: .gigabytes)
        } else {
            self.init(amount: mebibytes, unit: .megabytes)
        }
    }

    /// The size in MiB, or nil when it doesn't fit the setting.
    var mebibytes: UInt32? {
        let (value, overflow) = amount.multipliedReportingOverflow(by: unit.mebibytes)
        return overflow ? nil : value
    }

    var label: String {
        switch unit {
        case .megabytes: String(localized: "\(Int(amount)) MB")
        case .gigabytes: String(localized: "\(Int(amount)) GB")
        }
    }
}
