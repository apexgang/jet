#if DEBUG
import SwiftUI

extension DesktopPreviewScenes {
    /// Settings panes and sheets (WP11). JetSettingsView skips loading in
    /// previews, so each scene renders a pane or sheet directly with seeded models.
    @MainActor static var settings: [DesktopPreviewScene] {
        [
            SettingsPreview.pane("settings-general") {
                let session = SettingsPreview.session { $0.notificationAuthorization = .authorized }
                return GeneralSettingsPane(session: session)
                    .defaultAppStorage(SettingsPreview.defaults([
                        "jet.notifications.approvals": true,
                        "jet.notifications.completions": true,
                        JetDesign.transcriptScaleKey: 1.15,
                    ]))
            },
            SettingsPreview.pane("settings-general-denied") {
                let session = SettingsPreview.session { $0.notificationAuthorization = .denied }
                return GeneralSettingsPane(session: session)
                    .defaultAppStorage(SettingsPreview.defaults([
                        "jet.notifications.approvals": true,
                        "jet.notifications.completions": true,
                        "jet.notifications.failures": true,
                    ]))
            },
            SettingsPreview.pane("settings-assistants") {
                let session = SettingsPreview.session()
                let settings = SettingsPreview.settings(for: session.localPlaneRegistryID)
                settings.usage = SettingsPreview.usage
                return AssistantsSettingsPane(
                    session: session,
                    settings: settings,
                    recovery: SettingsPreview.recovery(),
                    computer: .constant(session.localPlaneRegistryID)
                )
            },
            SettingsPreview.pane("settings-tasks") {
                let session = SettingsPreview.session()
                return TasksSettingsPane(
                    session: session,
                    settings: SettingsPreview.settings(for: session.localPlaneRegistryID),
                    recovery: SettingsPreview.recovery(rules: []),
                    computer: .constant(session.localPlaneRegistryID)
                )
            },
            SettingsPreview.pane("settings-tasks-cleanup-on") {
                let session = SettingsPreview.session()
                return TasksSettingsPane(
                    session: session,
                    settings: SettingsPreview.settings(for: session.localPlaneRegistryID),
                    recovery: SettingsPreview.recovery(rules: [
                        SettingsPreview.managedRule(state: .approved(days: 30, at: SettingsPreview.date(1_789_804_800_000))),
                        SettingsPreview.customDraft,
                    ]),
                    computer: .constant(session.localPlaneRegistryID)
                )
            },
            SettingsPreview.sheet("settings-cleanup-review") {
                let session = SettingsPreview.session()
                let rule = SettingsPreview.managedRule(state: .draft(days: 30), candidates: SettingsPreview.candidates)
                let recovery = SettingsPreview.recovery(rules: [rule])
                return CleanUpSheet(
                    model: recovery,
                    start: .review(rule),
                    graceDays: 30,
                    computerName: "This Mac",
                    taskTitle: { id in
                        session.conversations.first { $0.id == id }?.title ?? "Untitled task"
                    }
                )
            },
            SettingsPreview.pane("settings-computers") {
                let session = SettingsPreview.session()
                return ComputersSettingsPane(session: session, pairing: SettingsPreview.pairing(for: session))
            },
            SettingsPreview.sheet("settings-allow-code") {
                let session = SettingsPreview.session()
                let pairing = SettingsPreview.pairing(for: session)
                pairing.allowPlaneRegistryID = session.localPlaneRegistryID
                pairing.allowPhase = .showingCode("4821-0937")
                pairing.offerExpiresAt = Date.now.addingTimeInterval(120)
                return AllowConnectionSheet(model: pairing, planeRegistryID: session.localPlaneRegistryID)
            },
            SettingsPreview.sheet("settings-allow-compare") {
                let session = SettingsPreview.session()
                let pairing = SettingsPreview.pairing(for: session)
                pairing.allowPlaneRegistryID = session.localPlaneRegistryID
                pairing.allowPhase = .comparing("482-913")
                return AllowConnectionSheet(model: pairing, planeRegistryID: session.localPlaneRegistryID)
            },
            SettingsPreview.sheet("settings-connect-details") {
                let session = SettingsPreview.session { session in
                    session.remotePlaneName = "Studio Mac"
                    session.remoteSSHEndpoint = "alex@studio.local"
                    session.remotePairingSecret = "4821-0937"
                    session.remotePairingNotice = "Jet could not reach this Plane."
                }
                return ConnectComputerSheet(session: session, initialStep: .details)
            },
            SettingsPreview.sheet("settings-connect-check") {
                let session = SettingsPreview.session { session in
                    session.remotePlaneName = "Studio Mac"
                    session.remoteSSHEndpoint = "alex@studio.local"
                    session.remotePairingClaim = JetRemotePairingClaim(
                        endpoint: "alex@studio.local",
                        offerID: SettingsPreview.id("A01"),
                        authenticationString: "482-913",
                        signingBytes: Data()
                    )
                }
                return ConnectComputerSheet(session: session, initialStep: .check)
            },
            SettingsPreview.sheet("settings-connect-done") {
                let session = SettingsPreview.session { $0.remotePlaneName = "Studio Mac" }
                return ConnectComputerSheet(session: session, initialStep: .done)
            },
            SettingsPreview.pane("settings-safety") {
                let session = SettingsPreview.session()
                let settings = SettingsPreview.settings(for: session.localPlaneRegistryID, values: [
                    "review.automatic": .flag(true),
                    "review.account_binding": .text(SafetySettingsPane.bindingValue(SettingsPreview.claudeBindingID)),
                    "energy.concurrency": .count(6),
                ])
                settings.rowStatuses[settings.rowKey("energy.concurrency", scope: .plane)] = .saved
                settings.rowStatuses[settings.rowKey("energy.low_power_concurrency", scope: .plane)] = .failed(
                    .invalidInput(code: "setting.value_out_of_range", message: "Enter a number from 0 to 64.")
                )
                return SafetySettingsPane(
                    session: session,
                    settings: settings,
                    recovery: SettingsPreview.recovery(),
                    computer: .constant(session.localPlaneRegistryID)
                )
            },
            SettingsPreview.pane("settings-advanced") {
                let session = SettingsPreview.session()
                return AdvancedSettingsPane(
                    session: session,
                    settings: SettingsPreview.settings(for: session.localPlaneRegistryID),
                    recovery: SettingsPreview.recovery(health: SettingsPreview.health(readOnly: false, backups: 7)),
                    computer: .constant(session.localPlaneRegistryID)
                )
            },
            SettingsPreview.pane("settings-advanced-recovery") {
                let session = SettingsPreview.session()
                return AdvancedSettingsPane(
                    session: session,
                    settings: SettingsPreview.settings(for: session.localPlaneRegistryID),
                    recovery: SettingsPreview.recovery(health: SettingsPreview.health(readOnly: true, backups: 2)),
                    computer: .constant(session.localPlaneRegistryID)
                )
            },
            SettingsPreview.pane("settings-offline") {
                let session = SettingsPreview.session()
                let settings = SettingsPreview.settings(for: SettingsPreview.studioMacID, values: [
                    "review.automatic": .flag(true),
                ])
                settings.issues[.plane] = .offline
                return SafetySettingsPane(
                    session: session,
                    settings: settings,
                    recovery: SettingsPreview.recovery(),
                    computer: .constant(SettingsPreview.studioMacID)
                )
            },
        ]
    }
}

/// Seeds for the Settings scenes. Nothing here reaches a computer.
@MainActor
enum SettingsPreview {
    static let studioMacID = id("901")
    static let buildServerID = id("902")
    static let claudeBindingID = id("301")
    static let codexBindingID = id("302")

    static let paneSize = CGSize(width: 640, height: 1700)
    static let sheetSize = CGSize(width: 520, height: 460)

    static func pane<V: View>(_ id: String, _ make: @escaping @MainActor () -> V) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id, size: paneSize) {
            AnyView(make().tint(JetDesign.accent))
        }
    }

    static func sheet<V: View>(_ id: String, _ make: @escaping @MainActor () -> V) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id, size: sheetSize) {
            AnyView(make().tint(JetDesign.accent))
        }
    }

    /// This Mac connected, plus Studio Mac (connected) and Build Server (offline).
    static func session(_ configure: @MainActor (DesktopSession) -> Void = { _ in }) -> DesktopSession {
        .preview { session in
            DesktopPreviewData.connect(session)
            session.remoteProfiles = [
                JetRemotePlaneProfile(id: studioMacID, name: "Studio Mac", endpoint: "alex@studio.local", planeID: id("911")),
                JetRemotePlaneProfile(id: buildServerID, name: "Build Server", endpoint: "ci@build-server", planeID: nil),
            ]
            session.planes.append(JetPlanePresentation(
                id: studioMacID,
                name: "Studio Mac",
                endpoint: "alex@studio.local",
                isLocal: false,
                planeID: id("911"),
                connection: .connected(DesktopPreviewData.negotiation),
                snapshot: DesktopPreviewData.setupSnapshot,
                failure: nil,
                conversationCursor: DesktopPreviewData.headCursor
            ))
            session.planes.append(JetPlanePresentation(
                id: buildServerID,
                name: "Build Server",
                endpoint: "ci@build-server",
                isLocal: false,
                planeID: nil,
                connection: .disconnected,
                snapshot: nil,
                failure: .offline,
                conversationCursor: nil
            ))
            configure(session)
        }
    }

    /// A throwaway defaults store for panes that read AppStorage.
    static func defaults(_ values: [String: Any]) -> UserDefaults {
        let name = "jet.preview.settings"
        let defaults = UserDefaults(suiteName: name) ?? .standard
        defaults.removePersistentDomain(forName: name)
        for (key, value) in values { defaults.set(value, forKey: key) }
        return defaults
    }

    /// Every setting at its default, with `values` set explicitly.
    static func settings(
        for planeRegistryID: UUID,
        values: [String: JetSettingValue] = [:]
    ) -> JetSettingsModel {
        let model = JetSettingsModel(planeAccess: { _ in throw JetClientFailure.presentation(.offline) })
        model.planeRegistryID = planeRegistryID
        let defaults: [String: JetSettingValue] = [
            "review.automatic": .flag(false),
            "review.account_binding": .text(""),
            "review.cross_provider_consent": .text(""),
            "energy.concurrency": .count(8),
            "energy.low_power_concurrency": .count(1),
            "energy.constrained": .flag(false),
            "energy.foreground_override": .flag(false),
            "retention.trash_grace_days": .count(30),
            "security.audit_retention_days": .count(365),
            "storage.disposable_mib": .count(5_120),
            "artifact.max_mib": .count(512),
            "artifact.run_mib": .count(2_048),
            "craft.developer_mode": .flag(false),
        ]
        let resolved = defaults.merging(values) { _, explicit in explicit }
        model.planeSettings = JetSettingSnapshot(
            cursor: DesktopPreviewData.headCursor,
            scope: .plane,
            settings: resolved.keys.sorted().compactMap { key in
                guard let settingKey = SettingKey(rawValue: key), let value = resolved[key] else { return nil }
                return JetResolvedSetting(
                    key: settingKey,
                    value: value,
                    source: values[key] == nil ? .builtIn : .scope(.plane)
                )
            }
        )
        model.accounts = JetAccountBindingList(
            cursor: DesktopPreviewData.headCursor,
            bindings: [
                JetAccountBindingSummary(
                    id: claudeBindingID,
                    provider: "anthropic",
                    label: "Claude Code login",
                    state: "resolvable",
                    stateLabel: "Ready"
                ),
            ]
        )
        let capabilities = DesktopPreviewData.setupSnapshot.capabilities
        model.crafts = capabilities.crafts
        model.authProviders = capabilities.authProviders
        model.selectedExtensionCraftID = capabilities.crafts.first?.id ?? ""
        model.usage = JetUsageSnapshot(
            cursor: DesktopPreviewData.headCursor,
            planeID: DesktopPreviewData.planeID,
            tokens: JetUsageTokens(input: 0, cachedInput: 0, output: 0, reasoning: 0),
            measurements: 0,
            estimated: 0,
            interim: 0,
            quotaWindows: []
        )
        return model
    }

    /// Claude Code's limit at 42%, resetting at 14:30 today, and a stale second
    /// Codex limit near full.
    static var usage: JetUsageSnapshot {
        let resets = Calendar.current.date(bySettingHour: 14, minute: 30, second: 0, of: .now) ?? .now
        return JetUsageSnapshot(
            cursor: DesktopPreviewData.headCursor,
            planeID: DesktopPreviewData.planeID,
            tokens: JetUsageTokens(input: 120_000, cachedInput: 40_000, output: 30_000, reasoning: 0),
            measurements: 12,
            estimated: 0,
            interim: 0,
            quotaWindows: [
                JetQuotaWindowSummary(
                    bindingID: claudeBindingID,
                    provider: "anthropic",
                    window: "unified",
                    unit: "share",
                    used: 4_200,
                    limit: nil,
                    resetsAtUnixMilliseconds: Int64(resets.timeIntervalSince1970 * 1_000),
                    freshness: .fresh
                ),
                JetQuotaWindowSummary(
                    bindingID: codexBindingID,
                    provider: "openai",
                    window: "secondary",
                    unit: "share",
                    used: 9_300,
                    limit: nil,
                    resetsAtUnixMilliseconds: nil,
                    freshness: .stale
                ),
            ]
        )
    }

    static func recovery(
        health: JetSystemHealth? = nil,
        rules: [JetAutodeleteRule]? = nil
    ) -> JetRecoveryModel {
        let model = JetRecoveryModel(makeAccess: { _ in throw JetClientFailure.presentation(.offline) })
        model.health = health
        model.autodelete = rules.map { JetAutodeleteSnapshot(cursor: DesktopPreviewData.headCursor, rules: $0) }
        model.auditEntries = [
            JetAuditEntry(
                id: id("C01"),
                sequence: 380,
                epoch: 1,
                recordedAt: date(DesktopPreviewData.now - 2 * DesktopPreviewData.hour),
                planeID: DesktopPreviewData.planeID,
                actor: "interactive_client",
                decision: "setting_changed",
                targetKind: "setting",
                targetReference: "energy.concurrency",
                risk: "low",
                outcome: "applied"
            ),
        ]
        return model
    }

    static func health(readOnly: Bool, backups: Int) -> JetSystemHealth {
        let day: Int64 = 24 * DesktopPreviewData.hour
        return JetSystemHealth(
            planeID: DesktopPreviewData.planeID,
            daemonVersion: "1.4.0",
            daemonStarts: 3,
            daemonStartedAt: date(DesktopPreviewData.now - 2 * DesktopPreviewData.hour),
            platform: "macos-aarch64",
            capabilitiesAvailable: true,
            externalTools: [JetExternalToolSummary(tool: "git", availability: .present(version: "2.50.1"))],
            crafts: DesktopPreviewData.setupSnapshot.capabilities.crafts,
            degradedCapabilities: [],
            credentialStore: .available,
            recoveryState: readOnly ? "read_only" : "serving",
            recoveryReason: readOnly ? "integrity_check_failed" : nil,
            snapshots: (0 ..< backups).map { index in
                JetRecoverySnapshot(
                    name: "plane-\(index)-daily.sqlite3",
                    reason: index == backups - 1 && backups > 2 ? "migration" : "daily",
                    takenAt: date(DesktopPreviewData.now - Int64(index + 1) * day + 3 * DesktopPreviewData.hour),
                    bytes: UInt64(421_888 - index * 2_048)
                )
            },
            deletionLedger: "verified",
            auditIntegrity: .trusted
        )
    }

    static func managedRule(
        state: JetAutodeleteState,
        candidates: [JetAutodeleteCandidate] = []
    ) -> JetAutodeleteRule {
        JetAutodeleteRule(
            id: id("B01"),
            prompt: JetRecoveryModel.cleanUpPrompt(days: 30),
            scope: "forget",
            state: state,
            candidates: candidates
        )
    }

    static let customDraft = JetAutodeleteRule(
        id: id("B02"),
        prompt: "Forget billing-service experiments after two weeks",
        scope: "forget",
        state: .draft(days: 14),
        candidates: []
    )

    /// Three idle tasks; one has unsaved changes in its working copy.
    static var candidates: [JetAutodeleteCandidate] {
        let tasks = DesktopPreviewData.conversations
        return [
            JetAutodeleteCandidate(conversationID: tasks[4].id, lastActiveAt: date(1_787_223_600_000), protections: []),
            JetAutodeleteCandidate(conversationID: tasks[5].id, lastActiveAt: date(1_786_698_000_000), protections: ["dirty_workspace"]),
            JetAutodeleteCandidate(conversationID: tasks[6].id, lastActiveAt: date(1_785_942_000_000), protections: []),
        ]
    }

    /// One Jet app paired on Aug 12, on This Mac.
    static func pairing(for session: DesktopSession) -> ComputerPairingModel {
        let model = ComputerPairingModel(makeAccess: { _ in throw JetClientFailure.presentation(.offline) })
        model.summaries[session.localPlaneRegistryID] = JetPairingSummary(
            cursor: DesktopPreviewData.headCursor,
            gate: "closed",
            clients: [
                JetPairedClientSummary(
                    id: id("D01"),
                    access: .enabled,
                    pairedAtUnixMilliseconds: 1_786_530_600_000,
                    pairingProtocol: "jet.pairing.v1",
                    publicKey: String(repeating: "ab", count: 32)
                ),
            ],
            pending: nil
        )
        return model
    }

    static func date(_ milliseconds: Int64) -> Date {
        Date(timeIntervalSince1970: TimeInterval(milliseconds) / 1_000)
    }

    static func id(_ suffix: String) -> UUID {
        let padded = String(repeating: "0", count: max(0, 12 - suffix.count)) + suffix
        guard let id = UUID(uuidString: "7A3C0000-0000-4000-8000-\(padded)") else {
            preconditionFailure("Invalid preview UUID \(suffix)")
        }
        return id
    }
}

#Preview("Settings › General") { DesktopPreviewScenes.view("settings-general") }
#Preview("Settings › Assistants") { DesktopPreviewScenes.view("settings-assistants") }
#Preview("Settings › Tasks") { DesktopPreviewScenes.view("settings-tasks-cleanup-on") }
#Preview("Settings › Computers") { DesktopPreviewScenes.view("settings-computers") }
#Preview("Settings › Safety") { DesktopPreviewScenes.view("settings-safety") }
#Preview("Settings › Advanced") { DesktopPreviewScenes.view("settings-advanced") }
#Preview("Settings › Offline") { DesktopPreviewScenes.view("settings-offline") }
#endif
