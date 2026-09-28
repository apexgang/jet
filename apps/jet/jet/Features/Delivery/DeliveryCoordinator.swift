import Foundation
import Observation

/// Runs a reviewed Keep Changes plan as separate `DeliverGit` Commands, keyed by
/// computer and task. Each step keeps its own Command ID and exact body, is pinned
/// to the reviewed checkpoint, starts only after the previous step completed, and
/// is never retried automatically (design §6.9).
///
/// This file defines the API only; the Keep Changes work package implements it.
@MainActor
@Observable
final class DeliveryCoordinator {
    struct Key: Hashable, Sendable {
        let planeRegistryID: UUID
        let conversationID: UUID
    }

    enum Step: String, CaseIterable, Hashable, Sendable {
        case branch
        case commit
        case push
        case draftPullRequest
    }

    enum StepState: Equatable, Sendable {
        case notStarted
        case submitting
        case pending
        case completed(JetGitDelivery?)
        case failed(code: String, delivery: JetGitDelivery?)
        /// The Git step's outcome is unknown until the person checks it.
        case unconfirmed(JetGitDelivery?)
        /// Jet couldn't confirm the core received the request.
        case admissionUncertain
    }

    struct Plan: Equatable, Sendable {
        var steps: [Step]
        var branchName: String
        var remote: String
        var baseBranch: String?
        var checkpoint: JetGitCheckpoint?
    }

    struct Progress: Equatable, Sendable {
        var plan: Plan
        var states: [Step: StepState]
        var isRunning: Bool
    }

    typealias Submit = @MainActor (Key, JetGitDeliveryRequest, _ commandID: UUID) async throws -> UUID
    typealias Fetch = @MainActor (Key) async throws -> [JetGitDelivery]
    typealias OnDeliveries = @MainActor (Key, [JetGitDelivery]) -> Void

    private(set) var progressByKey: [Key: Progress] = [:]

    @ObservationIgnored private(set) var submit: Submit?
    @ObservationIgnored private(set) var fetch: Fetch?
    @ObservationIgnored private(set) var onDeliveries: OnDeliveries?

    func configure(
        submit: @escaping Submit,
        fetch: @escaping Fetch,
        onDeliveries: @escaping OnDeliveries
    ) {
        self.submit = submit
        self.fetch = fetch
        self.onDeliveries = onDeliveries
    }

    func progress(for key: Key) -> Progress? {
        progressByKey[key]
    }

    /// Starts a reviewed plan. Not implemented yet.
    func start(_ plan: Plan, for key: Key) {}

    /// Retries only the failed step, after the person confirmed it. Not implemented yet.
    func retryFailedStep(for key: Key) {}

    /// Sends the same uncertain request again with its retained Command ID. Not implemented yet.
    func resendUncertain(for key: Key) {}

    /// Refreshes the task's Git deliveries. Not implemented yet.
    func refresh(for key: Key) async {}
}
