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

@MainActor
@Observable
final class JetSettingsModel {
    private let makeAccess: JetSettingsAccessProvider
    private let now: @Sendable () -> Date
    private var loadGeneration = 0

    var isLoading = false
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

    init(
        makeAccess: @escaping JetSettingsAccessProvider,
        now: @escaping @Sendable () -> Date = Date.init
    ) {
        self.makeAccess = makeAccess
        self.now = now
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
            let access = try await makeAccess()
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

    func setSetting(_ rawKey: String, value: JetSettingValue, scope: JetSettingScope) async {
        guard operation == nil, let key = SettingKey(rawValue: rawKey) else { return }
        operation = "setting-\(rawKey)"
        notice = nil
        do {
            let access = try await makeAccess()
            // ASVS 2.3.1 and 8.3.1: each edit is one authenticated command.
            // The client never treats the local control value as policy truth.
            _ = try await access.setSetting(
                key,
                value: value,
                scope: scope,
                commandID: UUID()
            )
            await reloadSettings(access, scope: scope)
            notice = "Saved on the Plane."
        } catch {
            await handleSettingFailure(error, scope: scope)
        }
        operation = nil
    }

    func clearSetting(_ rawKey: String, scope: JetSettingScope) async {
        guard operation == nil, let key = SettingKey(rawValue: rawKey) else { return }
        operation = "setting-\(rawKey)"
        notice = nil
        do {
            let access = try await makeAccess()
            _ = try await access.clearSetting(key, scope: scope, commandID: UUID())
            await reloadSettings(access, scope: scope)
            notice = "Restored the inherited value."
        } catch {
            await handleSettingFailure(error, scope: scope)
        }
        operation = nil
    }

    func bindAccount(_ option: JetAuthProvider) async {
        guard operation == nil else { return }
        operation = "account-bind"
        notice = nil
        do {
            let access = try await makeAccess()
            _ = try await access.bindHarnessAccount(option, commandID: UUID())
            await loadAccounts(access, generation: loadGeneration)
            notice = "Connected \(option.harness) through its native login."
        } catch {
            issues[.accounts] = presentationError(error)
        }
        operation = nil
    }

    func confirmAccountRemoval() async {
        guard operation == nil, let binding = pendingAccountRemoval else { return }
        operation = "account-remove"
        notice = nil
        do {
            let access = try await makeAccess()
            try await access.unbindHarnessAccount(binding.id, commandID: UUID())
            pendingAccountRemoval = nil
            await loadAccounts(access, generation: loadGeneration)
            notice = "Removed the Account binding from this Plane."
        } catch {
            issues[.accounts] = presentationError(error)
        }
        operation = nil
    }

    func createSchedule() async {
        guard operation == nil, let conversationID else { return }
        operation = "schedule-create"
        notice = nil
        do {
            let access = try await makeAccess()
            let localTime = formattedScheduleTime()
            _ = try await access.createSchedule(
                conversationID: conversationID,
                timeZone: scheduleTimeZone,
                localTime: localTime,
                prompt: schedulePrompt,
                commandID: UUID()
            )
            schedulePrompt = ""
            await loadSchedules(access, conversationID: conversationID, generation: loadGeneration)
            notice = "Created the daily schedule."
        } catch {
            issues[.schedules] = presentationError(error)
        }
        operation = nil
    }

    func confirmScheduleCancellation() async {
        guard operation == nil,
              let schedule = pendingScheduleCancellation,
              let conversationID
        else { return }
        operation = "schedule-cancel"
        notice = nil
        do {
            let access = try await makeAccess()
            try await access.cancelSchedule(schedule.id, commandID: UUID())
            pendingScheduleCancellation = nil
            await loadSchedules(access, conversationID: conversationID, generation: loadGeneration)
            notice = "Canceled future schedule firings."
        } catch {
            issues[.schedules] = presentationError(error)
        }
        operation = nil
    }

    func inspectExtension() async {
        guard operation == nil, !selectedExtensionCraftID.isEmpty else { return }
        operation = "extension-inspect"
        notice = nil
        do {
            let access = try await makeAccess()
            pendingExtensionProposal = try await access.inspectExtension(
                craftID: selectedExtensionCraftID,
                extensionID: extensionID,
                action: extensionAction
            )
            issues.removeValue(forKey: .extensions)
        } catch {
            issues[.extensions] = presentationError(error)
        }
        operation = nil
    }

    func confirmExtensionChange() async {
        guard operation == nil, let proposal = pendingExtensionProposal else { return }
        operation = "extension-change"
        notice = nil
        do {
            let access = try await makeAccess()
            let changeID = try await access.changeExtension(proposal, commandID: UUID())
            latestExtensionChange = try await access.extensionChange(changeID)
            pendingExtensionProposal = nil
            await loadExtensions(access, crafts: crafts, generation: loadGeneration)
            notice = "The Plane accepted the extension change for subsequent Runs."
        } catch {
            issues[.extensions] = presentationError(error)
        }
        operation = nil
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

    private func handleSettingFailure(_ error: Error, scope: JetSettingScope) async {
        let failure = presentationError(error)
        let area: JetSettingsArea = switch scope {
        case .plane: .plane
        case .project: .project
        case .conversation: .conversation
        }
        issues[area] = failure
        if failure.category == .conflict {
            notice = "The value changed on the Plane. Jet refreshed it so you can review the current setting."
            if let access = try? await makeAccess() {
                await reloadSettings(access, scope: scope)
            }
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

    private func presentationError(_ error: Error) -> JetPresentationError {
        switch error {
        case let JetClientFailure.presentation(error): error
        case let JetClientFailure.commandOutcomeUnknown(commandID):
            JetPresentationError(
                category: .outcomeUnknown,
                code: "command.outcome_unknown",
                message: "Jet could not confirm the change. Refresh before trying again. Command \(commandID.uuidString.prefix(8)).",
                retryable: false
            )
        default: .invalidResponse
        }
    }
}

extension JetSettingsArea: CaseIterable {}
