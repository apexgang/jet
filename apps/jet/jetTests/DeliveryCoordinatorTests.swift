import Foundation
import Testing
@testable import jet

/// A fake jetd for Git delivery: it records every submitted Command, creates
/// deliveries with time-ordered IDs, and reports each one's outcome after a
/// number of pending checks.
@MainActor
final class FakeGitDeliveries {
    typealias Key = DeliveryCoordinator.Key

    enum Admission {
        /// jetd accepts the Command; the delivery reads pending for `pendingChecks`
        /// fetches, then `outcome`.
        case accept(JetGitDeliveryOutcome, pendingChecks: Int = 0)
        case refuse(Error)
    }

    struct Submission {
        let key: Key
        let request: JetGitDeliveryRequest
        let commandID: UUID
    }

    private struct Record {
        var delivery: JetGitDelivery
        var final: JetGitDeliveryOutcome
        var pendingChecks: Int
    }

    var admissions: [Admission] = []
    /// Admissions for one task, used before the shared queue.
    var admissionsByConversation: [UUID: [Admission]] = [:]
    var fetchErrors: [Error] = []
    /// Makes each fetch suspend, so concurrent callers overlap.
    var yieldsInFetch = false
    private(set) var submissions: [Submission] = []
    private(set) var events: [String] = []
    private(set) var fetchCount = 0
    private(set) var notified: [(key: Key, deliveries: [JetGitDelivery])] = []
    private(set) var sleeps: [Duration] = []
    private(set) var failedChecksAtSleep: [Bool] = []
    private var records: [Record] = []
    private var nextID = 0
    weak var coordinator: DeliveryCoordinator?

    static let completed = JetGitDeliveryOutcome.completed(head: "abc123", branch: "jet/fix-login", pullRequest: nil)

    func makeCoordinator(store: DeliveryChainStore = .inMemory()) -> DeliveryCoordinator {
        let coordinator = DeliveryCoordinator(store: store)
        coordinator.sleep = { [weak self] duration in await self?.recordSleep(duration) }
        coordinator.configure(
            submit: { [unowned self] key, request, commandID in try self.submit(key, request, commandID) },
            fetch: { [unowned self] key in
                if self.yieldsInFetch {
                    for _ in 0 ..< 5 { await Task.yield() }
                }
                return try self.fetch(key)
            },
            onDeliveries: { [unowned self] key, deliveries in self.notified.append((key, deliveries)) }
        )
        self.coordinator = coordinator
        return coordinator
    }

    /// A delivery jetd recorded without this coordinator submitting it.
    @discardableResult
    func record(
        _ key: Key,
        _ operation: JetGitOperation,
        checkpoint: JetGitCheckpoint? = nil,
        outcome: JetGitDeliveryOutcome
    ) -> JetGitDelivery {
        let delivery = makeDelivery(key, operation, checkpoint: checkpoint, outcome: outcome)
        records.append(Record(delivery: delivery, final: outcome, pendingChecks: 0))
        return delivery
    }

    func acknowledge(_ deliveryID: UUID) {
        guard let index = records.firstIndex(where: { $0.delivery.id == deliveryID }) else { return }
        let old = records[index].delivery
        records[index].delivery = JetGitDelivery(
            id: old.id,
            conversationID: old.conversationID,
            checkpoint: old.checkpoint,
            operation: old.operation,
            policy: old.policy,
            utilityJobID: nil,
            message: nil,
            acknowledgedBy: UUID(),
            outcome: old.outcome
        )
    }

    private func submit(_ key: Key, _ request: JetGitDeliveryRequest, _ commandID: UUID) throws -> UUID {
        submissions.append(Submission(key: key, request: request, commandID: commandID))
        events.append("submit \(DeliveryCoordinator.Step(request.operation).rawValue)")
        let admission: Admission
        if var queue = admissionsByConversation[key.conversationID], !queue.isEmpty {
            admission = queue.removeFirst()
            admissionsByConversation[key.conversationID] = queue
        } else {
            admission = admissions.isEmpty ? .accept(Self.completed) : admissions.removeFirst()
        }
        switch admission {
        case let .refuse(error):
            throw error
        case let .accept(outcome, pendingChecks):
            let delivery = makeDelivery(
                key,
                request.operation,
                checkpoint: request.checkpoint,
                outcome: pendingChecks > 0 ? .pending : outcome
            )
            records.append(Record(delivery: delivery, final: outcome, pendingChecks: pendingChecks))
            return delivery.id
        }
    }

    private func fetch(_ key: Key) throws -> [JetGitDelivery] {
        fetchCount += 1
        events.append("fetch")
        if !fetchErrors.isEmpty { throw fetchErrors.removeFirst() }
        var result: [JetGitDelivery] = []
        for index in records.indices where records[index].delivery.conversationID == key.conversationID {
            var record = records[index]
            if record.pendingChecks > 0 {
                record.pendingChecks -= 1
            } else if record.delivery.outcome != record.final {
                record.delivery = record.delivery.with(outcome: record.final)
            }
            records[index] = record
            result.append(record.delivery)
        }
        return result.reversed()
    }

    private func recordSleep(_ duration: Duration) {
        sleeps.append(duration)
        let failed = coordinator?.progressByKey.values.contains { $0.lastCheckFailed } ?? false
        failedChecksAtSleep.append(failed)
    }

    private func makeDelivery(
        _ key: Key,
        _ operation: JetGitOperation,
        checkpoint: JetGitCheckpoint?,
        outcome: JetGitDeliveryOutcome
    ) -> JetGitDelivery {
        nextID += 1
        return JetGitDelivery(
            id: FakeGitDeliveries.id(nextID),
            conversationID: key.conversationID,
            checkpoint: checkpoint,
            operation: operation,
            policy: DeliveryCoordinatorTests.policy,
            utilityJobID: nil,
            message: nil,
            acknowledgedBy: nil,
            outcome: outcome
        )
    }

    /// Time-ordered like UUIDv7: a larger `index` sorts later.
    static func id(_ index: Int) -> UUID {
        UUID(uuidString: String(format: "%08X-0000-7000-8000-%012X", 0x0190_0000 + index, index))!
    }
}

extension JetGitDelivery {
    func with(outcome: JetGitDeliveryOutcome, acknowledgedBy: UUID? = nil) -> JetGitDelivery {
        JetGitDelivery(
            id: id,
            conversationID: conversationID,
            checkpoint: checkpoint,
            operation: operation,
            policy: policy,
            utilityJobID: utilityJobID,
            message: message,
            acknowledgedBy: acknowledgedBy ?? self.acknowledgedBy,
            outcome: outcome
        )
    }
}

@MainActor
struct DeliveryCoordinatorTests {
    typealias Step = DeliveryCoordinator.Step
    typealias Key = DeliveryCoordinator.Key

    static let plane = UUID(uuidString: "11111111-0000-4000-8000-000000000001")!
    static let taskA = UUID(uuidString: "22222222-0000-4000-8000-000000000001")!
    static let taskB = UUID(uuidString: "22222222-0000-4000-8000-000000000002")!
    static let keyA = Key(planeRegistryID: plane, conversationID: taskA)
    static let keyB = Key(planeRegistryID: plane, conversationID: taskB)
    static let checkpoint = JetGitCheckpoint(runID: UUID(uuidString: "33333333-0000-4000-8000-000000000001")!, turn: 2)
    static let policy = JetGitDeliveryPolicy(
        automatic: false,
        branch: true,
        commit: true,
        push: true,
        draftPullRequest: true,
        branchPrefix: "jet/"
    )
    static let unknownAdmission = JetClientFailure.commandOutcomeUnknown(commandID: UUID())

    static func plan(_ steps: [Step] = [.branch, .commit, .push, .draftPullRequest]) -> DeliveryCoordinator.Plan {
        DeliveryCoordinator.Plan(
            steps: steps,
            branchName: "jet/fix-login",
            remote: "origin",
            baseBranch: "main",
            checkpoint: checkpoint
        )
    }

    static func refusal(_ code: String) -> JetClientFailure {
        .presentation(JetPresentationError(category: .conflict, code: code, message: "refused", retryable: false))
    }

    // MARK: - Running a plan

    @Test
    func runsStepsInOrderWithExactBodiesAndDistinctCommandIDs() async {
        let git = FakeGitDeliveries()
        git.admissions = [
            .accept(FakeGitDeliveries.completed, pendingChecks: 2),
            .accept(FakeGitDeliveries.completed),
            .accept(FakeGitDeliveries.completed, pendingChecks: 1),
            .accept(FakeGitDeliveries.completed),
        ]
        let coordinator = git.makeCoordinator()
        await coordinator.start(Self.plan(), for: Self.keyA)?.value

        #expect(git.submissions.map(\.request) == [
            JetGitDeliveryRequest(conversationID: Self.taskA, checkpoint: nil, operation: .branch(name: "jet/fix-login")),
            JetGitDeliveryRequest(conversationID: Self.taskA, checkpoint: Self.checkpoint, operation: .commit),
            JetGitDeliveryRequest(conversationID: Self.taskA, checkpoint: nil, operation: .push(remote: "origin")),
            JetGitDeliveryRequest(
                conversationID: Self.taskA,
                checkpoint: Self.checkpoint,
                operation: .draftPullRequest(remote: "origin", base: "main")
            ),
        ])
        #expect(Set(git.submissions.map(\.commandID)).count == 4)
        // Each step starts only after the previous one completed.
        #expect(git.events == [
            "submit branch", "fetch", "fetch", "fetch",
            "submit commit", "fetch",
            "submit push", "fetch", "fetch",
            "submit draftPullRequest", "fetch",
        ])
        let progress = coordinator.progress(for: Self.keyA)
        #expect(progress?.isRunning == false)
        #expect(progress?.isFinished == true)
        // Every fetch feeds the session (Needs You, the open task's deliveries).
        #expect(git.notified.count == git.fetchCount)
        #expect(git.sleeps == [.seconds(2), .seconds(2), .seconds(2)])
    }

    @Test(arguments: [
        JetGitDeliveryOutcome.failed(code: "git.policy_changed"),
        JetGitDeliveryOutcome.outcomeUnknown,
    ])
    func stopsAtTheFirstStepThatDidNotComplete(_ outcome: JetGitDeliveryOutcome) async throws {
        let git = FakeGitDeliveries()
        git.admissions = [.accept(FakeGitDeliveries.completed), .accept(outcome, pendingChecks: 1)]
        let coordinator = git.makeCoordinator()
        await coordinator.start(Self.plan([.branch, .commit, .push]), for: Self.keyA)?.value

        #expect(git.submissions.count == 2)
        let progress = try #require(coordinator.progress(for: Self.keyA))
        let commit = try #require(coordinator.history(for: Self.keyA).first)
        #expect(commit.outcome == outcome)
        switch outcome {
        case let .failed(code): #expect(progress.state(of: .commit) == .failed(code: code, delivery: commit))
        default: #expect(progress.state(of: .commit) == .unconfirmed(commit))
        }
        #expect(progress.state(of: .push) == .notStarted)
        #expect(progress.stoppedStep == .commit)
        #expect(progress.notStartedSteps == [.push])
        #expect(!progress.isRunning)
    }

    @Test(arguments: [0, 1])
    func uncertainAdmissionResendsTheSameCommandThenContinues(_ variant: Int) async throws {
        let git = FakeGitDeliveries()
        let error: Error = variant == 0 ? Self.unknownAdmission : CancellationError()
        git.admissions = [.accept(FakeGitDeliveries.completed), .refuse(error)]
        let coordinator = git.makeCoordinator()
        await coordinator.start(Self.plan([.branch, .commit, .push]), for: Self.keyA)?.value

        #expect(coordinator.progress(for: Self.keyA)?.state(of: .commit) == .admissionUncertain)
        #expect(git.submissions.count == 2)
        #expect(coordinator.retryFailedStep(for: Self.keyA) == nil)
        // A new plan would drop the uncertain Command's identity.
        #expect(coordinator.hasUncertainStep(for: Self.keyA))
        #expect(coordinator.start(Self.plan([.commit]), for: Self.keyA) == nil)

        await coordinator.resendUncertain(for: Self.keyA)?.value

        #expect(git.submissions.count == 4)
        #expect(git.submissions[2].commandID == git.submissions[1].commandID)
        #expect(git.submissions[2].request == git.submissions[1].request)
        #expect(git.submissions[3].request.operation == .push(remote: "origin"))
        #expect(git.submissions[3].commandID != git.submissions[2].commandID)
        #expect(coordinator.progress(for: Self.keyA)?.isFinished == true)
    }

    @Test
    func aRefusalFailsTheStepAfterOneSubmit() async throws {
        let git = FakeGitDeliveries()
        git.admissions = [.accept(FakeGitDeliveries.completed), .refuse(Self.refusal("git.run_active"))]
        let coordinator = git.makeCoordinator()
        await coordinator.start(Self.plan([.branch, .commit, .push]), for: Self.keyA)?.value

        #expect(git.submissions.count == 2)
        let progress = try #require(coordinator.progress(for: Self.keyA))
        #expect(progress.state(of: .commit) == .failed(code: "git.run_active", delivery: nil))
        #expect(progress.state(of: .push) == .notStarted)
        #expect(coordinator.resendUncertain(for: Self.keyA) == nil)
    }

    @Test
    func unexpectedErrorsFailWithAGenericCode() {
        struct Broken: Error {}
        #expect(DeliveryCoordinator.admissionState(for: Broken()) == .failed(code: "request.failed", delivery: nil))
        #expect(DeliveryCoordinator.admissionState(for: JetClientFailure.presentation(.cancelled)) == .admissionUncertain)
        #expect(DeliveryCoordinator.admissionState(for: JetClientFailure.presentation(.offline))
            == .failed(code: "transport.offline", delivery: nil))
    }

    @Test
    func tryAgainSendsOnlyTheFailedStepWithANewCommandID() async throws {
        let git = FakeGitDeliveries()
        git.admissions = [
            .accept(FakeGitDeliveries.completed),
            .accept(.failed(code: "git.index_locked")),
            .accept(FakeGitDeliveries.completed),
        ]
        let coordinator = git.makeCoordinator()
        await coordinator.start(Self.plan([.branch, .commit, .push]), for: Self.keyA)?.value
        #expect(coordinator.resendUncertain(for: Self.keyA) == nil)

        await coordinator.retryFailedStep(for: Self.keyA)?.value

        #expect(git.submissions.count == 3)
        #expect(git.submissions[2].request == git.submissions[1].request)
        #expect(git.submissions[2].commandID != git.submissions[1].commandID)
        let progress = try #require(coordinator.progress(for: Self.keyA))
        #expect(progress.state(of: .commit).isCompleted)
        // Later steps wait for Continue… (lead decision L4).
        #expect(progress.notStartedSteps == [.push])
        #expect(!progress.isRunning)
    }

    @Test
    func retryingAHistoryDeliverySendsItsExactBodyWithANewCommandID() async throws {
        let git = FakeGitDeliveries()
        let failed = git.record(
            Self.keyA,
            .draftPullRequest(remote: "upstream", base: nil),
            checkpoint: Self.checkpoint,
            outcome: .failed(code: "git.github_timeout")
        )
        let coordinator = git.makeCoordinator()
        await coordinator.refresh(for: Self.keyA)

        await coordinator.retry(failed, for: Self.keyA)?.value

        #expect(git.submissions.map(\.request) == [
            JetGitDeliveryRequest(
                conversationID: Self.taskA,
                checkpoint: Self.checkpoint,
                operation: .draftPullRequest(remote: "upstream", base: nil)
            ),
        ])
        #expect(coordinator.progress(for: Self.keyA)?.isFinished == true)
    }

    @Test
    func refreshNeverSubmitsAndAdoptsARecordedUncertainStep() async throws {
        let git = FakeGitDeliveries()
        // A commit recorded before the plan started must not be adopted.
        git.record(Self.keyA, .commit, checkpoint: Self.checkpoint, outcome: FakeGitDeliveries.completed)
        let coordinator = git.makeCoordinator()
        await coordinator.refresh(for: Self.keyA)
        git.admissions = [.accept(FakeGitDeliveries.completed), .refuse(Self.unknownAdmission)]
        await coordinator.start(Self.plan([.branch, .commit, .push]), for: Self.keyA)?.value

        await coordinator.refresh(for: Self.keyA)
        #expect(coordinator.progress(for: Self.keyA)?.state(of: .commit) == .admissionUncertain)

        // jetd did record the uncertain commit.
        let recorded = git.record(Self.keyA, .commit, checkpoint: Self.checkpoint, outcome: FakeGitDeliveries.completed)
        await coordinator.refresh(for: Self.keyA)

        #expect(git.submissions.count == 2)
        let progress = try #require(coordinator.progress(for: Self.keyA))
        #expect(progress.state(of: .commit) == .completed(recorded))
        #expect(progress.deliveryIDs[.commit] == recorded.id)
        // Refresh never resumes the plan.
        #expect(progress.state(of: .push) == .notStarted)
        #expect(!progress.isRunning)
        #expect(coordinator.resendUncertain(for: Self.keyA) == nil)
    }

    @Test
    func refreshesOfOneTaskShareOneQuery() async {
        let git = FakeGitDeliveries()
        git.yieldsInFetch = true
        let coordinator = git.makeCoordinator()
        async let first = coordinator.refresh(for: Self.keyA)
        async let second = coordinator.refresh(for: Self.keyA)
        _ = await (first, second)
        #expect(git.fetchCount == 1)
    }

    @Test
    func tasksRunIndependently() async throws {
        let git = FakeGitDeliveries()
        git.admissionsByConversation = [
            Self.taskA: [.accept(FakeGitDeliveries.completed, pendingChecks: 1), .accept(FakeGitDeliveries.completed)],
            Self.taskB: [.accept(.failed(code: "git.policy_changed"), pendingChecks: 1)],
        ]
        let coordinator = git.makeCoordinator()
        let first = coordinator.start(Self.plan([.branch, .commit]), for: Self.keyA)
        let second = coordinator.start(Self.plan([.branch, .commit]), for: Self.keyB)
        #expect(first != nil)
        #expect(second != nil)
        await first?.value
        await second?.value

        let a = try #require(coordinator.progress(for: Self.keyA))
        let b = try #require(coordinator.progress(for: Self.keyB))
        #expect(a.isFinished)
        #expect(b.stoppedStep == .branch)
        #expect(b.state(of: .commit) == .notStarted)
        #expect(coordinator.history(for: Self.keyA).count == 2)
        #expect(coordinator.history(for: Self.keyA).allSatisfy { $0.conversationID == Self.taskA })
        #expect(coordinator.history(for: Self.keyB).count == 1)
        #expect(coordinator.history(for: Self.keyB).allSatisfy { $0.conversationID == Self.taskB })
        #expect(Set(git.submissions.map(\.commandID)).count == 3)
    }

    @Test
    func startIsIgnoredWhileThePlanRuns() async {
        let git = FakeGitDeliveries()
        let coordinator = git.makeCoordinator()
        let first = coordinator.start(Self.plan(), for: Self.keyA)
        let second = coordinator.start(Self.plan(), for: Self.keyA)
        #expect(first != nil)
        #expect(second == nil)
        #expect(coordinator.start(Self.plan([]), for: Self.keyB) == nil)
        await first?.value
        #expect(git.submissions.count == 4)
    }

    @Test
    func failedChecksBackOffUpToTenSecondsAndNeverResubmit() async {
        let git = FakeGitDeliveries()
        git.admissions = [.accept(FakeGitDeliveries.completed, pendingChecks: 1)]
        git.fetchErrors = Array(repeating: JetClientFailure.presentation(.offline), count: 4)
        let coordinator = git.makeCoordinator()
        await coordinator.start(Self.plan([.push]), for: Self.keyA)?.value

        #expect(git.sleeps == [.seconds(4), .seconds(8), .seconds(10), .seconds(10), .seconds(2)])
        #expect(git.failedChecksAtSleep == [true, true, true, true, false])
        #expect(git.submissions.count == 1)
        #expect(coordinator.progress(for: Self.keyA)?.lastCheckFailed == false)
        #expect(coordinator.progress(for: Self.keyA)?.isFinished == true)
    }

    // MARK: - Mark as Checked

    @Test
    func acknowledgementKeepsOneCommandIDUntilJetdConfirmsIt() async throws {
        let git = FakeGitDeliveries()
        let unknown = git.record(Self.keyA, .push(remote: "origin"), outcome: .outcomeUnknown)
        let coordinator = git.makeCoordinator()
        await coordinator.refresh(for: Self.keyA)
        var sent: [UUID] = []

        let first = await coordinator.acknowledge(unknown, for: Self.keyA) { _, commandID in
            sent.append(commandID)
            throw Self.unknownAdmission
        }
        #expect(!first)
        #expect(coordinator.acknowledgement(for: unknown.id) == .failed)
        #expect(coordinator.acknowledgementCommandID(for: unknown.id) == sent.first)

        let second = await coordinator.acknowledge(unknown, for: Self.keyA) { deliveryID, commandID in
            sent.append(commandID)
            git.acknowledge(deliveryID)
        }
        #expect(second)
        #expect(sent.count == 2)
        #expect(sent[0] == sent[1])
        #expect(coordinator.acknowledgementCommandID(for: unknown.id) == nil)
        #expect(coordinator.history(for: Self.keyA).first?.acknowledgedBy != nil)
        #expect(coordinator.acknowledgement(for: unknown.id) == nil)
        #expect(git.notified.last?.deliveries.contains(where: \.needsAcknowledgement) == false)
    }

    // MARK: - Relaunch

    @Test
    func restoredTasksAreOnlyCheckedNeverSubmitted() async throws {
        let store = DeliveryChainStore.inMemory([Self.keyA])
        let git = FakeGitDeliveries()
        git.record(Self.keyA, .push(remote: "origin"), outcome: .outcomeUnknown)
        let coordinator = git.makeCoordinator(store: store)

        #expect(coordinator.restoredKeys == [Self.keyA])
        #expect(coordinator.progress(for: Self.keyA) == nil)

        await coordinator.refreshRestoredChains { _ in false }
        #expect(git.fetchCount == 0)

        await coordinator.refreshRestoredChains { $0 == Self.plane }
        #expect(git.submissions.isEmpty)
        #expect(coordinator.progress(for: Self.keyA) == nil)
        #expect(git.notified.last?.deliveries.first?.needsAcknowledgement == true)
        #expect(coordinator.restoredKeys.isEmpty)
        // Still unconfirmed, so still remembered.
        #expect(store.load() == [Self.keyA])
    }

    @Test
    func theStoreHoldsOnlyIDsAndForgetsFinishedPlans() async throws {
        let suite = "jet.tests.keep-changes"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defaults.removePersistentDomain(forName: suite)
        defer { defaults.removePersistentDomain(forName: suite) }
        let git = FakeGitDeliveries()
        git.admissions = [.accept(FakeGitDeliveries.completed), .refuse(Self.unknownAdmission)]
        let coordinator = git.makeCoordinator(store: .userDefaults(defaults))

        await coordinator.start(Self.plan([.branch, .commit, .push]), for: Self.keyA)?.value

        let data = try #require(defaults.data(forKey: DeliveryChainStore.defaultsKey))
        let text = String(decoding: data, as: UTF8.self)
        #expect(!text.contains("jet/fix-login"))
        #expect(!text.contains("origin"))
        #expect(!text.contains(Self.checkpoint.runID.uuidString))
        #expect(try JSONDecoder().decode([Key].self, from: data) == [Self.keyA])

        // Resending the uncertain request finishes the plan, so the task is forgotten.
        git.admissions = []
        await coordinator.resendUncertain(for: Self.keyA)?.value
        #expect(coordinator.progress(for: Self.keyA)?.isFinished == true)
        #expect(defaults.data(forKey: DeliveryChainStore.defaultsKey) == nil)
    }

    @Test
    func theStoreRemembersAtMost64Tasks() async {
        let store = DeliveryChainStore.inMemory()
        let git = FakeGitDeliveries()
        let coordinator = git.makeCoordinator(store: store)
        let keys = (0 ..< 70).map { Key(planeRegistryID: Self.plane, conversationID: FakeGitDeliveries.id(1_000 + $0)) }
        for key in keys {
            git.admissions = [.refuse(Self.unknownAdmission)]
            await coordinator.start(Self.plan([.commit]), for: key)?.value
        }
        #expect(store.load() == Array(keys.suffix(64)))
    }

    // MARK: - Copy

    @Test(arguments: [
        ("git.branch_exists", Step.branch, "A branch with this name already exists.", .chooseAnotherName),
        ("git.invalid_name", Step.branch, "Git doesn't accept this branch name.", .chooseAnotherName),
        ("git.invalid_name", Step.push, "Git couldn't use the remote origin.", .keepChanges),
        ("git.run_active", Step.commit, "Claude Code was still working, so Git didn't run.", .tryAgain),
        ("git.delivery_unresolved", Step.commit, "An earlier Git step couldn't be confirmed.", .markAsChecked),
        ("git.head_changed", Step.commit, "The working copy changed after you reviewed it.", .keepChanges),
        ("git.index_locked", Step.commit, "Another Git operation is running in the working copy.", .tryAgain),
        ("git.policy_changed", Step.push, "The project's Keep settings changed while this step waited.", .tryAgain),
        ("git.checkpoint_mismatch", Step.commit, "These changes are no longer available to commit.", .keepChanges),
        ("git.remote_invalid", Step.push, "Git couldn't use the remote origin.", .keepChanges),
        ("git.branch_required", Step.push, "There's no branch to push yet.", .keepChanges),
        ("git.push_required", Step.draftPullRequest, "Push the branch before opening a pull request.", .keepChanges),
        ("git.github_required", Step.draftPullRequest, "Pull requests work only with GitHub remotes.", nil),
        ("git.github_credential_unavailable", Step.draftPullRequest, "Jet needs a GitHub token saved on Studio Mac.", .howToSetUp),
        ("git.github_refused", Step.draftPullRequest, "GitHub refused the request. Check that your token can write pull requests.", .howToSetUp),
        ("git.github_timeout", Step.draftPullRequest, "Jet couldn't get a usable answer from GitHub.", .tryAgain),
        ("git.draft_conflict", Step.draftPullRequest, "The draft pull request was closed, marked ready, or changed on GitHub.", nil),
        ("git.timeout", Step.push, "Git took too long and was stopped.", .tryAgain),
        ("git.incomplete_or_dirty_baseline", Step.commit, "The project had uncommitted changes when this task started, so Jet can't commit safely.", nil),
        ("transport.offline", Step.push, "Not connected to Studio Mac.", .tryAgain),
        ("git.something_new", Step.push, "Git refused this step.", .tryAgain),
        ("request.failed", Step.branch, "Git refused this step.", .tryAgain),
    ] as [(String, Step, String, GitFailureCopy.Fix?)])
    func failureCodesReadAsPlainSentencesWithTheirFix(
        code: String,
        step: Step,
        sentence: String,
        fix: GitFailureCopy.Fix?
    ) {
        let copy = GitFailureCopy.make(code: code, step: step, computer: "Studio Mac", assistant: "Claude Code")
        #expect(copy == GitFailureCopy(sentence: sentence, fix: fix))
    }

    @Test
    func stepCopyNamesTheBranchAndRemote() {
        #expect(GitStepCopy.title(.branch, branch: "jet/x", remote: "origin") == "Create branch jet/x")
        #expect(GitStepCopy.title(.commit, branch: "jet/x", remote: "origin") == "Commit (Jet writes the message)")
        #expect(GitStepCopy.title(.push, branch: "jet/x", remote: "upstream") == "Push to upstream")
        #expect(GitStepCopy.title(.draftPullRequest, branch: nil, remote: "origin", updatesPullRequest: true)
            == "Update the draft pull request")
        #expect(GitStepCopy.running(.branch, branch: "jet/x", remote: "origin") == "Creating branch jet/x…")
        #expect(GitStepCopy.done(.commit, branch: nil, remote: "origin", reply: 3) == "Committed the changes from reply 3")
        #expect(GitStepCopy.done(.push, branch: "jet/x", remote: "origin", reply: nil) == "Pushed jet/x to origin")
        #expect(GitStepCopy.failed(.push, remote: "origin") == "Couldn't push to origin.")
        #expect(GitStepCopy.couldNotConfirm(.push) == "Jet couldn't confirm whether the push happened.")
        #expect(GitStepCopy.followUp(.push) == "Check the branch on GitHub, then mark it as checked.")
        #expect(GitStepCopy.followUp(.commit) == "Check the branch in your project, then mark it as checked.")
    }

    // MARK: - Branch names

    @Test
    func branchSlugsAreShortLowercaseASCII() {
        let id = UUID(uuidString: "ABCDEF12-0000-4000-8000-000000000001")!
        #expect(KeepChangesNaming.slug("Fix login redirect loop", conversationID: id) == "fix-login-redirect-loop")
        #expect(KeepChangesNaming.slug("Исправить вход", conversationID: id) == "ispravit-vhod")
        #expect(KeepChangesNaming.slug("Ça marche déjà!", conversationID: id) == "ca-marche-deja")
        #expect(KeepChangesNaming.slug("", conversationID: id) == "task-abcdef12")
        #expect(KeepChangesNaming.slug("!!! ???", conversationID: id) == "task-abcdef12")

        let long = "Add a much longer title that keeps going well past the forty eight character limit"
        let full = "add-a-much-longer-title-that-keeps-going-well-past-the-forty-eight-character-limit"
        let slug = KeepChangesNaming.slug(long, conversationID: id)
        #expect(slug.count <= 48)
        #expect(full.hasPrefix(slug))
        #expect(!slug.hasSuffix("-"))
        #expect(full.dropFirst(slug.count).first == "-")

        #expect(KeepChangesNaming.defaultBranch(prefix: "jet/", title: "Fix login", conversationID: id) == "jet/fix-login")
        #expect(KeepChangesNaming.nextAvailable("jet/fix", taken: ["jet/fix", "jet/fix-2"]) == "jet/fix-3")
        #expect(KeepChangesNaming.nextAvailable("jet/fix", taken: []) == "jet/fix")
    }

    @Test(arguments: [
        ("", "Enter a branch name."),
        ("fix login", "Branch names can't contain spaces."),
        ("fix~1", "Branch names can't contain ~ ^ : ? * [ or \\."),
        ("a^b", "Branch names can't contain ~ ^ : ? * [ or \\."),
        ("a:b", "Branch names can't contain ~ ^ : ? * [ or \\."),
        ("a?b", "Branch names can't contain ~ ^ : ? * [ or \\."),
        ("a*b", "Branch names can't contain ~ ^ : ? * [ or \\."),
        ("a[b", "Branch names can't contain ~ ^ : ? * [ or \\."),
        ("a\\b", "Branch names can't contain ~ ^ : ? * [ or \\."),
        ("a..b", "This isn't a valid branch name."),
        ("a@{b", "This isn't a valid branch name."),
        ("a//b", "This isn't a valid branch name."),
        ("-a", "This isn't a valid branch name."),
        ("/a", "This isn't a valid branch name."),
        (".a", "This isn't a valid branch name."),
        ("a/", "This isn't a valid branch name."),
        ("a.", "This isn't a valid branch name."),
        ("a.lock", "This isn't a valid branch name."),
        ("a@b", "This isn't a valid branch name."),
        ("jet/ünïcode", "This isn't a valid branch name."),
        ("a\u{7}b", "This isn't a valid branch name."),
        (String(repeating: "a", count: 241), "This isn't a valid branch name."),
        ("jet/fix-login", nil),
        ("feature/ABC_1.2", nil),
    ] as [(String, String?)])
    func branchNamesAreValidatedAsYouType(name: String, issue: String?) {
        #expect(KeepChangesNaming.branchIssue(name) == issue)
    }

    @Test
    func remoteAndBaseNamesAreValidated() {
        #expect(KeepChangesNaming.remoteIssue("") == "Enter a remote name without spaces.")
        #expect(KeepChangesNaming.remoteIssue("my remote") == "Enter a remote name without spaces.")
        #expect(KeepChangesNaming.remoteIssue("or@gin") == "This isn't a valid remote name.")
        #expect(KeepChangesNaming.remoteIssue("origin") == nil)
        #expect(KeepChangesNaming.baseIssue("") == nil)
        #expect(KeepChangesNaming.baseIssue("main") == nil)
        #expect(KeepChangesNaming.baseIssue("ma in") == "Branch names can't contain spaces.")
    }

    // MARK: - Planner and sheet model

    static func delivery(
        _ index: Int,
        _ operation: JetGitOperation,
        _ outcome: JetGitDeliveryOutcome,
        checkpoint: JetGitCheckpoint? = nil,
        conversationID: UUID = taskA,
        acknowledged: Bool = false
    ) -> JetGitDelivery {
        JetGitDelivery(
            id: FakeGitDeliveries.id(index),
            conversationID: conversationID,
            checkpoint: checkpoint,
            operation: operation,
            policy: policy,
            utilityJobID: nil,
            message: nil,
            acknowledgedBy: acknowledged ? UUID() : nil,
            outcome: outcome
        )
    }

    @Test
    func thePlannerSkipsStepsThatAlreadyHappened() {
        let steps = { (choice: KeepChangesChoice, history: [JetGitDelivery]) in
            KeepChangesPlanner.steps(choice: choice, mode: .plan, checkpoint: Self.checkpoint, history: history)
        }
        #expect(steps(.saveToBranch, []) == [.branch, .commit])
        #expect(steps(.saveAndPush, []) == [.branch, .commit, .push])
        #expect(steps(.draftPullRequest, []) == [.branch, .commit, .push, .draftPullRequest])

        let branch = Self.delivery(1, .branch(name: "jet/x"), FakeGitDeliveries.completed)
        #expect(steps(.saveAndPush, [branch]) == [.commit, .push])

        let commit = Self.delivery(2, .commit, FakeGitDeliveries.completed, checkpoint: Self.checkpoint)
        #expect(steps(.saveToBranch, [commit, branch]) == [])
        #expect(steps(.saveAndPush, [commit, branch]) == [.push])

        let push = Self.delivery(3, .push(remote: "origin"), FakeGitDeliveries.completed)
        #expect(steps(.saveAndPush, [push, commit, branch]) == [])
        #expect(steps(.draftPullRequest, [push, commit, branch]) == [.draftPullRequest])

        // A push older than the commit doesn't count.
        let olderPush = Self.delivery(1, .push(remote: "origin"), FakeGitDeliveries.completed)
        let newerCommit = Self.delivery(4, .commit, FakeGitDeliveries.completed, checkpoint: Self.checkpoint)
        #expect(steps(.saveAndPush, [newerCommit, olderPush, branch]) == [.push])

        // A failed branch step doesn't count as a branch.
        let failedBranch = Self.delivery(5, .branch(name: "jet/x"), .failed(code: "git.branch_exists"))
        #expect(steps(.saveToBranch, [failedBranch]) == [.branch, .commit])

        #expect(KeepChangesPlanner.steps(choice: .draftPullRequest, mode: .single(.push), checkpoint: nil, history: [])
            == [.push])
    }

    static func facts(
        fileCount: Int = 3,
        history: [JetGitDelivery] = [],
        isLocalCheckout: Bool = false,
        isRemote: Bool = false
    ) -> KeepChangesFacts {
        KeepChangesFacts(
            conversationID: taskA,
            title: "Fix login redirect",
            projectName: "web-app",
            assistantName: "Claude Code",
            computerName: isRemote ? "Studio Mac" : "This Mac",
            isRemote: isRemote,
            isLocalCheckout: isLocalCheckout,
            fileCount: fileCount,
            checkpoint: fileCount > 0 ? checkpoint : nil,
            branchPrefix: "jet/",
            history: history
        )
    }

    @Test
    func theSheetModelNamesThePrimaryAfterThePlan() throws {
        let model = KeepChangesModel(mode: .plan)
        model.apply(Self.facts())
        #expect(model.branchName == "jet/fix-login-redirect")
        #expect(model.sheetTitle == "Keep Changes")
        #expect(model.primaryTitle == "Save to Branch")
        model.choice = .saveAndPush
        #expect(model.primaryTitle == "Save and Push")
        model.choice = .draftPullRequest
        #expect(model.primaryTitle == "Create Pull Request")
        #expect(model.steps == [.branch, .commit, .push, .draftPullRequest])
        #expect(model.canConfirm)

        let plan = try #require(model.makePlan())
        #expect(plan == DeliveryCoordinator.Plan(
            steps: [.branch, .commit, .push, .draftPullRequest],
            branchName: "jet/fix-login-redirect",
            remote: "origin",
            baseBranch: nil,
            checkpoint: Self.checkpoint
        ))

        model.branchName = "fix login"
        #expect(model.branchIssue == "Branch names can't contain spaces.")
        #expect(!model.canConfirm)
        #expect(model.makePlan() == nil)

        let opened = Self.delivery(1, .draftPullRequest(remote: "origin", base: nil), FakeGitDeliveries.completed, checkpoint: Self.checkpoint)
        let updating = KeepChangesModel(mode: .plan)
        updating.apply(Self.facts(history: [opened]), lastPlan: Self.plan([.branch, .commit, .push, .draftPullRequest]))
        #expect(updating.choice == .draftPullRequest)
        #expect(updating.primaryTitle == "Update Pull Request")
        #expect(updating.baseBranch == "main")
    }

    @Test
    func theSheetModelUsesTheBranchCreatedEarlierAndSuggestsAFreeName() throws {
        let created = Self.delivery(1, .branch(name: "jet/earlier"), .completed(head: "abc", branch: "jet/earlier", pullRequest: nil))
        let model = KeepChangesModel(mode: .plan)
        model.apply(Self.facts(history: [created]))
        model.choice = .saveAndPush
        #expect(model.existingBranch == "jet/earlier")
        #expect(model.steps == [.commit, .push])
        #expect(try #require(model.makePlan()).branchName == "jet/earlier")

        let refused = Self.delivery(2, .branch(name: "jet/fix-login-redirect"), .failed(code: "git.branch_exists"))
        let retry = KeepChangesModel(mode: .plan)
        retry.apply(Self.facts(history: [refused]))
        #expect(retry.branchName == "jet/fix-login-redirect-2")
    }

    @Test
    func theSheetModelExplainsWhyItCannotConfirm() {
        let empty = KeepChangesModel(mode: .plan)
        empty.apply(Self.facts(fileCount: 0))
        #expect(empty.emptyState == .noChanges)
        #expect(!empty.canConfirm)

        let commit = Self.delivery(1, .commit, FakeGitDeliveries.completed, checkpoint: Self.checkpoint)
        let branch = Self.delivery(0, .branch(name: "jet/x"), FakeGitDeliveries.completed)
        let kept = KeepChangesModel(mode: .plan)
        kept.apply(Self.facts(history: [commit, branch]))
        #expect(kept.emptyState == .alreadyKept)
        #expect(!kept.canConfirm)

        let push = KeepChangesModel(mode: .single(.push))
        push.apply(Self.facts(fileCount: 0))
        #expect(push.sheetTitle == "Push Branch")
        #expect(push.primaryTitle == "Push")
        #expect(push.emptyState == nil)
        #expect(push.singleStepWarning == "This task has no branch yet. Create one first.")
        #expect(push.canConfirm)
        push.remote = "my remote"
        #expect(push.remoteIssue == "Enter a remote name without spaces.")
        #expect(!push.canConfirm)

        let draft = KeepChangesModel(mode: .single(.draftPullRequest))
        draft.apply(Self.facts())
        #expect(draft.singleStepWarning == "Push the branch first.")
        #expect(draft.primaryTitle == "Open Pull Request")
    }

    @Test
    func theSubtitleCountsFilesAndNamesTheFolder() {
        let text = { (facts: KeepChangesFacts) in
            KeepChangesSheetCopy.subtitle(facts).map { String($0.characters) }
        }
        #expect(text(Self.facts()) == "Claude Code changed 3 files in a separate working copy. Your web-app folder hasn't changed.")
        #expect(text(Self.facts(fileCount: 1)) == "Claude Code changed 1 file in a separate working copy. Your web-app folder hasn't changed.")
        #expect(text(Self.facts(isRemote: true))
            == "Claude Code changed 3 files in a separate working copy. Your web-app folder on Studio Mac hasn't changed.")
        #expect(text(Self.facts(isLocalCheckout: true)) == "Claude Code changed 3 files in your web-app folder.")
        #expect(text(Self.facts(fileCount: 0)) == nil)
    }

    // MARK: - Status line

    static let context = KeepChangesSummary.Context(computer: "This Mac", assistant: "Claude Code", isLocalCheckout: false)

    static func progress(
        _ steps: [Step],
        _ states: [Step: DeliveryCoordinator.StepState],
        running: Bool = false,
        lastCheckFailed: Bool = false
    ) -> DeliveryCoordinator.Progress {
        DeliveryCoordinator.Progress(
            plan: plan(steps),
            states: states,
            isRunning: running,
            lastCheckFailed: lastCheckFailed
        )
    }

    @Test
    func theStatusLineShowsProgressAndOutcomes() throws {
        let make = { (progress: DeliveryCoordinator.Progress?, history: [JetGitDelivery], context: KeepChangesSummary.Context) in
            KeepChangesSummary.make(progress: progress, history: history, context: context)
        }
        #expect(make(nil, [], Self.context) == nil)

        let running = try #require(make(Self.progress([.branch, .commit, .push], [.branch: .completed(nil), .commit: .pending], running: true), [], Self.context))
        #expect(running.tone == .running)
        #expect(running.systemImage == nil)
        #expect(running.headline == "Committing the changes…")
        #expect(running.details == ["Step 2 of 3"])
        #expect(running.actions.isEmpty)

        let unreachable = try #require(make(Self.progress([.push], [.push: .pending], running: true, lastCheckFailed: true), [], Self.context))
        #expect(unreachable.details == ["Can't reach This Mac. Jet keeps checking."])

        let saved = try #require(make(Self.progress([.branch, .commit], [.branch: .completed(nil), .commit: .completed(nil)]), [], Self.context))
        #expect(saved.tone == .success)
        #expect(saved.headline == "Saved to branch jet/fix-login")
        #expect(saved.details.isEmpty)
        #expect(saved.switchCommand == "git switch jet/fix-login")
        #expect(saved.actions == [.copyBranchName("jet/fix-login")])

        var local = Self.context
        local.isLocalCheckout = true
        let inFolder = try #require(make(Self.progress([.branch, .commit], [.branch: .completed(nil), .commit: .completed(nil)]), [], local))
        #expect(inFolder.switchCommand == nil)

        let branchOnly = try #require(make(Self.progress([.branch], [.branch: .completed(nil)]), [], Self.context))
        #expect(branchOnly.headline == "Created branch jet/fix-login")

        let pushed = try #require(make(Self.progress([.commit, .push], [.commit: .completed(nil), .push: .completed(nil)]), [], Self.context))
        #expect(pushed.details == ["Pushed to origin"])

        let url = "https://github.com/alex/web-app/pull/42"
        let draft = Self.delivery(9, .draftPullRequest(remote: "origin", base: nil), .completed(head: "abc", branch: "jet/fix-login", pullRequest: url))
        let opened = try #require(make(Self.progress([.push, .draftPullRequest], [.push: .completed(nil), .draftPullRequest: .completed(draft)]), [], Self.context))
        #expect(opened.details == ["Pushed to origin", "Draft pull request opened"])
        #expect(opened.actions == [.openPullRequest(URL(string: url)!), .copyBranchName("jet/fix-login")])
        #expect(opened.switchCommand == nil)

        for link in [
            "http://github.com/alex/web-app/pull/42",
            "https://github.com.evil.com/alex/web-app/pull/42",
            "https://user@github.com/alex/web-app/pull/42",
            "https://github.com/alex/web-app/issues/42",
            "javascript:alert(1)",
        ] {
            #expect(GitHubPullRequestLink.url(link) == nil)
            let unsafe = Self.delivery(9, .draftPullRequest(remote: "origin", base: nil), .completed(head: "abc", branch: "jet/fix-login", pullRequest: link))
            let summary = try #require(make(nil, [unsafe], Self.context))
            #expect(summary.pullRequestURL == nil)
            #expect(summary.actions == [.copyBranchName("jet/fix-login")])
        }
        #expect(GitHubPullRequestLink.url("https://GitHub.com/alex/web-app/pull/42") != nil)

        let exists = try #require(make(Self.progress([.branch, .commit], [.branch: .failed(code: "git.branch_exists", delivery: nil)]), [], Self.context))
        #expect(exists.tone == .failure)
        #expect(exists.systemImage == "xmark.octagon.fill")
        #expect(exists.headline == "Couldn't create the branch.")
        #expect(exists.details == ["A branch with this name already exists."])
        #expect(exists.actions == [.chooseAnotherName])

        let locked = try #require(make(Self.progress([.commit], [.commit: .failed(code: "git.index_locked", delivery: nil)]), [], Self.context))
        #expect(locked.actions == [.tryAgainStep(.commit)])

        let credential = Self.delivery(9, .draftPullRequest(remote: "origin", base: nil), .failed(code: "git.github_credential_unavailable"))
        let token = try #require(make(nil, [credential], Self.context))
        #expect(token.headline == "Couldn't open the draft pull request.")
        #expect(token.details == ["Jet needs a GitHub token saved on This Mac."])
        #expect(token.actions == [.howToSetUp])

        let timedOut = Self.delivery(9, .push(remote: "origin"), .failed(code: "git.timeout"))
        #expect(make(nil, [timedOut], Self.context)?.actions == [.tryAgainDelivery(timedOut)])

        let unknown = Self.delivery(9, .push(remote: "origin"), .outcomeUnknown)
        let unconfirmed = try #require(make(Self.progress([.push], [.push: .unconfirmed(unknown)]), [unknown], Self.context))
        #expect(unconfirmed.tone == .warning)
        #expect(unconfirmed.systemImage == "questionmark.circle")
        #expect(unconfirmed.headline == "Jet couldn't confirm whether the push happened.")
        #expect(unconfirmed.details == ["Check the branch on GitHub, then mark it as checked."])
        #expect(unconfirmed.actions == [.checkStatus, .markAsChecked(unknown)])

        var marking = Self.context
        marking.acknowledgements = [unknown.id: .marking]
        #expect(make(nil, [unknown], marking)?.actions == [])
        #expect(make(nil, [unknown], marking)?.details.last == "Marking as checked…")
        marking.acknowledgements = [unknown.id: .failed]
        #expect(make(nil, [unknown], marking)?.actions == [.checkStatus, .retryAcknowledgement(unknown)])
        #expect(make(nil, [unknown], marking)?.details.last == "Couldn't mark it as checked.")

        let checked = Self.delivery(9, .push(remote: "origin"), .outcomeUnknown, acknowledged: true)
        let acknowledged = try #require(make(nil, [checked], Self.context))
        #expect(acknowledged.tone == .neutral)
        #expect(acknowledged.details == ["Marked as checked. Jet didn't retry or undo it."])
        #expect(acknowledged.actions.isEmpty)

        let uncertain = try #require(make(Self.progress([.branch, .commit], [.branch: .completed(nil), .commit: .admissionUncertain]), [], Self.context))
        #expect(uncertain.tone == .warning)
        #expect(uncertain.headline == "Jet couldn't confirm it received the request.")
        #expect(uncertain.details == ["Commit (Jet writes the message)"])
        #expect(uncertain.actions == [.checkStatus, .sendSameRequestAgain])

        let notStarted = try #require(make(Self.progress([.branch, .commit, .push], [.branch: .completed(nil), .commit: .completed(nil)]), [], Self.context))
        #expect(notStarted.headline == "Push to origin · Not started")
        #expect(notStarted.details == ["Committed the changes from reply 2"])
        #expect(notStarted.actions == [.continuePlan])

        let pending = Self.delivery(9, .push(remote: "origin"), .pending)
        #expect(make(nil, [pending], Self.context)?.headline == "Pushing to origin…")
        #expect(make(nil, [pending], Self.context)?.tone == .running)
    }

    @Test
    func deliveryIDsTellWhenTheyWereCreated() {
        #expect(DeliveryTime.createdAt(UUID(uuidString: "0190A5B2-C3D4-7E5F-8000-000000000000")!) == 0x0190_A5B2_C3D4)
        #expect(DeliveryTime.createdAt(UUID(uuidString: "0190A5B2-C3D4-4E5F-8000-000000000000")!) == nil)
    }

    @Test
    func gitActivityOffersAFixOnlyOnTheNewestUnsupersededFailure() {
        let oldFailure = Self.delivery(1, .push(remote: "origin"), .failed(code: "git.index_locked"))
        let laterPush = Self.delivery(2, .push(remote: "origin"), FakeGitDeliveries.completed)
        let draftFailure = Self.delivery(3, .draftPullRequest(remote: "origin", base: nil), .failed(code: "git.github_timeout"))
        let history = DeliveryCoordinator.newestFirst([oldFailure, laterPush, draftFailure])
        #expect(GitActivityRules.newestUnsupersededFailure(in: history) == draftFailure)
        #expect(GitActivityRules.newestUnsupersededFailure(in: [laterPush, oldFailure]) == nil)
    }

    // MARK: - Session wiring

    @Test
    func menuTargetsAskForConfirmationFirst() {
        let session = DesktopSession(memory: ClientMemory(defaults: UserDefaults(suiteName: "jet.tests.delivery-session")!))
        let unknown = Self.delivery(9, .push(remote: "origin"), .outcomeUnknown)
        session.requestMarkAsChecked(unknown)
        #expect(session.deliveries.pendingConfirmation
            == .markAsChecked(Key(planeRegistryID: session.localPlaneRegistryID, conversationID: Self.taskA), unknown))

        session.deliveries.pendingConfirmation = nil
        session.requestMarkAsChecked(Self.delivery(9, .push(remote: "origin"), .outcomeUnknown, acknowledged: true))
        #expect(session.deliveries.pendingConfirmation == nil)

        session.requestRetry(.markAsChecked(Self.keyA, unknown))
        #expect(session.deliveries.pendingConfirmation == nil)
        session.requestRetry(.retryStep(Self.keyA, .commit))
        #expect(session.deliveries.pendingConfirmation == .retryStep(Self.keyA, .commit))
    }

    // MARK: - Lexicon

    @Test
    func casualKeepCopyAvoidsJetDomainWords() {
        var strings: [String] = []
        for step in Step.allCases {
            strings += [
                GitStepCopy.title(step, branch: "jet/x", remote: "origin"),
                GitStepCopy.title(step, branch: nil, remote: "origin", updatesPullRequest: true),
                GitStepCopy.running(step, branch: "jet/x", remote: "origin"),
                GitStepCopy.done(step, branch: "jet/x", remote: "origin", reply: 2),
                GitStepCopy.done(step, branch: nil, remote: "origin", reply: nil),
                GitStepCopy.failed(step, remote: "origin"),
                GitStepCopy.couldNotConfirm(step),
                GitStepCopy.followUp(step),
            ]
        }
        strings += [GitStepCopy.acknowledged, GitStepCopy.admissionUncertain]
        let states: [DeliveryCoordinator.StepState] = [.notStarted, .pending, .completed(nil), .failed(code: "x", delivery: nil), .admissionUncertain]
        strings += states.map(GitStepCopy.stateLabel)
        let codes = [
            "git.branch_exists", "git.invalid_name", "git.run_active", "git.delivery_unresolved",
            "git.content_changed", "git.index_locked", "git.policy_changed", "git.checkpoint_required",
            "git.remote_required", "git.branch_required", "git.push_required", "git.github_required",
            "git.github_credential_unavailable", "git.github_refused", "git.github_invalid",
            "git.draft_conflict", "git.timeout", "git.incomplete_or_dirty_baseline", "transport.offline", "other",
        ]
        for code in codes {
            for step in Step.allCases {
                let copy = GitFailureCopy.make(code: code, step: step, computer: "This Mac")
                strings.append(copy.sentence)
                if let fix = copy.fix { strings.append(fix.title) }
            }
        }
        let actions: [KeepChangesSummary.Action] = [
            .checkStatus, .sendSameRequestAgain, .tryAgainStep(.push), .chooseAnotherName,
            .keepChanges, .continuePlan, .howToSetUp, .copyBranchName("x"),
        ]
        strings += actions.map(\.title)
        for choice in KeepChangesChoice.allCases {
            strings.append(choice.title)
            strings.append(KeepChangesSheetCopy.caption(choice, facts: Self.facts(), existingBranch: nil, remote: "origin"))
            strings.append(KeepChangesSheetCopy.caption(choice, facts: Self.facts(isLocalCheckout: true), existingBranch: "jet/x", remote: "origin"))
            for mode in [KeepChangesMode.plan, .single(.branch), .single(.commit), .single(.push), .single(.draftPullRequest)] {
                let model = KeepChangesModel(mode: mode)
                model.apply(Self.facts())
                model.choice = choice
                strings += [model.sheetTitle, model.primaryTitle] + model.steps.map(model.stepTitle)
                if let warning = model.singleStepWarning { strings.append(warning) }
            }
        }
        strings += [KeepChangesModel.EmptyState.noChanges.text, KeepChangesModel.EmptyState.alreadyKept.text]
        strings += ["", "a b", "a~", "a..b"].compactMap(KeepChangesNaming.branchIssue)
        strings += ["", "a@b"].compactMap(KeepChangesNaming.remoteIssue)
        if let subtitle = KeepChangesSheetCopy.subtitle(Self.facts()) { strings.append(String(subtitle.characters)) }

        for string in strings {
            #expect(JetCopy.foundAvoidWords(in: string).isEmpty, "\(string)")
        }
    }
}
