#if DEBUG
import SwiftUI

extension DesktopPreviewScenes {
    /// Keep Changes states (WP9): the sheet, the status line, Git Activity and the
    /// window with a kept or unconfirmed task.
    @MainActor static var keepChanges: [DesktopPreviewScene] {
        typealias Data = KeepChangesPreviewData
        return [
            Data.sheet("keep-sheet-branch", height: 620) { model, _ in
                model.apply(Data.facts())
                model.branchName = Data.branch
            },
            Data.sheet("keep-sheet-pull-request", height: 760) { model, _ in
                model.apply(Data.facts())
                model.branchName = Data.branch
                model.choice = .draftPullRequest
                model.showsOptions = true
            },
            Data.sheet("keep-sheet-created-earlier", height: 600) { model, _ in
                model.apply(Data.facts(history: [
                    Data.delivery(.branch(name: Data.branch), .completed(head: Data.head, branch: Data.branch, pullRequest: nil), minutesAgo: 40, salt: 1),
                ]))
                model.choice = .saveAndPush
            },
            Data.sheet("keep-sheet-invalid-name", height: 620) { model, _ in
                model.apply(Data.facts())
                model.branchName = "fix login"
            },
            Data.sheet("keep-sheet-single-push", height: 520, mode: .single(.push)) { model, _ in
                model.apply(Data.facts())
                model.showsOptions = true
            },
            Data.sheet("keep-sheet-empty", height: 420) { model, _ in
                model.apply(Data.facts(fileCount: 0))
            },
            Data.sheet("keep-sheet-running", height: 600) { model, session in
                model.apply(Data.facts())
                session.deliveries.previewSeed(
                    progress: Data.progress(
                        [.branch, .commit, .push],
                        [.branch: .completed(nil), .commit: .pending],
                        isRunning: true
                    ),
                    history: [],
                    for: Data.key(session)
                )
            },
            DesktopPreviewScene(id: "keep-status-states", size: CGSize(width: 1200, height: 1500)) {
                AnyView(KeepChangesStatusGallery(session: Data.gallerySession()))
            },
            DesktopPreviewScene(id: "git-activity", size: CGSize(width: 420, height: 900)) {
                let session = DesktopSession.preview { session in
                    DesktopPreviewData.waitingWithChanges(session)
                    Data.seedActivity(session)
                }
                return AnyView(
                    ScrollView {
                        GitActivityList(session: session)
                            .padding(16)
                    }
                    .background(.background)
                    .tint(JetDesign.accent)
                )
            },
            window("task-keep-saved", size: CGSize(width: 1440, height: 860)) { session in
                DesktopPreviewData.inspector(session)
                session.deliveries.previewSeed(
                    progress: Data.progress(
                        [.branch, .commit],
                        [.branch: .completed(Data.branchDelivery), .commit: .completed(Data.commitDelivery)],
                        isRunning: false
                    ),
                    history: [Data.commitDelivery, Data.branchDelivery],
                    for: Data.key(session)
                )
                session.gitDeliveries = [Data.commitDelivery, Data.branchDelivery]
            },
            window("task-git-unconfirmed", size: CGSize(width: 1440, height: 860)) { session in
                DesktopPreviewData.waitingWithChanges(session)
                let history = [Data.unconfirmedPush, Data.commitDelivery, Data.branchDelivery]
                session.deliveries.previewSeed(progress: nil, history: history, for: Data.key(session))
                session.gitDeliveries = history
                session.statusStore.recordGitDeliveries(history, conversationID: Data.task.id)
                session.selectedWorkPanel = .run
                session.isWorkPanelPresented = true
            },
        ]
    }
}

/// Git deliveries and plans for the Keep Changes scenes. Delivery IDs are UUIDv7
/// relative to now, so Git Activity reads "25 min. ago" in every capture.
@MainActor
enum KeepChangesPreviewData {
    typealias Step = DeliveryCoordinator.Step

    static let branch = "jet/fix-login-redirect"
    static let head = "8b7d9e0a1f2e3d4c5b6a7f8e9d0c1b2a3f4e5d6c"
    static let pullRequest = "https://github.com/alex/web-app/pull/42"

    static var task: JetConversationSummary { DesktopPreviewData.loginRedirect }

    static var checkpoint: JetGitCheckpoint {
        JetGitCheckpoint(runID: DesktopPreviewData.runID(1), turn: 1)
    }

    static func key(_ session: DesktopSession, conversationID: UUID? = nil) -> DeliveryCoordinator.Key {
        DeliveryCoordinator.Key(
            planeRegistryID: session.localPlaneRegistryID,
            conversationID: conversationID ?? task.id
        )
    }

    static func facts(fileCount: Int = 3, history: [JetGitDelivery] = []) -> KeepChangesFacts {
        KeepChangesFacts(
            conversationID: task.id,
            title: task.title,
            projectName: DesktopPreviewData.webApp.name,
            assistantName: "Claude Code",
            computerName: "This Mac",
            isRemote: false,
            isLocalCheckout: false,
            fileCount: fileCount,
            checkpoint: fileCount > 0 ? checkpoint : nil,
            branchPrefix: "jet/",
            history: history
        )
    }

    static func plan(_ steps: [Step]) -> DeliveryCoordinator.Plan {
        DeliveryCoordinator.Plan(
            steps: steps,
            branchName: branch,
            remote: "origin",
            baseBranch: nil,
            checkpoint: checkpoint
        )
    }

    static func progress(
        _ steps: [Step],
        _ states: [Step: DeliveryCoordinator.StepState],
        isRunning: Bool,
        lastCheckFailed: Bool = false
    ) -> DeliveryCoordinator.Progress {
        DeliveryCoordinator.Progress(
            plan: plan(steps),
            states: states,
            isRunning: isRunning,
            lastCheckFailed: lastCheckFailed
        )
    }

    /// A UUIDv7 made `minutesAgo` minutes before now; `salt` keeps IDs distinct.
    static func deliveryID(minutesAgo: Double, salt: Int) -> UUID {
        let milliseconds = UInt64(max(0, Date.now.timeIntervalSince1970 * 1_000 - minutesAgo * 60_000))
        let time = (0 ..< 6).map { UInt8(truncatingIfNeeded: milliseconds >> (8 * (5 - UInt64($0)))) }
        let salt = UInt8(truncatingIfNeeded: salt)
        return UUID(uuid: (
            time[0], time[1], time[2], time[3], time[4], time[5],
            0x70, salt, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, salt
        ))
    }

    static func delivery(
        _ operation: JetGitOperation,
        _ outcome: JetGitDeliveryOutcome,
        minutesAgo: Double,
        conversationID: UUID? = nil,
        automatic: Bool = false,
        acknowledged: Bool = false,
        salt: Int
    ) -> JetGitDelivery {
        let carriesCheckpoint: Bool = switch operation {
        case .commit, .draftPullRequest: true
        case .branch, .push: false
        }
        return JetGitDelivery(
            id: deliveryID(minutesAgo: minutesAgo, salt: salt),
            conversationID: conversationID ?? task.id,
            checkpoint: carriesCheckpoint ? checkpoint : nil,
            operation: operation,
            policy: JetGitDeliveryPolicy(
                automatic: automatic,
                branch: true,
                commit: true,
                push: true,
                draftPullRequest: true,
                branchPrefix: "jet/"
            ),
            utilityJobID: nil,
            message: carriesCheckpoint
                ? JetGitMessage(title: "Fix the login redirect loop", body: "", fallbackReason: nil)
                : nil,
            acknowledgedBy: acknowledged ? UUID(uuidString: "7A3C0000-0000-4000-8000-000000000999") : nil,
            outcome: outcome
        )
    }

    static let branchDelivery = delivery(
        .branch(name: branch),
        .completed(head: head, branch: branch, pullRequest: nil),
        minutesAgo: 3,
        salt: 1
    )
    static let commitDelivery = delivery(
        .commit,
        .completed(head: head, branch: branch, pullRequest: nil),
        minutesAgo: 2,
        salt: 2
    )
    static let unconfirmedPush = delivery(.push(remote: "origin"), .outcomeUnknown, minutesAgo: 1, salt: 3)

    /// Every kind of Git Activity row, newest first: more than ten, so Show More appears.
    static func seedActivity(_ session: DesktopSession) {
        let marking = delivery(.commit, .outcomeUnknown, minutesAgo: 20, salt: 14)
        let markFailed = delivery(.branch(name: "jet/try-again"), .outcomeUnknown, minutesAgo: 30, salt: 15)
        let history = [
            delivery(.push(remote: "origin"), .pending, minutesAgo: 1, salt: 11),
            delivery(.push(remote: "origin"), .outcomeUnknown, minutesAgo: 5, salt: 12),
            delivery(.draftPullRequest(remote: "origin", base: nil), .failed(code: "git.github_credential_unavailable"), minutesAgo: 12, salt: 13),
            marking,
            markFailed,
            delivery(.draftPullRequest(remote: "origin", base: nil), .completed(head: head, branch: branch, pullRequest: pullRequest), minutesAgo: 45, salt: 16),
            delivery(.push(remote: "origin"), .completed(head: head, branch: branch, pullRequest: nil), minutesAgo: 50, salt: 17),
            delivery(.commit, .completed(head: head, branch: branch, pullRequest: nil), minutesAgo: 55, automatic: true, salt: 18),
            delivery(.commit, .outcomeUnknown, minutesAgo: 70, acknowledged: true, salt: 19),
            delivery(.branch(name: branch), .completed(head: head, branch: branch, pullRequest: nil), minutesAgo: 80, salt: 20),
            delivery(.branch(name: branch), .failed(code: "git.branch_exists"), minutesAgo: 85, salt: 21),
            delivery(.push(remote: "origin"), .failed(code: "git.index_locked"), minutesAgo: 90, salt: 22),
        ]
        session.deliveries.previewSeed(progress: nil, history: history, for: key(session))
        session.deliveries.previewSeedAcknowledgement(.marking, for: marking.id)
        session.deliveries.previewSeedAcknowledgement(.failed, for: markFailed.id)
    }

    // MARK: - Status line gallery

    struct Example: Identifiable {
        let id: String
        let ref: ConversationRef
    }

    /// One session with a task per status-line state.
    static func gallerySession() -> DesktopSession {
        DesktopSession.preview { session in
            DesktopPreviewData.waitingWithChanges(session)
            for (index, seed) in gallerySeeds.enumerated() {
                let conversationID = galleryConversationID(index)
                session.deliveries.previewSeed(
                    progress: seed.progress,
                    history: seed.history(conversationID),
                    for: key(session, conversationID: conversationID)
                )
            }
        }
    }

    static func galleryExamples(_ session: DesktopSession) -> [Example] {
        gallerySeeds.enumerated().map { index, seed in
            Example(
                id: seed.name,
                ref: ConversationRef(
                    conversationID: galleryConversationID(index),
                    planeRegistryID: session.localPlaneRegistryID
                )
            )
        }
    }

    private static func galleryConversationID(_ index: Int) -> UUID {
        UUID(uuidString: String(format: "7A3C0000-0000-4000-8000-%012ld", 900 + index))!
    }

    private struct GallerySeed {
        let name: String
        var progress: DeliveryCoordinator.Progress?
        var history: (UUID) -> [JetGitDelivery] = { _ in [] }
    }

    private static var gallerySeeds: [GallerySeed] {
        let prDelivery = delivery(
            .draftPullRequest(remote: "origin", base: nil),
            .completed(head: head, branch: branch, pullRequest: pullRequest),
            minutesAgo: 1,
            salt: 40
        )
        let pushUnknown = delivery(.push(remote: "origin"), .outcomeUnknown, minutesAgo: 1, salt: 41)
        return [
            GallerySeed(name: "running", progress: progress(
                [.branch, .commit, .push],
                [.branch: .completed(nil), .commit: .pending],
                isRunning: true
            )),
            GallerySeed(name: "running, can't reach", progress: progress(
                [.branch, .commit, .push],
                [.branch: .completed(nil), .commit: .completed(nil), .push: .pending],
                isRunning: true,
                lastCheckFailed: true
            )),
            GallerySeed(name: "saved", progress: progress(
                [.branch, .commit],
                [.branch: .completed(nil), .commit: .completed(nil)],
                isRunning: false
            )),
            GallerySeed(name: "pushed", progress: progress(
                [.branch, .commit, .push],
                [.branch: .completed(nil), .commit: .completed(nil), .push: .completed(nil)],
                isRunning: false
            )),
            GallerySeed(name: "pull request", progress: progress(
                [.branch, .commit, .push, .draftPullRequest],
                [.branch: .completed(nil), .commit: .completed(nil), .push: .completed(nil), .draftPullRequest: .completed(prDelivery)],
                isRunning: false
            )),
            GallerySeed(name: "branch exists", progress: progress(
                [.branch, .commit],
                [.branch: .failed(code: "git.branch_exists", delivery: nil)],
                isRunning: false
            )),
            GallerySeed(name: "credential", history: { conversationID in [
                delivery(
                    .draftPullRequest(remote: "origin", base: nil),
                    .failed(code: "git.github_credential_unavailable"),
                    minutesAgo: 2,
                    conversationID: conversationID,
                    salt: 42
                ),
            ] }),
            GallerySeed(name: "unconfirmed", progress: progress(
                [.branch, .commit, .push],
                [.branch: .completed(nil), .commit: .completed(nil), .push: .unconfirmed(pushUnknown)],
                isRunning: false
            )),
            GallerySeed(name: "uncertain", progress: progress(
                [.branch, .commit],
                [.branch: .completed(nil), .commit: .admissionUncertain],
                isRunning: false
            )),
            GallerySeed(name: "acknowledged", history: { conversationID in [
                delivery(
                    .push(remote: "origin"),
                    .outcomeUnknown,
                    minutesAgo: 3,
                    conversationID: conversationID,
                    acknowledged: true,
                    salt: 43
                ),
            ] }),
            GallerySeed(name: "not started", progress: progress(
                [.branch, .commit, .push],
                [.branch: .completed(nil), .commit: .completed(nil)],
                isRunning: false
            )),
        ]
    }

    // MARK: - Sheet scenes

    /// The sheet on a window-coloured backdrop (harness captures can't show sheets).
    static func sheet(
        _ id: String,
        height: CGFloat,
        mode: KeepChangesMode = .plan,
        seed: @escaping @MainActor (KeepChangesModel, DesktopSession) -> Void
    ) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id, size: CGSize(width: 640, height: height)) {
            let session = DesktopSession.preview { DesktopPreviewData.waitingWithChanges($0) }
            let ref = ConversationRef(conversationID: task.id, planeRegistryID: session.localPlaneRegistryID)
            return AnyView(
                KeepChangesSheet(session: session, ref: ref, mode: mode) { model in
                    seed(model, session)
                }
                .background(.background, in: RoundedRectangle(cornerRadius: JetDesign.fieldRadius))
                .overlay {
                    RoundedRectangle(cornerRadius: JetDesign.fieldRadius)
                        .strokeBorder(.separator)
                }
                .padding(.top, 24)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                .background(.background.secondary)
                .tint(JetDesign.accent)
            )
        }
    }
}

/// Every status-line state at the reading width and at a narrow 320 points.
private struct KeepChangesStatusGallery: View {
    let session: DesktopSession

    var body: some View {
        let examples = KeepChangesPreviewData.galleryExamples(session)
        ScrollView {
            HStack(alignment: .top, spacing: 32) {
                column(examples, width: JetDesign.readingWidth)
                column(examples, width: 320)
            }
            .padding(24)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .background(.background)
        .tint(JetDesign.accent)
    }

    private func column(_ examples: [KeepChangesPreviewData.Example], width: CGFloat) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            ForEach(examples) { example in
                VStack(alignment: .leading, spacing: 6) {
                    Text(verbatim: example.id)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.tertiary)
                    KeepChangesStatusLine(session: session, ref: example.ref)
                }
                Divider()
            }
        }
        .frame(width: width, alignment: .leading)
    }
}
#endif
