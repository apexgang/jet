#if DEBUG
import SwiftUI

/// Library Queries answered from fixed data for previews and screenshots.
/// Commands never reach a computer: they fail as offline.
nonisolated struct PreviewLibraryAccess: JetLibraryAccess {
    /// 2026-09-28 12:00 UTC.
    static let now = Date(timeIntervalSince1970: 1_790_596_800)

    static let billingWebhooksID = previewID("7A3C0000-0000-4000-8000-000000000901")
    static let onboardingCopyID = previewID("7A3C0000-0000-4000-8000-000000000902")
    static let cleanedUpID = previewID("7A3C0000-0000-4000-8000-000000000903")
    static let transferredID = previewID("7A3C0000-0000-4000-8000-000000000904")
    static let deletedEverywhereID = previewID("7A3C0000-0000-4000-8000-000000000905")
    static let berlinScheduleID = previewID("7A3C0000-0000-4000-8000-000000000911")
    static let newYorkScheduleID = previewID("7A3C0000-0000-4000-8000-000000000912")

    static let cursor: UInt64 = 400

    let snapshots: [UUID: JetConversationSnapshot]
    let settingSnapshots: [JetSettingSnapshot]
    let planeID: UUID

    /// The standard data, with the session's tasks answering `conversation(id)`.
    /// web-app saves each reply to a branch; its tasks follow it.
    @MainActor
    static func standard(for session: DesktopSession) -> PreviewLibraryAccess {
        let webApp = DesktopPreviewData.webApp.id
        let billingWebhooks = JetConversationSummary(
            id: billingWebhooksID,
            revision: 2,
            title: "Migrate billing webhooks",
            createdAtUnixMilliseconds: Int64(now.timeIntervalSince1970 * 1_000) - 6 * 86_400_000,
            projectID: DesktopPreviewData.billingService.id
        )
        var snapshots: [UUID: JetConversationSnapshot] = [:]
        var settings = [settingSnapshot(scope: .plane, flags: ReplyFinishFlags(), flagSource: .builtIn)]
        for project in session.allProjects {
            let scope = JetSettingScope.project(project.project.id)
            let flags = project.project.id == webApp ? ReplyFinishPreset.saveToBranch.flags : ReplyFinishFlags()
            settings.append(settingSnapshot(scope: scope, flags: flags, flagSource: .scope(scope)))
        }
        for summary in session.conversations + [billingWebhooks] {
            snapshots[summary.id] = JetConversationSnapshot(
                cursor: cursor,
                conversation: summary,
                workspaceID: nil,
                workspaceRoot: nil,
                runs: []
            )
            let flags = summary.projectID == webApp ? ReplyFinishPreset.saveToBranch.flags : ReplyFinishFlags()
            settings.append(settingSnapshot(
                scope: .conversation(summary.id),
                flags: flags,
                flagSource: summary.projectID.map { .scope(.project($0)) } ?? .builtIn
            ))
        }
        return PreviewLibraryAccess(
            snapshots: snapshots,
            settingSnapshots: settings,
            planeID: DesktopPreviewData.planeID
        )
    }

    // MARK: Data

    static var trashEntries: [JetTrashEntry] {
        [
            entry(billingWebhooksID, "manual_forget", trashed: 1_790_436_000, expires: 1_793_028_000),
            entry(onboardingCopyID, "manual_forget", trashed: 1_789_898_700, expires: 1_792_490_700),
            entry(cleanedUpID, "autodelete_rule", trashed: 1_789_182_000, expires: 1_791_774_000),
            entry(transferredID, "plane_transfer", trashed: 1_787_907_600, expires: 1_790_499_600),
            entry(deletedEverywhereID, "delete_everywhere", trashed: 1_790_534_400, expires: 1_793_126_400),
        ]
    }

    static func schedules(for conversationID: UUID) -> [JetScheduledTask] {
        [
            JetScheduledTask(
                id: berlinScheduleID,
                conversationID: conversationID,
                timeZone: "Europe/Berlin",
                localTime: "09:00:00",
                prompt: "Check last night's CI failures and summarize anything new since yesterday.",
                nextDueAtUnixMilliseconds: 1_790_665_200_000,
                nextIntendedLocal: "2026-09-29T09:00:00"
            ),
            JetScheduledTask(
                id: newYorkScheduleID,
                conversationID: conversationID,
                timeZone: "America/New_York",
                localTime: "17:30:00",
                prompt: "Add today's merged pull requests to the changelog draft.",
                nextDueAtUnixMilliseconds: 1_790_631_000_000,
                nextIntendedLocal: "2026-09-28T17:30:00"
            ),
        ]
    }

    /// Resolved settings for one scope.
    @MainActor
    static func settingSnapshot(
        scope: JetSettingScope,
        flags: ReplyFinishFlags,
        flagSource: JetSettingSource,
        branchPrefix: String = "jet/",
        branchPrefixSource: JetSettingSource = .builtIn,
        automaticNaming: Bool = true,
        automaticNamingSource: JetSettingSource = .builtIn
    ) -> JetSettingSnapshot {
        var settings = zip(ReplyFinishFlags.keys, flags.values).map { key, value in
            JetResolvedSetting(key: key, value: .flag(value), source: value ? flagSource : .builtIn)
        }
        settings.append(JetResolvedSetting(key: ReplyFinishModel.branchPrefixKey, value: .text(branchPrefix), source: branchPrefixSource))
        settings.append(JetResolvedSetting(key: ReplyFinishModel.automaticNamingKey, value: .flag(automaticNaming), source: automaticNamingSource))
        settings.append(JetResolvedSetting(key: JetTrashModel.graceDaysKey, value: .count(30), source: .builtIn))
        return JetSettingSnapshot(cursor: cursor, scope: scope, settings: settings)
    }

    // MARK: Queries

    func conversation(_ conversationID: UUID) async throws -> JetConversationSnapshot {
        guard let snapshot = snapshots[conversationID] else {
            throw JetClientFailure.presentation(JetPresentationError(
                category: .notFound,
                code: "conversation.not_found",
                message: "",
                retryable: false
            ))
        }
        return snapshot
    }

    func settings(scope: JetSettingScope) async throws -> JetSettingSnapshot {
        guard let snapshot = settingSnapshots.first(where: { $0.scope == scope }) else {
            throw JetClientFailure.presentation(JetPresentationError(
                category: .notFound,
                code: "setting.scope_not_found",
                message: "",
                retryable: false
            ))
        }
        return snapshot
    }

    func scheduledTasks(conversationID: UUID) async throws -> JetScheduledTaskSnapshot {
        JetScheduledTaskSnapshot(cursor: Self.cursor, tasks: Self.schedules(for: conversationID))
    }

    func systemHealth() async throws -> JetSystemHealth {
        JetSystemHealth(
            planeID: planeID,
            daemonVersion: "1.4.0",
            daemonStarts: 3,
            daemonStartedAt: Self.now.addingTimeInterval(-7_200),
            platform: "macos-aarch64",
            capabilitiesAvailable: true,
            externalTools: [],
            crafts: [],
            degradedCapabilities: [],
            credentialStore: .available,
            recoveryState: "serving",
            recoveryReason: nil,
            snapshots: [],
            deletionLedger: "verified",
            auditIntegrity: .trusted
        )
    }

    func conversationTrash() async throws -> JetTrashSnapshot {
        JetTrashSnapshot(cursor: Self.cursor, entries: Self.trashEntries)
    }

    func retentionPreview(conversationID: UUID) async throws -> JetRetentionPreview {
        JetRetentionPreview(
            conversationID: conversationID,
            protections: ["dirty_workspace", "unpushed_work"],
            auditRecords: 0,
            trash: Self.trashEntries.first { $0.conversationID == conversationID }
        )
    }

    // MARK: Commands

    func setSetting(_ key: SettingKey, value: JetSettingValue, scope: JetSettingScope, commandID: UUID) async throws -> JetSettingSet {
        throw JetClientFailure.presentation(.offline)
    }

    func clearSetting(_ key: SettingKey, scope: JetSettingScope, commandID: UUID) async throws -> JetSettingCleared {
        throw JetClientFailure.presentation(.offline)
    }

    func createSchedule(conversationID: UUID, timeZone: String, localTime: String, prompt: String, commandID: UUID) async throws -> JetScheduledTask {
        throw JetClientFailure.presentation(.offline)
    }

    func cancelSchedule(_ scheduleID: UUID, commandID: UUID) async throws {
        throw JetClientFailure.presentation(.offline)
    }

    func stageConversation(_ conversationID: UUID, action: JetRetentionAction, commandID: UUID) async throws -> JetTrashEntry {
        throw JetClientFailure.presentation(.offline)
    }

    func restoreConversation(_ conversationID: UUID, commandID: UUID) async throws {
        throw JetClientFailure.presentation(.offline)
    }

    func stopRun(runID: UUID, commandID: UUID) async throws {
        throw JetClientFailure.presentation(.offline)
    }

    // MARK: Helpers

    private static func entry(_ id: UUID, _ reason: String, trashed: TimeInterval, expires: TimeInterval) -> JetTrashEntry {
        JetTrashEntry(
            conversationID: id,
            reason: reason,
            trashedAt: Date(timeIntervalSince1970: trashed),
            expiresAt: Date(timeIntervalSince1970: expires)
        )
    }

    static func previewID(_ value: String) -> UUID {
        guard let id = UUID(uuidString: value) else { preconditionFailure("Invalid preview UUID \(value)") }
        return id
    }
}

extension DesktopPreviewScenes {
    /// Project page, Jet Trash and library sheets (WP10).
    @MainActor static var library: [DesktopPreviewScene] {
        [
            window("library-project-page") { session in
                DesktopPreviewData.connect(session)
                session.open(.project(DesktopPreviewData.webApp.id))
            },
            view("library-project-custom", width: 760, height: 640) { session in
                DesktopPreviewData.connect(session)
                session.open(.project(DesktopPreviewData.webApp.id))
                let model = projectModel(session)
                model.seedForPreview(
                    scopeSnapshot: PreviewLibraryAccess.settingSnapshot(
                        scope: .project(DesktopPreviewData.webApp.id),
                        flags: ReplyFinishFlags(branch: true, commit: false, push: true),
                        flagSource: .scope(.project(DesktopPreviewData.webApp.id)),
                        branchPrefix: "alex/",
                        branchPrefixSource: .scope(.project(DesktopPreviewData.webApp.id)),
                        automaticNaming: false,
                        automaticNamingSource: .scope(.project(DesktopPreviewData.webApp.id))
                    ),
                    planeSnapshot: PreviewLibraryAccess.settingSnapshot(scope: .plane, flags: ReplyFinishFlags(), flagSource: .builtIn),
                    issue: LibraryIssue(
                        kind: .error,
                        text: String(localized: "Couldn't finish changing this setting. Some steps were saved."),
                        action: .tryAgain,
                        code: "transport.too_many_requests"
                    ),
                    issueRow: .replyFinish
                )
                return AnyView(ProjectPageView(session: session, model: model))
            },
            view("library-project-offline", width: 760, height: 640) { session in
                DesktopPreviewData.connect(session)
                session.open(.project(DesktopPreviewData.webApp.id))
                session.connectionState = .disconnected
                session.updatePlane(session.localPlaneRegistryID) { plane in
                    plane.connection = .disconnected
                    plane.failure = .offline
                }
                let model = projectModel(session)
                model.seedForPreview(
                    scopeSnapshot: PreviewLibraryAccess.settingSnapshot(
                        scope: .project(DesktopPreviewData.webApp.id),
                        flags: ReplyFinishPreset.saveToBranch.flags,
                        flagSource: .scope(.project(DesktopPreviewData.webApp.id))
                    ),
                    isOffline: true
                )
                return AnyView(ProjectPageView(session: session, model: model))
            },
            window("library-trash") { session in
                DesktopPreviewData.connect(session)
                session.memory.recordTrashedTitle("Update onboarding copy", for: PreviewLibraryAccess.onboardingCopyID)
                session.open(.trash)
            },
            view("library-trash-empty", width: 900, height: 600) { session in
                DesktopPreviewData.connect(session)
                session.open(.trash)
                let model = trashModel(session)
                model.seedForPreview(planeRegistryID: session.localPlaneRegistryID, phase: .loaded)
                return AnyView(JetTrashPage(session: session, model: model))
            },
            view("library-trash-loading", width: 900, height: 600) { session in
                DesktopPreviewData.connect(session)
                session.open(.trash)
                let model = trashModel(session)
                model.seedForPreview(planeRegistryID: session.localPlaneRegistryID, phase: .loading, graceDays: nil)
                return AnyView(JetTrashPage(session: session, model: model))
            },
            moveToTrash("library-move-to-trash", height: 440, protections: ["dirty_workspace", "unpushed_work", "enabled_schedule"]),
            moveToTrash("library-move-to-trash-running", height: 400, protections: ["active_run", "dirty_workspace"]),
            moveToTrash("library-move-to-trash-stopping", height: 400, protections: ["active_run", "dirty_workspace"], phase: .stopping),
            moveToTrash("library-move-to-trash-waiting", height: 360, protections: ["pending_turn", "dirty_workspace"]),
            moveToTrash("library-delete-everywhere", height: 400, protections: ["active_run", "pending_turn", "dirty_workspace"], mode: .deleteEverywhere),
            view("library-repeat-daily", width: 520, height: 620) { session in
                DesktopPreviewData.connect(session)
                let ref = loginRef(session)
                let model = RepeatDailyModel(ref: ref, makeAccess: session.libraryAccessProvider)
                model.seedForPreview(
                    schedules: PreviewLibraryAccess.schedules(for: ref.conversationID),
                    message: "Run the end-to-end login tests against staging and tell me if anything regressed.",
                    timeZoneID: "Europe/Berlin"
                )
                return AnyView(RepeatDailySheet(session: session, ref: ref, model: model))
            },
            view("library-task-settings", width: 480, height: 420) { session in
                DesktopPreviewData.connect(session)
                let ref = loginRef(session)
                let webApp = DesktopPreviewData.webApp.id
                let model = ReplyFinishModel(
                    planeRegistryID: ref.planeRegistryID,
                    scope: .task(conversationID: ref.conversationID, projectID: webApp),
                    makeAccess: session.libraryAccessProvider
                )
                model.seedForPreview(
                    scopeSnapshot: PreviewLibraryAccess.settingSnapshot(
                        scope: .conversation(ref.conversationID),
                        flags: ReplyFinishPreset.saveToBranch.flags,
                        flagSource: .scope(.project(webApp))
                    ),
                    projectSnapshot: PreviewLibraryAccess.settingSnapshot(
                        scope: .project(webApp),
                        flags: ReplyFinishPreset.saveToBranch.flags,
                        flagSource: .scope(.project(webApp))
                    )
                )
                return AnyView(TaskSettingsSheet(session: session, ref: ref, model: model))
            },
            view("library-project-removal", width: 520, height: 480) { session in
                DesktopPreviewData.connect(session)
                let preview = removalPreview(liveRuns: 0, schedules: 0, obstacles: [])
                session.removalPreview = preview
                return AnyView(ProjectRemovalSheet(session: session, preview: preview))
            },
            view("library-project-removal-blocked", width: 520, height: 440) { session in
                DesktopPreviewData.connect(session)
                let preview = removalPreview(
                    liveRuns: 1,
                    schedules: 2,
                    obstacles: [ProjectRemovalSheet.liveRunsLabel, ProjectRemovalSheet.schedulesLabel]
                )
                session.removalPreview = preview
                return AnyView(ProjectRemovalSheet(session: session, preview: preview))
            },
        ]
    }

    // MARK: Helpers

    /// A scene that renders one library view for a seeded preview session.
    @MainActor private static func view(
        _ id: String,
        width: CGFloat,
        height: CGFloat,
        make: @escaping @MainActor (DesktopSession) -> AnyView
    ) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id, size: CGSize(width: width, height: height)) {
            var content: AnyView?
            _ = DesktopSession.preview { session in content = make(session) }
            return AnyView(
                (content ?? AnyView(EmptyView()))
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                    .background(.background)
                    .tint(JetDesign.accent)
            )
        }
    }

    @MainActor private static func loginRef(_ session: DesktopSession) -> ConversationRef {
        ConversationRef(
            conversationID: DesktopPreviewData.loginRedirect.id,
            planeRegistryID: session.localPlaneRegistryID
        )
    }

    @MainActor private static func projectModel(_ session: DesktopSession) -> ReplyFinishModel {
        ReplyFinishModel(
            planeRegistryID: session.localPlaneRegistryID,
            scope: .project(DesktopPreviewData.webApp.id),
            makeAccess: session.libraryAccessProvider
        )
    }

    @MainActor private static func trashModel(_ session: DesktopSession) -> JetTrashModel {
        JetTrashModel(
            makeAccess: session.libraryAccessProvider,
            memory: session.memory,
            now: { PreviewLibraryAccess.now }
        )
    }

    @MainActor private static func moveToTrash(
        _ id: String,
        height: CGFloat,
        protections: [String],
        mode: MoveToTrashModel.Mode = .forget,
        phase: MoveToTrashModel.Phase = .ready
    ) -> DesktopPreviewScene {
        view(id, width: 480, height: height) { session in
            DesktopPreviewData.connect(session)
            let ref = loginRef(session)
            let model = MoveToTrashModel(
                ref: ref,
                title: "Fix login redirect",
                assistantName: "Claude Code",
                mode: mode,
                makeAccess: session.libraryAccessProvider,
                memory: session.memory,
                now: { PreviewLibraryAccess.now }
            )
            model.seedForPreview(
                preview: JetRetentionPreview(
                    conversationID: ref.conversationID,
                    protections: protections,
                    auditRecords: 0,
                    trash: nil
                ),
                phase: phase
            )
            return AnyView(MoveToTrashSheet(
                session: session,
                ref: ref,
                deleteEverywhere: mode == .deleteEverywhere,
                model: model
            ))
        }
    }

    @MainActor private static func removalPreview(
        liveRuns: UInt64,
        schedules: UInt64,
        obstacles: [String]
    ) -> JetProjectRemovalPreview {
        JetProjectRemovalPreview(
            projectID: DesktopPreviewData.billingService.id,
            root: "/Users/alex/code/api-server",
            diskUseBytes: 184_320_000,
            liveRuns: liveRuns,
            schedules: schedules,
            dirtyFiles: 3,
            unpushedCommits: 2,
            workspaceCount: 4,
            obstacles: obstacles,
            permanentWarning: "Deleting permanently can't be undone.",
            binding: JetRawJSON(source: "{}")
        )
    }
}
#endif
