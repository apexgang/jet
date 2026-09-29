import Foundation
import Testing
@testable import jet

@MainActor
struct Wave32SettingsTests {
    // MARK: - Loading

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
        #expect(model.isUnreachable)
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
    func switchingComputersClearsWhatThePreviousOneShowed() async throws {
        let first = SettingsAccessFake(cursor: 41)
        let second = SettingsAccessFake(cursor: 91)
        let firstID = UUID()
        let secondID = UUID()
        let model = JetSettingsModel(planeAccess: { $0 == firstID ? first : second })

        await model.load(planeRegistryID: firstID, crafts: [], authProviders: [])
        await first.setMutationFailure(.presentation(.invalidInput(code: "setting.value_invalid", message: "No.")))
        await model.setSetting("energy.concurrency", value: .count(3), scope: .plane)
        #expect(model.rowStatus("energy.concurrency", scope: .plane) != nil)
        model.notice = "Repeats daily."

        await second.setSettingsFailure(.offline)
        await model.load(planeRegistryID: secondID, crafts: [], authProviders: [])

        #expect(model.planeRegistryID == secondID)
        #expect(model.planeSettings == nil)
        #expect(model.rowStatuses.isEmpty)
        #expect(model.notice == nil)
        #expect(model.issues[.plane]?.code == "transport.offline")

        await second.setSettingsFailure(nil)
        await model.refresh()
        #expect(model.planeSettings?.cursor == 91)
        #expect(model.issues.isEmpty)
    }

    // MARK: - Row saves

    @Test
    func eachRowReportsItsOwnOutcome() async throws {
        let access = SettingsAccessFake()
        let model = JetSettingsModel(planeAccess: { _ in access })
        await model.load(planeRegistryID: UUID(), crafts: [], authProviders: [])

        await model.setSetting("energy.concurrency", value: .count(4), scope: .plane)
        #expect(model.rowStatus("energy.concurrency", scope: .plane) == .saved)
        #expect(model.settingValue("energy.concurrency", scope: .plane) == .count(4))
        #expect(model.isExplicit("energy.concurrency", scope: .plane))
        #expect(model.notice == nil)
        #expect(model.operation == nil)

        let invalid = JetPresentationError.invalidInput(code: "setting.value_invalid", message: "Enter a number from 0 to 64.")
        await access.setMutationFailure(.presentation(invalid))
        await model.setSetting("energy.low_power_concurrency", value: .count(99), scope: .plane)
        #expect(model.rowStatus("energy.low_power_concurrency", scope: .plane) == .failed(invalid))
        #expect(SettingsErrorCopy.sentence(invalid) == "Enter a number from 0 to 64.")
        #expect(model.rowStatus("energy.concurrency", scope: .plane) == .saved)

        await access.setMutationFailure(.commandOutcomeUnknown(commandID: UUID()))
        await model.setSetting("review.automatic", value: .flag(true), scope: .plane)
        #expect(model.rowStatus("review.automatic", scope: .plane) == .unconfirmed)

        model.clearRowStatus("energy.concurrency", scope: .plane)
        #expect(model.rowStatus("energy.concurrency", scope: .plane) == nil)
        #expect(!model.isBusy)
    }

    @Test
    func twoRowsSaveAtTheSameTime() async throws {
        let access = SettingsAccessFake()
        let model = JetSettingsModel(planeAccess: { _ in access })
        await model.load(planeRegistryID: UUID(), crafts: [], authProviders: [])
        await access.hold("energy.concurrency")

        let slow = Task { await model.setSetting("energy.concurrency", value: .count(2), scope: .plane) }
        await access.waitUntilHeld()
        #expect(model.isSaving("energy.concurrency", scope: .plane))
        #expect(model.isBusy)

        // The same row can't save twice, but another row can.
        await model.setSetting("energy.concurrency", value: .count(3), scope: .plane)
        await model.setSetting("review.automatic", value: .flag(true), scope: .plane)
        #expect(model.rowStatus("review.automatic", scope: .plane) == .saved)
        #expect(model.isSaving("energy.concurrency", scope: .plane))
        #expect(!model.isSaving("review.automatic", scope: .plane))

        await access.release()
        await slow.value
        #expect(model.rowStatus("energy.concurrency", scope: .plane) == .saved)
        #expect(!model.isBusy)
        let sent = await access.sentKeys
        #expect(sent == ["energy.concurrency", "review.automatic"])
    }

    @Test
    func anUnknownOutcomeKeepsItsCommandIDForTheExplicitRetry() async throws {
        let access = SettingsAccessFake()
        let model = JetSettingsModel(planeAccess: { _ in access })
        await model.load(planeRegistryID: UUID(), crafts: [], authProviders: [])

        await access.setMutationFailure(.commandOutcomeUnknown(commandID: UUID()))
        await model.setSetting("energy.concurrency", value: .count(5), scope: .plane)
        #expect(model.rowStatus("energy.concurrency", scope: .plane) == .unconfirmed)
        #expect(model.settingCommandIDs.count == 1)
        // Jet never retries on its own.
        #expect(await access.commandIDs.count == 1)

        // Check Again only reads; the change still isn't there, so the ID stays.
        await model.checkAgain("energy.concurrency", scope: .plane)
        #expect(await access.commandIDs.count == 1)
        #expect(model.rowStatus("energy.concurrency", scope: .plane) == nil)
        #expect(model.settingCommandIDs.count == 1)

        // The person saves the same value again: the same Command is replayed.
        await access.setMutationFailure(nil)
        await model.setSetting("energy.concurrency", value: .count(5), scope: .plane)
        let ids = await access.commandIDs
        #expect(ids.count == 2)
        #expect(ids[0] == ids[1])
        #expect(model.settingCommandIDs.isEmpty)

        // A definite error drops the ID; the next attempt is a new Command.
        await access.setMutationFailure(.presentation(.invalidInput(code: "setting.value_invalid", message: "No.")))
        await model.setSetting("energy.concurrency", value: .count(6), scope: .plane)
        #expect(model.settingCommandIDs.isEmpty)
        await access.setMutationFailure(nil)
        await model.setSetting("energy.concurrency", value: .count(6), scope: .plane)
        let later = await access.commandIDs
        #expect(later.count == 4)
        #expect(later[2] != later[3])
    }

    @Test
    func checkAgainConfirmsAChangeThatDidApply() async throws {
        let access = SettingsAccessFake()
        let model = JetSettingsModel(planeAccess: { _ in access })
        await model.load(planeRegistryID: UUID(), crafts: [], authProviders: [])

        // The core applied it, but the answer was lost.
        await access.setMutationFailure(.commandOutcomeUnknown(commandID: UUID()), applyAnyway: true)
        await model.setSetting("review.automatic", value: .flag(true), scope: .plane)
        #expect(model.rowStatus("review.automatic", scope: .plane) == .unconfirmed)

        await model.checkAgain("review.automatic", scope: .plane)
        #expect(model.rowStatus("review.automatic", scope: .plane) == .saved)
        #expect(model.settingCommandIDs.isEmpty)
        #expect(await access.commandIDs.count == 1)
    }

    @Test
    func settingConflictReloadsBeforeSayingWhatHappened() async {
        let access = SettingsAccessFake()
        let model = JetSettingsModel(makeAccess: { access })
        await model.load(
            conversationID: nil,
            projectID: nil,
            crafts: [],
            authProviders: []
        )
        let conflict = JetPresentationError(
            category: .conflict,
            code: "setting.revision_conflict",
            message: "The setting changed.",
            retryable: false
        )
        await access.setMutationFailure(.presentation(conflict))
        let readsBefore = await access.settingsReadCount

        await model.setSetting(
            "review.automatic",
            value: .flag(true),
            scope: .plane
        )

        let mutationCount = await access.settingMutationCount
        let readCount = await access.settingsReadCount
        #expect(model.rowStatus("review.automatic", scope: .plane) == .failed(conflict))
        #expect(SettingsErrorCopy.sentence(conflict) == "This setting changed somewhere else. Jet reloaded it.")
        #expect(model.notice == nil)
        #expect(mutationCount == 1)
        #expect(readCount == readsBefore + 1)
    }

    @Test
    func resetToDefaultClearsTheExplicitValue() async throws {
        let access = SettingsAccessFake()
        let model = JetSettingsModel(planeAccess: { _ in access })
        await model.load(planeRegistryID: UUID(), crafts: [], authProviders: [])
        await model.setSetting("energy.concurrency", value: .count(3), scope: .plane)
        #expect(model.isExplicit("energy.concurrency", scope: .plane))

        await model.clearSetting("energy.concurrency", scope: .plane)
        #expect(!model.isExplicit("energy.concurrency", scope: .plane))
        #expect(model.settingValue("energy.concurrency", scope: .plane) == .count(8))
        #expect(model.rowStatus("energy.concurrency", scope: .plane) == .saved)
    }

    // MARK: - Panes

    @Test
    func panesKeepTheirRawValuesAndReadAsTheDesignNamesThem() {
        #expect(JetSettingsPane.allCases.map(\.rawValue) == [
            "general", "agents", "work", "connections", "safety", "advanced",
        ])
        #expect(JetSettingsPane.allCases.map(\.title) == [
            "General", "Assistants", "Tasks", "Computers", "Safety", "Advanced",
        ])
        #expect(JetSettingsPane.allCases.map(\.symbol) == [
            "gearshape", "sparkles", "checklist", "desktopcomputer", "checkmark.shield", "gearshape.2",
        ])
        #expect(JetSettingsPane.agents.openTitle == "Assistant Settings…")
        #expect(JetSettingsPane.work.openTitle == "Open Settings…")
    }

    @Test(arguments: [
        ("credential.keychain_unavailable", JetSettingsPane.agents),
        ("extension.change_refused", JetSettingsPane.agents),
        ("schedule.time_invalid", JetSettingsPane.work),
        ("autodelete.rule_invalid", JetSettingsPane.work),
        ("pairing.gate_invalid", JetSettingsPane.connections),
        ("remote.ssh_endpoint_invalid", JetSettingsPane.connections),
        ("energy.limit_invalid", JetSettingsPane.safety),
        ("review.binding_missing", JetSettingsPane.safety),
        ("setting.value_invalid", JetSettingsPane.safety),
        ("notification.denied", JetSettingsPane.general),
        ("storage.disk_pressure", JetSettingsPane.advanced),
        ("audit.integrity_degraded", JetSettingsPane.advanced),
        ("recovery.read_only", JetSettingsPane.advanced),
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

    @Test
    func unknownErrorsOpenNoPane() {
        let error = JetPresentationError.invalidInput(code: "transport.offline", message: "Test")
        #expect(JetSettingsPane.resolving(error) == nil)
    }

    // MARK: - Pure helpers

    @Test
    func usageWindowsReadAsAShareOfTheirLimit() throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = try #require(TimeZone(identifier: "UTC"))
        let locale = Locale(identifier: "en_GB")
        let now = Date(timeIntervalSince1970: 1_790_000_000) // 2026-09-21 14:13 UTC
        let resets = Date(timeIntervalSince1970: 1_790_001_000) // 14:30 UTC

        let unified = UsageWindowPresentation(
            window(window: "unified", unit: "share", used: 4_200, resets: resets),
            now: now, calendar: calendar, locale: locale
        )
        #expect(unified.text == "42% of usage limit used · Resets 14:30")
        #expect(unified.fraction == 0.42)
        #expect(!unified.isNearLimit)

        let secondary = UsageWindowPresentation(
            window(window: "secondary", unit: "share", used: 9_000, freshness: .stale),
            now: now, calendar: calendar, locale: locale
        )
        #expect(secondary.text == "90% of second usage limit used · May be out of date")
        #expect(secondary.isNearLimit)

        let counted = UsageWindowPresentation(
            window(window: "primary", unit: "requests", used: 30, limit: 40),
            now: now, calendar: calendar, locale: locale
        )
        #expect(counted.text == "75% of usage limit used")
        #expect(counted.fraction == 0.75)

        let unlimited = UsageWindowPresentation(
            window(window: "primary", unit: "tokens", used: 1_200),
            now: now, calendar: calendar, locale: locale
        )
        #expect(unlimited.text == "1,200 tokens used")
        #expect(unlimited.fraction == nil)

        let unreachable = UsageWindowPresentation(
            window(window: "unified", unit: "share", used: 100, freshness: .unreachable("provider.timeout")),
            now: now, calendar: calendar, locale: locale
        )
        #expect(unreachable.text == "Couldn't check usage")
        #expect(unreachable.fraction == nil)
    }

    @Test
    func resetToDefaultNamesTheCoreDefaults() {
        #expect(JetSettingDefaults.label("review.automatic") == "Off")
        #expect(JetSettingDefaults.label("craft.developer_mode") == "Off")
        #expect(JetSettingDefaults.label("energy.constrained") == "Off")
        #expect(JetSettingDefaults.label("energy.foreground_override") == "Off")
        #expect(JetSettingDefaults.label("energy.concurrency") == "8")
        #expect(JetSettingDefaults.label("energy.low_power_concurrency") == "1")
        #expect(JetSettingDefaults.label("retention.trash_grace_days") == "30 days")
        #expect(JetSettingDefaults.label("security.audit_retention_days") == "365 days")
        #expect(JetSettingDefaults.label("storage.disposable_mib") == "5 GB")
        #expect(JetSettingDefaults.label("artifact.max_mib") == "512 MB")
        #expect(JetSettingDefaults.label("artifact.run_mib") == "2 GB")
        #expect(JetSettingDefaults.label("review.account_binding") == nil)
    }

    @Test
    func storageSizesShowWholeGigabytesAsGB() {
        #expect(StorageSize(mebibytes: 5_120) == StorageSize(amount: 5, unit: .gigabytes))
        #expect(StorageSize(mebibytes: 512) == StorageSize(amount: 512, unit: .megabytes))
        #expect(StorageSize(mebibytes: 1_536) == StorageSize(amount: 1_536, unit: .megabytes))
        #expect(StorageSize(mebibytes: 0) == StorageSize(amount: 0, unit: .megabytes))
        #expect(StorageSize(amount: 2, unit: .gigabytes).mebibytes == 2_048)
        #expect(StorageSize(amount: 700, unit: .megabytes).mebibytes == 700)
        #expect(StorageSize(amount: .max, unit: .gigabytes).mebibytes == nil)
        #expect(StorageSize(mebibytes: 2_048).label == "2 GB")
    }

    @Test(arguments: [
        ("30", UInt32?.some(30)),
        (" 7 ", 7),
        ("1,000", 1_000),
        ("", nil),
        ("-3", nil),
        ("2.5", nil),
        ("ten", nil),
        ("99999999999", nil),
    ])
    func numberFieldsAcceptOnlyWholeNumbers(_ text: String, expected: UInt32?) {
        #expect(SettingNumberInput.parse(text, locale: Locale(identifier: "en_US")) == expected)
    }

    @Test
    func theDiagnosticSummaryHoldsNoAddressesTitlesOrMessages() {
        let health = JetSystemHealth(
            planeID: UUID(), daemonVersion: "1.4.0", daemonStarts: 3,
            daemonStartedAt: Date(timeIntervalSince1970: 1_790_000_000),
            platform: "macos-aarch64", capabilitiesAvailable: true,
            externalTools: [JetExternalToolSummary(tool: "git", availability: .present(version: "2.50.1"))],
            crafts: [JetInstalledCraft(id: "jet-craft-claude", version: "1.4.0", harnesses: ["claude-code"])],
            degradedCapabilities: [], credentialStore: .available,
            recoveryState: "serving", recoveryReason: nil,
            snapshots: [], deletionLedger: "verified", auditIntegrity: .trusted
        )
        let errors = [
            JetPresentationError(
                category: .offline,
                code: "transport.offline",
                message: "Can't reach alex@studio.example while working on Fix login redirect loop",
                retryable: true
            ),
            JetPresentationError.invalidInput(
                code: "Fix login redirect loop at /Users/alex/code/web-app",
                message: "alex@studio.example"
            ),
            JetPresentationError.invalidInput(code: "storage.disk_pressure", message: "/Users/alex"),
        ]

        let summary = JetDiagnosticSummary.make(
            appVersion: "2.0 (alex@studio.example)",
            isLocal: false,
            connection: .failed(errors[0]),
            health: health,
            capabilities: nil,
            errors: errors
        )

        #expect(summary.contains("Service: 1.4.0 · macos-aarch64 · 3 starts"))
        #expect(summary.contains("Crafts: jet-craft-claude 1.4.0"))
        #expect(summary.contains("Recent error codes: storage.disk_pressure, transport.offline"))
        for leaked in ["alex", "studio.example", "Fix login", "/Users", "web-app", "@"] {
            #expect(!summary.contains(leaked), "Leaked \(leaked)")
        }
    }

    private func window(
        window: String,
        unit: String,
        used: UInt64,
        limit: UInt64? = nil,
        resets: Date? = nil,
        freshness: JetUsageFreshness = .fresh
    ) -> JetQuotaWindowSummary {
        JetQuotaWindowSummary(
            bindingID: UUID(),
            provider: "anthropic",
            window: window,
            unit: unit,
            used: used,
            limit: limit,
            resetsAtUnixMilliseconds: resets.map { Int64($0.timeIntervalSince1970 * 1_000) },
            freshness: freshness
        )
    }
}

/// A computer's settings. Saved values change what later reads return.
private actor SettingsAccessFake: JetSettingsAccess {
    private(set) var settingsReadCount = 0
    private(set) var settingMutationCount = 0
    private(set) var commandIDs: [UUID] = []
    private(set) var sentKeys: [String] = []
    private let cursor: UInt64
    private var explicit: [String: JetSettingValue] = [:]
    private var settingsFailure: JetPresentationError?
    private var mutationFailure: JetClientFailure?
    private var appliesDespiteFailure = false
    private var usageHistoryFailure: JetPresentationError?
    private var heldKey: String?
    private var heldContinuation: CheckedContinuation<Void, Never>?
    private var heldWaiter: CheckedContinuation<Void, Never>?
    private let planeID = UUID()

    private static let builtIn: [String: JetSettingValue] = [
        "review.automatic": .flag(false),
        "energy.concurrency": .count(8),
        "energy.low_power_concurrency": .count(1),
    ]

    init(cursor: UInt64 = 41) {
        self.cursor = cursor
    }

    func setSettingsFailure(_ error: JetPresentationError?) {
        settingsFailure = error
    }

    func setMutationFailure(_ failure: JetClientFailure?, applyAnyway: Bool = false) {
        mutationFailure = failure
        appliesDespiteFailure = applyAnyway
    }

    func setUsageHistoryFailure(_ error: JetPresentationError?) {
        usageHistoryFailure = error
    }

    /// The next save of `key` waits until `release()`.
    func hold(_ key: String) {
        heldKey = key
    }

    func waitUntilHeld() async {
        if heldContinuation != nil { return }
        await withCheckedContinuation { heldWaiter = $0 }
    }

    func release() {
        heldKey = nil
        heldContinuation?.resume()
        heldContinuation = nil
    }

    func settings(scope: JetSettingScope) async throws -> JetSettingSnapshot {
        settingsReadCount += 1
        if let settingsFailure { throw JetClientFailure.presentation(settingsFailure) }
        let scopeCursor: UInt64 = switch scope {
        case .plane: cursor
        case .project: cursor + 1
        case .conversation: cursor + 2
        }
        let settings = Self.builtIn.keys.sorted().compactMap { key -> JetResolvedSetting? in
            guard let settingKey = SettingKey(rawValue: key), let value = Self.builtIn[key] else { return nil }
            if let explicitValue = explicit[key] {
                return JetResolvedSetting(key: settingKey, value: explicitValue, source: .scope(.plane))
            }
            return JetResolvedSetting(key: settingKey, value: value, source: .builtIn)
        }
        return JetSettingSnapshot(cursor: scopeCursor, scope: scope, settings: settings)
    }

    func setSetting(
        _ key: SettingKey,
        value: JetSettingValue,
        scope: JetSettingScope,
        commandID: UUID
    ) async throws -> JetSettingSet {
        settingMutationCount += 1
        commandIDs.append(commandID)
        sentKeys.append(key.rawValue)
        if heldKey == key.rawValue {
            await withCheckedContinuation { continuation in
                heldContinuation = continuation
                heldWaiter?.resume()
                heldWaiter = nil
            }
        }
        if let mutationFailure {
            if appliesDespiteFailure { explicit[key.rawValue] = value }
            throw mutationFailure
        }
        explicit[key.rawValue] = value
        return JetSettingSet(key: key, scope: scope, value: value)
    }

    func clearSetting(
        _ key: SettingKey,
        scope: JetSettingScope,
        commandID: UUID
    ) async throws -> JetSettingCleared {
        commandIDs.append(commandID)
        if let mutationFailure { throw mutationFailure }
        explicit.removeValue(forKey: key.rawValue)
        return JetSettingCleared(key: key, scope: scope)
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
