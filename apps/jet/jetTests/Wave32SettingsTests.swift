import Foundation
import Testing
@testable import jet

@MainActor
struct Wave32SettingsTests {
    @Test
    func settingsLoadKeepsEachAuthoritativeFenceAndUsageSnapshot() async throws {
        let access = SettingsAccessFake()
        let projectID = UUID()
        let model = JetSettingsModel(
            makeAccess: { access },
            now: { Date(timeIntervalSince1970: 2_000_000) }
        )

        await model.load(
            conversationID: nil,
            projectID: projectID,
            crafts: [],
            authProviders: []
        )

        #expect(model.planeSettings?.cursor == 41)
        #expect(model.projectSettings?.cursor == 42)
        #expect(model.usage?.tokens.total == 60)
        #expect(model.usageHistory?.series.first?.measurements == 2)
        #expect(model.issues.isEmpty)
    }

    @Test
    func failedRefreshPreservesTheLastTrustedSettings() async throws {
        let access = SettingsAccessFake()
        let model = JetSettingsModel(makeAccess: { access })

        await model.load(
            conversationID: nil,
            projectID: nil,
            crafts: [],
            authProviders: []
        )
        let trusted = try #require(model.planeSettings)

        await access.setSettingsFailure(.offline)
        await model.load(
            conversationID: nil,
            projectID: nil,
            crafts: [],
            authProviders: []
        )

        #expect(model.planeSettings == trusted)
        #expect(model.issues[.plane]?.code == "transport.offline")
    }

    @Test
    func unsupportedHistoryKeepsCurrentUsageVisible() async {
        let access = SettingsAccessFake()
        await access.setUsageHistoryFailure(
            JetPresentationError(
                category: .invalidInput,
                code: "protocol.feature_unavailable",
                message: "This Plane does not support Usage history.",
                retryable: false
            )
        )
        let model = JetSettingsModel(makeAccess: { access })

        await model.load(
            conversationID: nil,
            projectID: nil,
            crafts: [],
            authProviders: []
        )

        #expect(model.usage?.tokens.total == 60)
        #expect(model.usageHistory == nil)
        #expect(model.issues[.usage]?.code == "protocol.feature_unavailable")
    }

    @Test
    func settingConflictRefreshesBeforeOfferingAnotherEdit() async {
        let access = SettingsAccessFake()
        let model = JetSettingsModel(makeAccess: { access })
        await model.load(
            conversationID: nil,
            projectID: nil,
            crafts: [],
            authProviders: []
        )
        await access.setMutationFailure(
            JetPresentationError(
                category: .conflict,
                code: "setting.revision_conflict",
                message: "The setting changed.",
                retryable: false
            )
        )

        await model.setSetting(
            "review.automatic",
            value: .flag(true),
            scope: .plane
        )

        let mutationCount = await access.settingMutationCount
        let readCount = await access.settingsReadCount
        #expect(model.notice?.contains("refreshed") == true)
        #expect(mutationCount == 1)
        #expect(readCount >= 2)
    }

    @Test(arguments: [
        ("credential.keychain_unavailable", JetSettingsPane.agents),
        ("schedule.time_invalid", JetSettingsPane.work),
        ("pairing.gate_invalid", JetSettingsPane.connections),
        ("energy.limit_invalid", JetSettingsPane.safety),
        ("notification.denied", JetSettingsPane.general),
    ])
    func stableErrorsRouteToTheRelevantSettingsPane(
        code: String,
        pane: JetSettingsPane
    ) {
        let error = JetPresentationError(
            category: .invalidInput,
            code: code,
            message: "Test",
            retryable: false
        )
        #expect(JetSettingsPane.resolving(error) == pane)
    }
}

private actor SettingsAccessFake: JetSettingsAccess {
    private(set) var settingsReadCount = 0
    private(set) var settingMutationCount = 0
    private var settingsFailure: JetPresentationError?
    private var mutationFailure: JetPresentationError?
    private var usageHistoryFailure: JetPresentationError?
    private let planeID = UUID()

    func setSettingsFailure(_ error: JetPresentationError?) {
        settingsFailure = error
    }

    func setMutationFailure(_ error: JetPresentationError?) {
        mutationFailure = error
    }

    func setUsageHistoryFailure(_ error: JetPresentationError?) {
        usageHistoryFailure = error
    }

    func settings(scope: JetSettingScope) async throws -> JetSettingSnapshot {
        settingsReadCount += 1
        if let settingsFailure { throw JetClientFailure.presentation(settingsFailure) }
        let cursor: UInt64 = switch scope {
        case .plane: 41
        case .project: 42
        case .conversation: 43
        }
        return JetSettingSnapshot(
            cursor: cursor,
            scope: scope,
            settings: [
                JetResolvedSetting(
                    key: SettingKey(rawValue: "review.automatic")!,
                    value: .flag(false),
                    source: .builtIn
                ),
            ]
        )
    }

    func setSetting(
        _ key: SettingKey,
        value: JetSettingValue,
        scope: JetSettingScope,
        commandID: UUID
    ) async throws -> JetSettingSet {
        settingMutationCount += 1
        if let mutationFailure { throw JetClientFailure.presentation(mutationFailure) }
        return JetSettingSet(key: key, scope: scope, value: value)
    }

    func clearSetting(
        _ key: SettingKey,
        scope: JetSettingScope,
        commandID: UUID
    ) async throws -> JetSettingCleared {
        JetSettingCleared(key: key, scope: scope)
    }

    func accountBindings(
        _ observation: JetCapabilityObservation
    ) async throws -> JetAccountBindingList {
        JetAccountBindingList(cursor: 44, bindings: [])
    }

    func bindHarnessAccount(
        _ option: JetAuthProvider,
        commandID: UUID
    ) async throws -> JetAccountBindingSummary {
        let provider = await option.provider
        let label = await option.label
        return JetAccountBindingSummary(
            id: UUID(),
            provider: provider,
            label: label,
            state: "resolved_at_use",
            stateLabel: "Checked when used"
        )
    }

    func unbindHarnessAccount(_ bindingID: UUID, commandID: UUID) async throws {}

    func usage() async throws -> JetUsageSnapshot {
        JetUsageSnapshot(
            cursor: 45,
            planeID: planeID,
            tokens: JetUsageTokens(input: 10, cachedInput: 5, output: 20, reasoning: 30),
            measurements: 2,
            estimated: 0,
            interim: 0,
            quotaWindows: []
        )
    }

    func usageHistory(
        fromUnixMilliseconds: Int64,
        untilUnixMilliseconds: Int64
    ) async throws -> JetUsageHistorySnapshot {
        if let usageHistoryFailure {
            throw JetClientFailure.presentation(usageHistoryFailure)
        }
        return JetUsageHistorySnapshot(
            cursor: 46,
            planeID: planeID,
            resolution: "day",
            series: [
                JetUsageHistorySeries(
                    model: "test-model",
                    tokens: JetUsageTokens(input: 4, cachedInput: 0, output: 5, reasoning: 6),
                    measurements: 2
                ),
            ]
        )
    }

    func extensionCatalog(craftID: String) async throws -> JetExtensionCatalogSummary {
        JetExtensionCatalogSummary(craftID: craftID, harness: "test", nativeMetadata: "{}")
    }

    func inspectExtension(
        craftID: String,
        extensionID: String,
        action: JetExtensionAction
    ) async throws -> JetExtensionProposal {
        JetExtensionProposal(
            catalog: try await extensionCatalog(craftID: craftID),
            extensionID: extensionID,
            action: action
        )
    }

    func changeExtension(
        _ proposal: JetExtensionProposal,
        commandID: UUID
    ) async throws -> UUID {
        UUID()
    }

    func extensionChange(_ changeID: UUID) async throws -> JetExtensionChangeSummary {
        JetExtensionChangeSummary(
            id: changeID,
            craftID: "test-craft",
            extensionID: "test-extension",
            action: .install,
            state: "applied"
        )
    }

    func scheduledTasks(conversationID: UUID) async throws -> JetScheduledTaskSnapshot {
        JetScheduledTaskSnapshot(cursor: 47, tasks: [])
    }

    func createSchedule(
        conversationID: UUID,
        timeZone: String,
        localTime: String,
        prompt: String,
        commandID: UUID
    ) async throws -> JetScheduledTask {
        JetScheduledTask(
            id: UUID(),
            conversationID: conversationID,
            timeZone: timeZone,
            localTime: localTime,
            prompt: prompt,
            nextDueAtUnixMilliseconds: 1,
            nextIntendedLocal: "test"
        )
    }

    func cancelSchedule(_ scheduleID: UUID, commandID: UUID) async throws {}
}
