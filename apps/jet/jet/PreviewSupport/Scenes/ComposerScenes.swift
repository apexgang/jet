#if DEBUG
import SwiftUI

// `DesktopSession.preview` marks the one-time notification offer as shown, so
// only `composer-notification-offer` below (which clears it) shows the offer.
extension DesktopPreviewScenes {
    /// Composer states (WP7), 1000×260.
    @MainActor static var composer: [DesktopPreviewScene] {
        [
            composerScene("composer-idle") { DesktopPreviewData.waitingWithChanges($0) },
            composerScene("composer-working-followup", focused: true) {
                DesktopPreviewData.working($0)
                $0.draft = "Also add a test for the logout redirect."
            },
            composerScene("composer-offline") {
                DesktopPreviewData.offline($0)
                $0.draft = "Also check the redirect after signing out."
            },
            composerScene("composer-too-long", colorScheme: .light) {
                DesktopPreviewData.waitingWithChanges($0)
                $0.draft = ComposerPreviewData.longDraft
            },
            composerScene("composer-git-unconfirmed", colorScheme: .light) {
                DesktopPreviewData.waitingWithChanges($0)
                ComposerPreviewData.recordUnconfirmedDelivery($0)
            },
            composerScene("composer-sign-in") { ComposerPreviewData.waitingForSignIn($0) },
            composerScene("composer-interrupt-reply", focused: true) {
                DesktopPreviewData.waitingWithChanges($0)
                $0.interruptThenReply = true
                $0.composerPlaceholderOverride = "Tell Claude Code what to do instead…"
            },
            composerScene("composer-first-message", colorScheme: .light) {
                ComposerPreviewData.firstMessage($0)
            },
            composerScene("composer-notification-offer") {
                DesktopPreviewData.waitingWithChanges($0)
                $0.memory.notificationOfferShown = false
            },
        ]
    }

    /// New Task, first-run and Add Project states (WP7), 1000×760.
    @MainActor static var newTask: [DesktopPreviewScene] {
        [
            newTaskScene("new-task-two-computers") { ComposerPreviewData.twoComputers($0) },
            newTaskScene("new-task-reconnecting", colorScheme: .light) {
                $0.setupState = .failed(ComposerPreviewData.startFailed)
                $0.connectionState = .reconnecting(attempt: 2)
                $0.setupRetryAttempt = 2
                $0.open(.newTask)
            },
            newTaskScene("new-task-helper-failed") {
                $0.setupState = .failed(ComposerPreviewData.installIncomplete)
                $0.connectionState = .failed(ComposerPreviewData.installIncomplete)
                $0.open(.newTask)
            },
            newTaskScene("new-task-no-project") {
                DesktopPreviewData.connect($0)
                ComposerPreviewData.useLocalSnapshot($0, ComposerPreviewData.snapshot(projects: []))
                $0.selectedProjectID = nil
                $0.open(.newTask)
            },
            newTaskScene("new-task-no-assistant", colorScheme: .light) {
                DesktopPreviewData.connect($0)
                ComposerPreviewData.useLocalSnapshot($0, ComposerPreviewData.snapshot(crafts: []))
                $0.chosenCraftID = nil
                $0.open(.newTask)
            },
            newTaskScene("new-task-starting", colorScheme: .light) {
                DesktopPreviewData.connect($0)
                $0.open(.newTask)
                $0.draft = "Add a CSV export button to the reports page."
                $0.userOperation = .starting
            },
            newTaskScene("new-task-narrow", size: CGSize(width: 480, height: 760), colorScheme: .light) {
                ComposerPreviewData.twoComputers($0)
            },
            addProjectScene("add-project-choose") { session in
                AddProjectModel(session: session, planeRegistryID: session.localPlaneRegistryID)
            },
            addProjectScene("add-project-confirm") { session in
                let model = AddProjectModel(session: session, planeRegistryID: session.localPlaneRegistryID)
                model.seedForPreview(.reviewed(ComposerPreviewData.mobileApp))
                return model
            },
            addProjectScene("add-project-subfolder", colorScheme: .light) { session in
                let model = AddProjectModel(session: session, planeRegistryID: session.localPlaneRegistryID)
                model.seedForPreview(.reviewed(ComposerPreviewData.mobileAppSubfolder))
                return model
            },
            addProjectScene("add-project-not-git", colorScheme: .light) { session in
                let model = AddProjectModel(session: session, planeRegistryID: session.localPlaneRegistryID)
                model.seedForPreview(.reviewed(ComposerPreviewData.downloads))
                return model
            },
            addProjectScene("add-project-remote", colorScheme: .light) { session in
                ComposerPreviewData.addStudioMac(session)
                return AddProjectModel(session: session, planeRegistryID: ComposerPreviewData.studioID)
            },
            addProjectScene("add-project-already-added", colorScheme: .light) { session in
                let model = AddProjectModel(session: session, planeRegistryID: session.localPlaneRegistryID)
                model.seedForPreview(.alreadyAdded(DesktopPreviewData.webApp))
                return model
            },
        ]
    }

    // MARK: - Scene builders

    @MainActor private static func composerScene(
        _ id: String,
        colorScheme: ColorScheme? = nil,
        focused: Bool = false,
        seed: @escaping @MainActor (DesktopSession) -> Void
    ) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id, size: CGSize(width: 1000, height: 260), colorScheme: colorScheme) {
            let session = DesktopSession.preview { session in
                // Only the offer scene shows the one-time notification offer.
                session.memory.notificationOfferShown = true
                seed(session)
            }
            return AnyView(ComposerSceneHost(session: session, focused: focused))
        }
    }

    @MainActor private static func newTaskScene(
        _ id: String,
        size: CGSize = CGSize(width: 1000, height: 760),
        colorScheme: ColorScheme? = nil,
        seed: @escaping @MainActor (DesktopSession) -> Void
    ) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id, size: size, colorScheme: colorScheme) {
            AnyView(NewTaskSceneHost(session: .preview(configure: seed)))
        }
    }

    @MainActor private static func addProjectScene(
        _ id: String,
        colorScheme: ColorScheme? = nil,
        model: @escaping @MainActor (DesktopSession) -> AddProjectModel
    ) -> DesktopPreviewScene {
        DesktopPreviewScene(id: id, size: CGSize(width: 480, height: 340), colorScheme: colorScheme) {
            let session = DesktopSession.preview { DesktopPreviewData.connect($0) }
            return AnyView(AddProjectSceneHost(session: session, model: model(session)))
        }
    }
}

// MARK: - Hosts

/// The composer at the bottom of an empty task area.
private struct ComposerSceneHost: View {
    let session: DesktopSession
    let focused: Bool
    @FocusState private var composerFocused: Bool
    /// Stands in for the sidebar, which holds focus in the real window. Without it
    /// the window makes the only text view first responder.
    @FocusState private var elsewhereFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            Color.clear
                .frame(width: 1, height: 1)
                .focusable()
                .focusEffectDisabled()
                .focused($elsewhereFocused)
                .accessibilityHidden(true)
            Spacer(minLength: 0)
            WorkspaceComposer(session: session, composerFocused: $composerFocused)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .tint(JetDesign.accent)
        .background(.background)
        .task {
            if focused {
                composerFocused = true
            } else {
                elsewhereFocused = true
            }
        }
    }
}

/// New Task as it first appears: focus stays in the sidebar, stood in for here.
private struct NewTaskSceneHost: View {
    let session: DesktopSession
    @FocusState private var composerFocused: Bool
    @FocusState private var elsewhereFocused: Bool

    var body: some View {
        NewTaskView(session: session, composerFocused: $composerFocused)
            .overlay(alignment: .topLeading) {
                Color.clear
                    .frame(width: 1, height: 1)
                    .focusable()
                    .focusEffectDisabled()
                    .focused($elsewhereFocused)
                    .accessibilityHidden(true)
            }
            .tint(JetDesign.accent)
            .background(.background)
            .task { elsewhereFocused = true }
    }
}

private struct AddProjectSceneHost: View {
    let session: DesktopSession
    let model: AddProjectModel

    var body: some View {
        AddProjectSheet(session: session, model: model)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .tint(JetDesign.accent)
            .background(.background)
    }
}

// MARK: - Data

@MainActor
private enum ComposerPreviewData {
    static let studioID = uuid("7A3C0000-0000-4000-8000-000000000701")
    static let studioPlaneID = uuid("7A3C0000-0000-4000-8000-000000000702")
    static let apiServer = JetProjectSummary(
        id: uuid("7A3C0000-0000-4000-8000-000000000703"),
        root: "/Users/alex/code/api-server"
    )
    static let deliveryID = uuid("7A3C0000-0000-4000-8000-000000000704")

    /// About 70,000 bytes, over the 65,536-byte prompt limit.
    static let longDraft = String(
        repeating: "Here is the full log from the failing checkout run so you can see every step. ",
        count: 898
    )

    static let mobileApp = JetProjectPreview(
        root: "/Users/alex/code/mobile-app",
        registrability: .registrable(detail: "main working tree, Git LFS not installed")
    )
    static let mobileAppSubfolder = JetProjectPreview(
        root: "/Users/alex/code/mobile-app/src/screens",
        registrability: .unavailable(
            verdict: "inside_working_tree",
            detail: "Choose the repository root at /Users/alex/code/mobile-app."
        ),
        suggestedRoot: "/Users/alex/code/mobile-app"
    )
    static let downloads = JetProjectPreview(
        root: "/Users/alex/Downloads",
        registrability: .unavailable(
            verdict: "not_a_repository",
            detail: "Choose the top folder of a Git working tree."
        )
    )

    static let installIncomplete = JetPresentationError(
        category: .unavailable,
        code: "core.install_incomplete",
        message: "Jet's helper is incomplete. Reinstall Jet from its latest release.",
        retryable: false
    )
    static let startFailed = JetPresentationError(
        category: .unavailable,
        code: "core.start_failed",
        message: "Jet couldn't start its helper on this Mac.",
        retryable: true
    )

    /// This Mac's snapshot with other projects or assistants.
    static func snapshot(
        projects: [JetProjectSummary]? = nil,
        crafts: [JetInstalledCraft]? = nil
    ) -> JetSetupSnapshot {
        let base = DesktopPreviewData.setupSnapshot
        let capabilities = crafts.map { crafts in
            JetCapabilitySummary(
                coreVersion: base.capabilities.coreVersion,
                platform: base.capabilities.platform,
                externalTools: base.capabilities.externalTools,
                harnesses: crafts.flatMap(\.harnesses),
                crafts: crafts,
                credentialStore: base.capabilities.credentialStore,
                degraded: base.capabilities.degraded
            )
        } ?? base.capabilities
        return JetSetupSnapshot(
            status: base.status,
            capabilities: capabilities,
            projects: projects.map { JetProjectList(cursor: base.projects.cursor, projects: $0) } ?? base.projects,
            accounts: base.accounts,
            pairing: base.pairing
        )
    }

    static func useLocalSnapshot(_ session: DesktopSession, _ snapshot: JetSetupSnapshot) {
        session.setupState = .ready(snapshot)
        session.updatePlane(session.localPlaneRegistryID) { $0.snapshot = snapshot }
    }

    /// A second, connected computer with one project.
    static func addStudioMac(_ session: DesktopSession) {
        let base = DesktopPreviewData.setupSnapshot
        let snapshot = JetSetupSnapshot(
            status: JetPlaneStatus(
                cursor: 120,
                planeID: studioPlaneID,
                daemonStarts: 1,
                startedAtUnixMilliseconds: base.status.startedAtUnixMilliseconds,
                coreVersion: base.status.coreVersion,
                security: nil,
                recovery: nil
            ),
            capabilities: base.capabilities,
            projects: JetProjectList(cursor: 120, projects: [apiServer]),
            accounts: base.accounts,
            pairing: base.pairing
        )
        session.planes.append(JetPlanePresentation(
            id: studioID,
            name: "Studio Mac",
            endpoint: "alex@studio.local",
            isLocal: false,
            planeID: studioPlaneID,
            connection: .connected(DesktopPreviewData.negotiation),
            snapshot: snapshot,
            failure: nil,
            conversationCursor: 120
        ))
    }

    /// New Task moved to Studio Mac, with a draft and the "Project changed" notice.
    static func twoComputers(_ session: DesktopSession) {
        DesktopPreviewData.connect(session)
        addStudioMac(session)
        session.open(.newTask)
        session.draft = "Add rate limiting to the public API."
        session.chooseNewTaskComputer(studioID)
    }

    /// The open task waits for the assistant's sign-in.
    static func waitingForSignIn(_ session: DesktopSession) {
        DesktopPreviewData.working(session)
        let run = DesktopPreviewData.makeRun(for: DesktopPreviewData.loginRedirect, lifecycle: .active)
        session.runExecution = DesktopPreviewData.makeExecution(run, activity: .waitingForAuth, needsAttention: true)
    }

    /// An uncertain Git step pauses the task until it is checked.
    static func recordUnconfirmedDelivery(_ session: DesktopSession) {
        guard let conversationID = session.selectedConversationID else { return }
        let delivery = JetGitDelivery(
            id: deliveryID,
            conversationID: conversationID,
            checkpoint: nil,
            operation: .push(remote: "origin"),
            policy: JetGitDeliveryPolicy(
                automatic: false,
                branch: false,
                commit: false,
                push: true,
                draftPullRequest: false,
                branchPrefix: "jet/"
            ),
            utilityJobID: nil,
            message: nil,
            acknowledgedBy: nil,
            outcome: .outcomeUnknown
        )
        session.gitDeliveries = [delivery]
        session.statusStore.recordGitDeliveries([delivery], conversationID: conversationID)
    }

    /// A task that exists but hasn't started: its first message picks the assistant.
    static func firstMessage(_ session: DesktopSession) {
        DesktopPreviewData.connect(session)
        let task = DesktopPreviewData.conversations[5]
        let snapshot = JetConversationSnapshot(
            cursor: DesktopPreviewData.headCursor,
            conversation: task,
            workspaceID: nil,
            workspaceRoot: nil,
            runs: []
        )
        session.sidebarSelection = .conversation
        session.selectedConversationID = task.id
        session.conversationSnapshot = snapshot
        session.runExecution = nil
        session.turnQueue = JetTurnQueue(cursor: DesktopPreviewData.headCursor, turns: [])
        session.timeline = []
        session.conversationFreshness = .live
        session.recordSelectedSnapshot(snapshot)
        session.draft = "Add a CSV export button to the reports page, next to Print."
    }

    private static func uuid(_ value: String) -> UUID {
        guard let id = UUID(uuidString: value) else {
            preconditionFailure("Invalid preview UUID \(value)")
        }
        return id
    }
}
#endif
