import SwiftUI

struct DeliveryWorkView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            WorkSectionHeader(
                title: "Delivery",
                detail: "Authoritative Git outcomes from this Conversation"
            ) {
                Button("Refresh") { Task { await session.loadGitDeliveries() } }
                    .disabled(session.gitDeliveryOperation != nil)
            }
            .padding(.horizontal, -14)
            .padding(.vertical, -12)

            if let reason = session.gitDeliveryUnavailableReason {
                Label(reason, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Picker("Operation", selection: $session.gitDeliveryChoice) {
                ForEach(JetGitDeliveryChoice.allCases, id: \.self) { choice in
                    Text(choice.title).tag(choice)
                }
            }
            .pickerStyle(.menu)

            deliveryFields

            if let request = previewRequest {
                LabeledContent("Destination", value: request.operation.destinationLabel)
                    .font(.caption)
                if let checkpoint = request.checkpoint {
                    LabeledContent("Checkpoint", value: checkpoint.label)
                        .font(.caption)
                }
            }

            HStack {
                Spacer()
                Button("Review \(session.gitDeliveryChoice.title)…") {
                    session.prepareGitDelivery()
                }
                .buttonStyle(.borderedProminent)
                .disabled(!session.canPrepareGitDelivery)
                .accessibilityIdentifier("git-delivery-review")
            }

            if let uncertain = session.gitDeliveryAdmissionUncertain {
                let reviewSummary: String = uncertain.reviewSummary
                VStack(alignment: .leading, spacing: 8) {
                    Label("Request admission is unknown", systemImage: "questionmark.diamond")
                        .font(.subheadline.weight(.semibold))
                    Text("Check the durable history first. Retrying uses the same command identity and unchanged request.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Text(reviewSummary)
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                    HStack {
                        Button("Check Status") { Task { await session.loadGitDeliveries() } }
                        Button("Retry Same Request") {
                            Task { await session.retryGitDeliveryAdmission() }
                        }
                    }
                    .controlSize(.small)
                }
                .padding(10)
                .background(.orange.opacity(0.1), in: RoundedRectangle(cornerRadius: 8))
            }

            if let notice = session.gitDeliveryNotice {
                VStack(alignment: .leading, spacing: 6) {
                    Text(notice)
                        .font(.caption)
                        .foregroundStyle(deliveryNoticeColor)
                        .accessibilityLabel(notice)
                    if let error = session.gitDeliveryError {
                        JetSettingsRecoveryButton(session: session, error: error)
                            .controlSize(.small)
                    }
                }
            }

            Divider()

            Text("Delivery history")
                .font(.subheadline.weight(.semibold))

            if session.gitDeliveryOperation == "refresh", session.gitDeliveries.isEmpty {
                ProgressView("Loading delivery history")
                    .controlSize(.small)
            } else if session.gitDeliveries.isEmpty {
                Text("No Git delivery has been requested for this Conversation.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(session.gitDeliveries) { delivery in
                    GitDeliveryRow(session: session, delivery: delivery)
                }
            }
        }
        .controlSize(.small)
        .padding(14)
        .confirmationDialog(
            session.gitDeliveryConfirmation.map { "Review \($0.operation.title)?" }
                ?? "Review delivery?",
            isPresented: Binding(
                get: { session.gitDeliveryConfirmation != nil },
                set: { if !$0 { session.cancelGitDeliveryConfirmation() } }
            ),
            titleVisibility: .visible
        ) {
            Button(session.gitDeliveryConfirmation?.operation.title ?? "Deliver") {
                Task { await session.confirmGitDelivery() }
            }
            Button("Cancel", role: .cancel, action: session.cancelGitDeliveryConfirmation)
        } message: {
            Text(
                session.gitDeliveryConfirmation.map {
                    "Destination: \($0.operation.destinationLabel). \($0.checkpoint?.label ?? "No checkpoint content"). Jet queues this as a durable Effect and reports each outcome separately."
                } ?? "Review the destination before delivery."
            )
        }
        .confirmationDialog(
            "Mark this outcome as reviewed?",
            isPresented: Binding(
                get: { session.gitDeliveryAcknowledgementConfirmation != nil },
                set: { if !$0 { session.cancelGitDeliveryAcknowledgement() } }
            ),
            titleVisibility: .visible
        ) {
            Button("Mark Reviewed") {
                Task { await session.confirmGitDeliveryAcknowledgement() }
            }
            Button("Cancel", role: .cancel, action: session.cancelGitDeliveryAcknowledgement)
        } message: {
            Text("Jet will release the uncertainty barrier after your review. It will not repeat the Git operation or claim that it succeeded or failed.")
        }
    }

    @ViewBuilder
    private var deliveryFields: some View {
        switch session.gitDeliveryChoice {
        case .branch:
            TextField("New branch name", text: $session.gitBranchName)
                .textFieldStyle(.roundedBorder)
        case .commit:
            Text("Jet commits the latest retained Turn checkpoint. The Plane generates and records the exact commit message.")
                .font(.caption)
                .foregroundStyle(.secondary)
        case .push:
            TextField("Remote", text: $session.gitRemoteName)
                .textFieldStyle(.roundedBorder)
        case .draftPullRequest:
            TextField("GitHub remote", text: $session.gitRemoteName)
                .textFieldStyle(.roundedBorder)
            TextField("Base branch, or leave empty for repository default", text: $session.gitBaseBranch)
                .textFieldStyle(.roundedBorder)
        }
    }

    private var previewRequest: JetGitDeliveryRequest? {
        guard let conversationID = session.selectedConversationID,
              let diff = session.workDiff
        else { return nil }
        let checkpoint = JetGitCheckpoint(runID: diff.runID, turn: diff.latestTurn)
        switch session.gitDeliveryChoice {
        case .branch:
            return JetGitDeliveryRequest(
                conversationID: conversationID,
                checkpoint: nil,
                operation: .branch(name: session.gitBranchName.isEmpty ? "Enter a branch" : session.gitBranchName)
            )
        case .commit:
            return JetGitDeliveryRequest(
                conversationID: conversationID,
                checkpoint: checkpoint,
                operation: .commit
            )
        case .push:
            return JetGitDeliveryRequest(
                conversationID: conversationID,
                checkpoint: nil,
                operation: .push(remote: session.gitRemoteName)
            )
        case .draftPullRequest:
            return JetGitDeliveryRequest(
                conversationID: conversationID,
                checkpoint: checkpoint,
                operation: .draftPullRequest(
                    remote: session.gitRemoteName,
                    base: session.gitBaseBranch.isEmpty ? nil : session.gitBaseBranch
                )
            )
        }
    }

    private var deliveryNoticeColor: Color {
        session.gitDeliveryError == nil ? .secondary : .red
    }
}

private struct GitDeliveryRow: View {
    let session: DesktopSession
    let delivery: JetGitDelivery

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Label(delivery.operation.title, systemImage: statusSymbol)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(statusColor)
                Spacer()
                Text(delivery.outcome.title)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            Text(delivery.operation.destinationLabel)
                .font(.caption2.monospaced())
                .textSelection(.enabled)
            if let checkpoint = delivery.checkpoint {
                Text("\(checkpoint.label) · Run \(checkpoint.runID.uuidString.prefix(8))")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            if let message = delivery.message {
                Text(message.title)
                    .font(.caption)
                    .lineLimit(2)
            }
            outcomeDetails
            if delivery.canRetry || delivery.needsAcknowledgement {
                HStack {
                    Spacer()
                    if delivery.canRetry {
                        Button("Review Retry…") { session.reviewRetry(delivery) }
                    }
                    if delivery.needsAcknowledgement {
                        Button("Mark Reviewed…") {
                            session.reviewGitDeliveryAcknowledgement(delivery)
                        }
                    }
                }
            }
        }
        .padding(10)
        .background(Color.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("git-delivery-\(delivery.id.uuidString.lowercased())")
    }

    @ViewBuilder
    private var outcomeDetails: some View {
        switch delivery.outcome {
        case .pending:
            Text("The outbox accepted this Effect and is still reconciling its result.")
                .font(.caption2)
                .foregroundStyle(.secondary)
        case let .completed(head, branch, pullRequest):
            Text("HEAD \(head.prefix(12))")
                .font(.caption2.monospaced())
            if let branch { Text("Branch \(branch)").font(.caption2.monospaced()) }
            if let pullRequest {
                Text(pullRequest)
                    .font(.caption2.monospaced())
                    .textSelection(.enabled)
            }
        case let .failed(code):
            Text("Jet confirmed failure: \(code)")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        case .outcomeUnknown:
            Text(
                delivery.acknowledgedBy == nil
                    ? "Jet could not establish whether the external operation happened. It will not retry automatically."
                    : "A user reviewed this unknown outcome. Jet did not repeat the operation."
            )
            .font(.caption2)
            .foregroundStyle(.secondary)
        }
    }

    private var statusSymbol: String {
        switch delivery.outcome {
        case .pending: "clock"
        case .completed: "checkmark.circle.fill"
        case .failed: "xmark.circle.fill"
        case .outcomeUnknown: "questionmark.diamond.fill"
        }
    }

    private var statusColor: Color {
        switch delivery.outcome {
        case .pending: .secondary
        case .completed: .green
        case .failed: .red
        case .outcomeUnknown: .orange
        }
    }
}

@MainActor
private func makeDeliveryPreviewSession() -> DesktopSession {
    let conversationID = UUID(uuidString: "00000000-0000-0000-0000-000000000020")!
    let checkpoint = JetGitCheckpoint(
        runID: UUID(uuidString: "00000000-0000-0000-0000-000000000010")!,
        turn: 3
    )
    let policy = JetGitDeliveryPolicy(
        automatic: false,
        branch: true,
        commit: true,
        push: true,
        draftPullRequest: true,
        branchPrefix: "jet/"
    )
    let session = DesktopSession()
    session.gitDeliveries = [
        JetGitDelivery(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000040")!,
            conversationID: conversationID,
            checkpoint: checkpoint,
            operation: .commit,
            policy: policy,
            utilityJobID: nil,
            message: JetGitMessage(title: "Updated desktop delivery", body: "", fallbackReason: nil),
            acknowledgedBy: nil,
            outcome: .pending
        ),
        JetGitDelivery(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000041")!,
            conversationID: conversationID,
            checkpoint: checkpoint,
            operation: .draftPullRequest(remote: "origin", base: "main"),
            policy: policy,
            utilityJobID: nil,
            message: JetGitMessage(title: "Updated desktop delivery", body: "", fallbackReason: nil),
            acknowledgedBy: nil,
            outcome: .completed(
                head: String(repeating: "a", count: 40),
                branch: "jet/delivery",
                pullRequest: "https://github.com/example/jet/pull/7"
            )
        ),
        JetGitDelivery(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000042")!,
            conversationID: conversationID,
            checkpoint: nil,
            operation: .push(remote: "origin"),
            policy: policy,
            utilityJobID: nil,
            message: nil,
            acknowledgedBy: nil,
            outcome: .failed(code: "git.policy_changed")
        ),
        JetGitDelivery(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000043")!,
            conversationID: conversationID,
            checkpoint: nil,
            operation: .branch(name: "jet/reviewed"),
            policy: policy,
            utilityJobID: nil,
            message: nil,
            acknowledgedBy: nil,
            outcome: .outcomeUnknown
        ),
    ]

    return session
}

#Preview("Delivery outcomes") {
    ScrollView {
        DeliveryWorkView(session: makeDeliveryPreviewSession())
    }
    .frame(width: 420, height: 760)
}
