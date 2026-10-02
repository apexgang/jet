import Foundation

// Keep Changes and Git activity (design §6.9, §6.14 Changes menu). No stored state:
// progress and history live in `deliveries` (DeliveryCoordinator).
extension DesktopSession {
    // MARK: - Refs and history

    /// The open task, for Keep Changes. Nil in fixture mode or without a task.
    var selectedDeliveryRef: ConversationRef? {
        guard usesLivePlane else { return nil }
        return selectedConversationRef
    }

    func deliveryKey(for ref: ConversationRef) -> DeliveryCoordinator.Key {
        DeliveryCoordinator.Key(planeRegistryID: ref.planeRegistryID, conversationID: ref.conversationID)
    }

    /// The task's Git deliveries, newest first: the coordinator's once it checked
    /// them, otherwise what the session loaded for the open task.
    func deliveryHistory(for ref: ConversationRef) -> [JetGitDelivery] {
        let key = deliveryKey(for: ref)
        if deliveries.hasHistory(for: key) { return deliveries.history(for: key) }
        guard ref.conversationID == selectedConversationID else { return [] }
        return DeliveryCoordinator.newestFirst(gitDeliveries.filter { $0.conversationID == ref.conversationID })
    }

    func deliveryHistoryStatus(for ref: ConversationRef) -> DeliveryCoordinator.HistoryStatus {
        let status = deliveries.historyStatus(for: deliveryKey(for: ref))
        if status == .unknown, ref.conversationID == selectedConversationID, !gitDeliveries.isEmpty {
            return .loaded
        }
        return status
    }

    /// Checks the task's deliveries, then keeps checking every two seconds while a
    /// step is pending in jetd. A running plan checks its own steps.
    func observeDeliveries(for ref: ConversationRef) async {
        guard !isPreviewSession, usesLivePlane else { return }
        let key = deliveryKey(for: ref)
        await deliveries.refresh(for: key)
        while !Task.isCancelled {
            let isRunning = deliveries.progress(for: key)?.isRunning == true
            let hasPending = deliveries.history(for: key).contains { $0.outcome == .pending }
            guard isRunning || hasPending else { return }
            do {
                try await deliveries.sleep(deliveries.pollInterval)
            } catch {
                return
            }
            if deliveries.progress(for: key)?.isRunning != true {
                await deliveries.refresh(for: key)
            }
        }
    }

    /// Checks tasks remembered from the last launch once their computer connects,
    /// so an unconfirmed Git step puts its task back in Needs You.
    func refreshRestoredDeliveries() async {
        guard !isPreviewSession, usesLivePlane else { return }
        await deliveries.refreshRestoredChains { [weak self] in self?.isPlaneConnected($0) == true }
    }

    // MARK: - Computers

    func deliveryComputerName(_ ref: ConversationRef) -> String {
        planes.first { $0.id == ref.planeRegistryID }?.name ?? String(localized: "This Mac")
    }

    func isDeliveryConnected(_ ref: ConversationRef) -> Bool {
        isPlaneConnected(ref.planeRegistryID)
    }

    /// The task works directly in the project folder (no separate working copy).
    func deliveryIsLocalCheckout(_ ref: ConversationRef) -> Bool {
        guard let snapshot = conversationSnapshot, snapshot.conversation.id == ref.conversationID else {
            return false
        }
        return snapshot.workspaceID == nil
    }

    // MARK: - Keep Changes sheet

    /// What the Keep Changes sheet needs: the latest changes, the task's Git
    /// history and the branch prefix. Nothing is changed.
    func loadKeepChangesFacts(for ref: ConversationRef) async throws -> KeepChangesFacts {
        let key = deliveryKey(for: ref)
        let isSelected = ref.conversationID == selectedConversationID
        let selectedSnapshot = conversationSnapshot.flatMap {
            $0.conversation.id == ref.conversationID ? $0 : nil
        }
        var snapshot = selectedSnapshot
        var diff: JetChangeDiff?
        var prefix: String?

        if isPreviewSession {
            diff = isSelected ? workDiff : nil
        } else {
            let client = try await client(for: ref.planeRegistryID)
            if snapshot == nil {
                snapshot = try await client.conversation(ref.conversationID)
            }
            let run = (isSelected ? selectedRun : nil) ?? snapshot?.runs.last
            if let run {
                if isSelected, let workDiff, workDiff.runID == run.id, workDiff.scope == .current {
                    diff = workDiff
                } else {
                    diff = try await client.changeDiff(runID: run.id, scope: .current)
                }
            }
            guard await deliveries.refresh(for: key) else {
                throw JetClientFailure.presentation(.offline)
            }
            if deliveries.history(for: key).first == nil,
               let prefixKey = SettingKey(rawValue: "git.branch_prefix"),
               let settings = try? await client.settings(scope: .conversation(ref.conversationID)),
               case let .text(value) = settings.value(for: prefixKey)
            {
                prefix = value
            }
        }

        let history = deliveryHistory(for: ref)
        let title = conversations.first { $0.id == ref.conversationID }?.title
            ?? snapshot?.conversation.title
            ?? ""
        let checkpoint = diff.flatMap { diff in
            diff.latestTurn > 0 ? JetGitCheckpoint(runID: diff.runID, turn: diff.latestTurn) : nil
        }
        return KeepChangesFacts(
            conversationID: ref.conversationID,
            title: title,
            projectName: projectName(for: ref.conversationID),
            assistantName: assistantName(for: ref.conversationID),
            computerName: deliveryComputerName(ref),
            isRemote: !isLocalPlane(ref.planeRegistryID),
            isLocalCheckout: snapshot.map { $0.workspaceID == nil } ?? false,
            fileCount: Int(diff?.totalFiles ?? 0),
            checkpoint: checkpoint,
            branchPrefix: history.first?.policy.branchPrefix ?? prefix ?? "jet/",
            history: history
        )
    }

    /// Starts the reviewed plan and closes the sheet. Progress shows in the Keep
    /// status line; Details doesn't open.
    func startKeepChanges(_ plan: DeliveryCoordinator.Plan, for ref: ConversationRef) {
        if !isPreviewSession {
            deliveries.start(plan, for: deliveryKey(for: ref))
        }
        dismissSheet()
    }

    /// Opens the Keep Changes sheet for a task (Continue…, Choose Another Name…).
    func presentKeepChanges(for ref: ConversationRef, mode: KeepChangesMode = .plan) {
        presentedSheet = .keepChanges(ref, mode)
    }

    // MARK: - Status line

    func keepChangesSummary(for ref: ConversationRef) -> KeepChangesSummary? {
        let history = deliveryHistory(for: ref)
        var acknowledgements: [UUID: DeliveryCoordinator.Acknowledgement] = [:]
        for delivery in history where delivery.outcome == .outcomeUnknown {
            acknowledgements[delivery.id] = deliveries.acknowledgement(for: delivery.id)
        }
        return KeepChangesSummary.make(
            progress: deliveries.progress(for: deliveryKey(for: ref)),
            history: history,
            context: KeepChangesSummary.Context(
                computer: deliveryComputerName(ref),
                assistant: assistantName(for: ref.conversationID),
                isLocalCheckout: deliveryIsLocalCheckout(ref),
                acknowledgements: acknowledgements
            )
        )
    }

    /// A step's title for a confirmation, such as "Push to origin".
    func stepTitle(for step: DeliveryCoordinator.Step, key: DeliveryCoordinator.Key) -> String {
        let plan = deliveries.progress(for: key)?.plan
        return GitStepCopy.title(
            step,
            branch: plan.map(\.branchName).flatMap { $0.isEmpty ? nil : $0 },
            remote: plan?.remote ?? "origin"
        )
    }

    func stepTitle(for delivery: JetGitDelivery, key: DeliveryCoordinator.Key) -> String {
        GitStepCopy.title(
            delivery.step,
            branch: delivery.branchName ?? DeliveryCoordinator.knownBranch(in: deliveries.history(for: key)),
            remote: delivery.remoteName ?? "origin"
        )
    }

    // MARK: - Changes menu targets

    var canCheckGitStatus: Bool {
        selectedDeliveryRef.map(isDeliveryConnected) ?? false
    }

    /// Check Git Status: reads the open task's deliveries again.
    func checkGitStatus() async {
        guard let ref = selectedDeliveryRef else { return }
        await checkGitStatus(for: ref)
    }

    func checkGitStatus(for ref: ConversationRef) async {
        guard !isPreviewSession else { return }
        await deliveries.refresh(for: deliveryKey(for: ref))
        if ref.conversationID == selectedConversationID {
            await loadGitDeliveries(startObservation: false)
        }
    }

    /// The open task's newest Git step that couldn't be confirmed and isn't checked.
    var uncheckedGitDelivery: JetGitDelivery? {
        selectedDeliveryRef.flatMap(uncheckedGitDelivery(for:))
    }

    func uncheckedGitDelivery(for ref: ConversationRef) -> JetGitDelivery? {
        deliveryHistory(for: ref).first {
            $0.needsAcknowledgement && deliveries.acknowledgement(for: $0.id) != .marked
        }
    }

    /// Mark as Checked…: asks for confirmation first (hosted by `keepChangesSupport`).
    func requestMarkAsChecked(_ delivery: JetGitDelivery? = nil) {
        guard let delivery = delivery ?? uncheckedGitDelivery, delivery.needsAcknowledgement else { return }
        let planeRegistryID = planeRegistryID(for: delivery.conversationID)
            ?? selectedDeliveryRef.flatMap { $0.conversationID == delivery.conversationID ? $0.planeRegistryID : nil }
            ?? localPlaneRegistryID
        let key = DeliveryCoordinator.Key(planeRegistryID: planeRegistryID, conversationID: delivery.conversationID)
        deliveries.pendingConfirmation = .markAsChecked(key, delivery)
    }

    /// Sends the acknowledgement. A failed attempt keeps its Command ID, so Try
    /// Again sends the same Command.
    func confirmMarkAsChecked(_ delivery: JetGitDelivery, for key: DeliveryCoordinator.Key) async {
        guard !isPreviewSession else { return }
        await deliveries.acknowledge(delivery, for: key) { [weak self] deliveryID, commandID in
            guard let self else { throw CancellationError() }
            _ = try await self.client(for: key.planeRegistryID)
                .acknowledgeGitDelivery(deliveryID: deliveryID, commandID: commandID)
        }
    }

    /// Try Again…: asks for confirmation first. Only `.retryStep` and
    /// `.retryDelivery` are accepted.
    func requestRetry(_ confirmation: DeliveryCoordinator.Confirmation) {
        switch confirmation {
        case .retryStep, .retryDelivery:
            deliveries.pendingConfirmation = confirmation
        case .markAsChecked:
            return
        }
    }

    /// Sends only the confirmed step again, as a new request.
    func confirmRetry(_ confirmation: DeliveryCoordinator.Confirmation) {
        guard !isPreviewSession else { return }
        switch confirmation {
        case let .retryStep(key, _):
            deliveries.retryFailedStep(for: key)
        case let .retryDelivery(key, delivery):
            deliveries.retry(delivery, for: key)
        case .markAsChecked:
            return
        }
    }
}
