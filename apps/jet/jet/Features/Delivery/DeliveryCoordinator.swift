import Foundation
import Observation

/// Runs a reviewed Keep Changes plan as separate `DeliverGit` Commands, keyed by
/// computer and task. Each step keeps its own Command ID and exact body, is pinned
/// to the reviewed checkpoint, starts only after the previous step completed, and
/// is never retried automatically (design §6.9).
///
/// Plans, request bodies and Git names stay in memory. The only persisted data is
/// which tasks may still have a Git step that needs the person (IDs only), so a
/// relaunch can check them again without sending anything.
@MainActor
@Observable
final class DeliveryCoordinator {
    struct Key: Hashable, Sendable, Codable {
        let planeRegistryID: UUID
        let conversationID: UUID
    }

    enum Step: String, CaseIterable, Hashable, Sendable, Codable {
        case branch
        case commit
        case push
        case draftPullRequest

        /// The step a recorded Git operation belongs to.
        init(_ operation: JetGitOperation) {
            switch operation {
            case .branch: self = .branch
            case .commit: self = .commit
            case .push: self = .push
            case .draftPullRequest: self = .draftPullRequest
            }
        }

        /// The step a single Changes-menu item runs.
        init(_ choice: JetGitDeliveryChoice) {
            switch choice {
            case .branch: self = .branch
            case .commit: self = .commit
            case .push: self = .push
            case .draftPullRequest: self = .draftPullRequest
            }
        }
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

        /// The step stopped the chain.
        var stopsChain: Bool {
            switch self {
            case .failed, .unconfirmed, .admissionUncertain: true
            case .notStarted, .submitting, .pending, .completed: false
            }
        }

        var isInFlight: Bool { self == .submitting || self == .pending }

        var isCompleted: Bool {
            if case .completed = self { return true }
            return false
        }
    }

    struct Plan: Equatable, Sendable {
        var steps: [Step]
        var branchName: String
        var remote: String
        var baseBranch: String?
        var checkpoint: JetGitCheckpoint?

        /// The exact body of one step. Commit and the draft pull request carry the
        /// reviewed checkpoint; branch and push carry none.
        func request(for step: Step, conversationID: UUID) -> JetGitDeliveryRequest {
            switch step {
            case .branch:
                JetGitDeliveryRequest(
                    conversationID: conversationID,
                    checkpoint: nil,
                    operation: .branch(name: branchName)
                )
            case .commit:
                JetGitDeliveryRequest(
                    conversationID: conversationID,
                    checkpoint: checkpoint,
                    operation: .commit
                )
            case .push:
                JetGitDeliveryRequest(
                    conversationID: conversationID,
                    checkpoint: nil,
                    operation: .push(remote: remote)
                )
            case .draftPullRequest:
                JetGitDeliveryRequest(
                    conversationID: conversationID,
                    checkpoint: checkpoint,
                    operation: .draftPullRequest(remote: remote, base: baseBranch)
                )
            }
        }
    }

    struct Progress: Equatable, Sendable {
        var plan: Plan
        var states: [Step: StepState]
        var isRunning: Bool
        /// The last status check couldn't reach the computer; Jet keeps checking.
        var lastCheckFailed = false
        /// The delivery each submitted step created.
        var deliveryIDs: [Step: UUID] = [:]
        /// The newest delivery known when the plan started. An uncertain step is
        /// only matched with deliveries newer than this.
        var baseline: UUID?

        func state(of step: Step) -> StepState {
            states[step] ?? .notStarted
        }

        /// The step being submitted or awaited.
        var currentStep: Step? {
            plan.steps.first { state(of: $0).isInFlight }
        }

        /// The step that stopped the chain: failed, couldn't confirm, or uncertain.
        var stoppedStep: Step? {
            plan.steps.first { state(of: $0).stopsChain }
        }

        var notStartedSteps: [Step] {
            plan.steps.filter { state(of: $0) == .notStarted }
        }

        var isFinished: Bool {
            plan.steps.allSatisfy { state(of: $0).isCompleted }
        }

        /// Whether any step was submitted.
        var hasStarted: Bool {
            plan.steps.contains { state(of: $0) != .notStarted }
        }
    }

    /// A confirmation the shell root presents (`keepChangesSupport`).
    enum Confirmation: Equatable, Sendable {
        case markAsChecked(Key, JetGitDelivery)
        case retryStep(Key, Step)
        case retryDelivery(Key, JetGitDelivery)

        var key: Key {
            switch self {
            case let .markAsChecked(key, _), let .retryStep(key, _), let .retryDelivery(key, _): key
            }
        }
    }

    enum HistoryStatus: Equatable, Sendable {
        case unknown
        case loading
        case loaded
        case failed
    }

    /// Marking an unconfirmed step as checked.
    enum Acknowledgement: Equatable, Sendable {
        case marking
        case failed
        /// jetd confirmed the acknowledgement; history shows it after the next check.
        case marked
    }

    typealias Submit = @MainActor (Key, JetGitDeliveryRequest, _ commandID: UUID) async throws -> UUID
    typealias Fetch = @MainActor (Key) async throws -> [JetGitDelivery]
    typealias OnDeliveries = @MainActor (Key, [JetGitDelivery]) -> Void
    typealias Acknowledge = @MainActor (_ deliveryID: UUID, _ commandID: UUID) async throws -> Void
    typealias Sleep = @Sendable (Duration) async throws -> Void

    private(set) var progressByKey: [Key: Progress] = [:]
    private(set) var historyByKey: [Key: [JetGitDelivery]] = [:]
    private(set) var historyStatusByKey: [Key: HistoryStatus] = [:]
    private(set) var acknowledgements: [UUID: Acknowledgement] = [:]
    /// Tasks from the last launch that may still have a Git step needing the person.
    private(set) var restoredKeys: [Key] = []
    var pendingConfirmation: Confirmation?

    @ObservationIgnored private(set) var submit: Submit?
    @ObservationIgnored private(set) var fetch: Fetch?
    @ObservationIgnored private(set) var onDeliveries: OnDeliveries?
    @ObservationIgnored var pollInterval: Duration = .seconds(2)
    @ObservationIgnored var maximumBackoff: Duration = .seconds(10)
    @ObservationIgnored var sleep: Sleep = { duration in try await Task.sleep(for: duration) }

    private let store: DeliveryChainStore
    @ObservationIgnored private var openKeys: [Key]
    @ObservationIgnored private var tasks: [Key: Task<Void, Never>] = [:]
    @ObservationIgnored private var refreshes: [Key: Task<Bool, Never>] = [:]
    /// Each step's exact body, in memory only.
    @ObservationIgnored private var bodies: [Key: [Step: JetGitDeliveryRequest]] = [:]
    /// Command IDs of steps that are being submitted or whose admission is uncertain.
    @ObservationIgnored private var commandIDs: [Key: [Step: UUID]] = [:]
    /// One Command ID per acknowledged delivery until jetd confirms it.
    @ObservationIgnored private var acknowledgementIDs: [UUID: UUID] = [:]

    init(store: DeliveryChainStore = .standard) {
        self.store = store
        let keys = store.load()
        openKeys = keys
        restoredKeys = keys
    }

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

    /// The task's Git deliveries, newest first, as of the last check.
    func history(for key: Key) -> [JetGitDelivery] {
        historyByKey[key] ?? []
    }

    func hasHistory(for key: Key) -> Bool {
        historyByKey[key] != nil
    }

    func historyStatus(for key: Key) -> HistoryStatus {
        historyStatusByKey[key] ?? .unknown
    }

    func acknowledgement(for deliveryID: UUID) -> Acknowledgement? {
        acknowledgements[deliveryID]
    }

    // MARK: - Running a plan

    /// A step whose request jetd may or may not have received. It keeps its
    /// Command ID, so no new plan starts until it is resent or found in history.
    func hasUncertainStep(for key: Key) -> Bool {
        guard let progress = progressByKey[key], let step = progress.stoppedStep else { return false }
        return progress.state(of: step) == .admissionUncertain
    }

    /// Starts a reviewed plan, or returns nil when this task already runs one or
    /// has an uncertain step.
    @discardableResult
    func start(_ plan: Plan, for key: Key) -> Task<Void, Never>? {
        guard progressByKey[key]?.isRunning != true, !hasUncertainStep(for: key), !plan.steps.isEmpty
        else { return nil }
        var requests: [Step: JetGitDeliveryRequest] = [:]
        for step in plan.steps {
            requests[step] = plan.request(for: step, conversationID: key.conversationID)
        }
        return begin(plan, requests: requests, for: key)
    }

    /// Submits a failed history step again: the exact body with a new Command ID.
    @discardableResult
    func retry(_ delivery: JetGitDelivery, for key: Key) -> Task<Void, Never>? {
        guard delivery.canRetry, progressByKey[key]?.isRunning != true, !hasUncertainStep(for: key)
        else { return nil }
        let step = Step(delivery.operation)
        let request = JetGitDeliveryRequest(
            conversationID: delivery.conversationID,
            checkpoint: delivery.checkpoint,
            operation: delivery.operation
        )
        var plan = Plan(
            steps: [step],
            branchName: Self.knownBranch(in: history(for: key)) ?? "",
            remote: delivery.remoteName ?? "origin",
            baseBranch: nil,
            checkpoint: delivery.checkpoint
        )
        switch delivery.operation {
        case let .branch(name): plan.branchName = name
        case let .draftPullRequest(_, base): plan.baseBranch = base
        case .commit, .push: break
        }
        return begin(plan, requests: [step: request], for: key)
    }

    /// Sends the uncertain request again with its retained Command ID and exact
    /// body, then continues with the rest of the reviewed plan.
    @discardableResult
    func resendUncertain(for key: Key) -> Task<Void, Never>? {
        guard var progress = progressByKey[key], !progress.isRunning,
              let step = progress.stoppedStep,
              progress.state(of: step) == .admissionUncertain,
              commandIDs[key]?[step] != nil,
              bodies[key]?[step] != nil
        else { return nil }
        progress.isRunning = true
        progressByKey[key] = progress
        let remaining = Array(progress.plan.steps.drop { $0 != step })
        return launch(remaining, for: key)
    }

    /// Sends only the failed step again, with a new Command ID and the same body,
    /// after the person confirmed it. Later steps stay "Not started" until the
    /// person continues the plan (lead decision L4).
    @discardableResult
    func retryFailedStep(for key: Key) -> Task<Void, Never>? {
        guard var progress = progressByKey[key], !progress.isRunning,
              let step = progress.stoppedStep,
              case .failed = progress.state(of: step),
              bodies[key]?[step] != nil
        else { return nil }
        commandIDs[key]?[step] = nil
        progress.deliveryIDs[step] = nil
        progress.states[step] = .notStarted
        progress.isRunning = true
        progress.baseline = history(for: key).first?.id ?? progress.baseline
        progressByKey[key] = progress
        return launch([step], for: key)
    }

    private func begin(
        _ plan: Plan,
        requests: [Step: JetGitDeliveryRequest],
        for key: Key
    ) -> Task<Void, Never> {
        bodies[key] = requests
        commandIDs[key] = [:]
        progressByKey[key] = Progress(
            plan: plan,
            states: [:],
            isRunning: true,
            baseline: history(for: key).first?.id
        )
        return launch(plan.steps, for: key)
    }

    private func launch(_ steps: [Step], for key: Key) -> Task<Void, Never> {
        markOpen(key)
        let task = Task { [weak self] in
            guard let self else { return }
            await self.run(steps, for: key)
        }
        tasks[key] = task
        return task
    }

    private func run(_ steps: [Step], for key: Key) async {
        for step in steps {
            guard await perform(step, for: key) else { break }
        }
        tasks[key] = nil
        if var progress = progressByKey[key] {
            progress.isRunning = false
            progressByKey[key] = progress
        }
        updateOpenState(for: key)
    }

    /// Submits one step and waits for its outcome. True when it completed.
    private func perform(_ step: Step, for key: Key) async -> Bool {
        guard let submit, let request = bodies[key]?[step] else {
            setState(.failed(code: "request.failed", delivery: nil), of: step, for: key)
            return false
        }
        let commandID = commandIDs[key]?[step] ?? UUID()
        commandIDs[key, default: [:]][step] = commandID
        setState(.submitting, of: step, for: key)
        let deliveryID: UUID
        do {
            deliveryID = try await submit(key, request, commandID)
        } catch {
            let state = Self.admissionState(for: error)
            if state != .admissionUncertain { commandIDs[key]?[step] = nil }
            setState(state, of: step, for: key)
            return false
        }
        commandIDs[key]?[step] = nil
        progressByKey[key]?.deliveryIDs[step] = deliveryID
        setState(.pending, of: step, for: key)
        return await poll(deliveryID, for: key)
    }

    /// The state for a request that didn't return a delivery. Only a definite
    /// refusal is a failure; anything that may have reached jetd is uncertain.
    static func admissionState(for error: Error) -> StepState {
        switch error {
        case JetClientFailure.commandOutcomeUnknown, is CancellationError:
            return .admissionUncertain
        case let JetClientFailure.presentation(failure):
            return state(forRefusal: failure)
        case let failure as JetPresentationError:
            return state(forRefusal: failure)
        default:
            return .failed(code: "request.failed", delivery: nil)
        }
    }

    private static func state(forRefusal failure: JetPresentationError) -> StepState {
        switch failure.category {
        case .cancelled, .outcomeUnknown: .admissionUncertain
        default: .failed(code: failure.code, delivery: nil)
        }
    }

    private enum Check {
        case waiting
        case completed
        case stopped
        case unreachable
    }

    /// Checks until the delivery leaves pending. Failed checks back off up to
    /// `maximumBackoff`; nothing is ever submitted again.
    private func poll(_ deliveryID: UUID, for key: Key) async -> Bool {
        var delay = pollInterval
        while !Task.isCancelled {
            switch await check(deliveryID, for: key) {
            case .completed: return true
            case .stopped: return false
            case .waiting: delay = pollInterval
            case .unreachable: delay = min(delay * 2, maximumBackoff)
            }
            do {
                try await sleep(delay)
            } catch {
                return false
            }
        }
        return false
    }

    private func check(_ deliveryID: UUID, for key: Key) async -> Check {
        guard let fetch else { return .stopped }
        do {
            let deliveries = try await fetch(key)
            apply(deliveries, for: key, notify: true)
            progressByKey[key]?.lastCheckFailed = false
            guard let delivery = deliveries.first(where: { $0.id == deliveryID }) else { return .waiting }
            switch delivery.outcome {
            case .pending: return .waiting
            case .completed: return .completed
            case .failed, .outcomeUnknown: return .stopped
            }
        } catch is CancellationError {
            return .stopped
        } catch {
            progressByKey[key]?.lastCheckFailed = true
            return .unreachable
        }
    }

    private func setState(_ state: StepState, of step: Step, for key: Key) {
        progressByKey[key]?.states[step] = state
    }

    // MARK: - History

    /// Reads the task's deliveries. It never submits anything and never resumes a
    /// plan; it reconciles known steps and adopts an uncertain step that jetd did
    /// record. Concurrent calls for one task share one query. True when it loaded.
    @discardableResult
    func refresh(for key: Key) async -> Bool {
        if let running = refreshes[key] { return await running.value }
        guard let fetch else { return false }
        if historyByKey[key] == nil { historyStatusByKey[key] = .loading }
        let task = Task { [weak self] () -> Bool in
            do {
                let deliveries = try await fetch(key)
                guard let self else { return false }
                self.apply(deliveries, for: key, notify: true)
                self.progressByKey[key]?.lastCheckFailed = false
                return true
            } catch {
                guard let self else { return false }
                self.historyStatusByKey[key] = .failed
                self.progressByKey[key]?.lastCheckFailed = true
                return false
            }
        }
        refreshes[key] = task
        let loaded = await task.value
        refreshes[key] = nil
        if loaded { restoredKeys.removeAll { $0 == key } }
        updateOpenState(for: key)
        return loaded
    }

    /// Takes deliveries another query loaded for this task (the session's
    /// `gitDeliveries`). An empty or foreign list is ignored.
    func absorb(_ deliveries: [JetGitDelivery], for key: Key) {
        guard !deliveries.isEmpty,
              deliveries.allSatisfy({ $0.conversationID == key.conversationID })
        else { return }
        apply(deliveries, for: key, notify: false)
        updateOpenState(for: key)
    }

    /// Checks restored tasks whose computer is connected, once each.
    func refreshRestoredChains(isConnected: (UUID) -> Bool) async {
        for key in restoredKeys where isConnected(key.planeRegistryID) {
            await refresh(for: key)
        }
    }

    private func apply(_ deliveries: [JetGitDelivery], for key: Key, notify: Bool) {
        let sorted = Self.newestFirst(deliveries.filter { $0.conversationID == key.conversationID })
        if historyByKey[key] != sorted { historyByKey[key] = sorted }
        if historyStatusByKey[key] != .loaded { historyStatusByKey[key] = .loaded }
        for delivery in sorted where delivery.acknowledgedBy != nil && acknowledgements[delivery.id] != nil {
            acknowledgements[delivery.id] = nil
            acknowledgementIDs[delivery.id] = nil
        }
        reconcile(key)
        if notify { onDeliveries?(key, sorted) }
    }

    /// Updates submitted steps from history, and adopts an uncertain step when a
    /// matching delivery newer than the baseline exists. It never starts a step.
    private func reconcile(_ key: Key) {
        guard var progress = progressByKey[key] else { return }
        let history = history(for: key)
        if !progress.isRunning,
           let step = progress.stoppedStep,
           progress.state(of: step) == .admissionUncertain,
           let request = bodies[key]?[step]
        {
            let claimed = Set(progress.deliveryIDs.values)
            if let match = history.first(where: {
                !claimed.contains($0.id)
                    && Self.isNewer($0.id, than: progress.baseline)
                    && $0.operation == request.operation
                    && $0.checkpoint == request.checkpoint
            }) {
                progress.deliveryIDs[step] = match.id
                commandIDs[key]?[step] = nil
            }
        }
        for (step, deliveryID) in progress.deliveryIDs {
            guard let delivery = history.first(where: { $0.id == deliveryID }) else { continue }
            progress.states[step] = Self.state(for: delivery)
        }
        if progressByKey[key] != progress { progressByKey[key] = progress }
    }

    static func state(for delivery: JetGitDelivery) -> StepState {
        switch delivery.outcome {
        case .pending: .pending
        case .completed: .completed(delivery)
        case let .failed(code): .failed(code: code, delivery: delivery)
        case .outcomeUnknown: .unconfirmed(delivery)
        }
    }

    /// Delivery IDs are UUIDv7, so their text sorts by creation time.
    static func newestFirst(_ deliveries: [JetGitDelivery]) -> [JetGitDelivery] {
        deliveries.sorted { $0.id.uuidString > $1.id.uuidString }
    }

    static func isNewer(_ id: UUID, than baseline: UUID?) -> Bool {
        guard let baseline else { return true }
        return id.uuidString > baseline.uuidString
    }

    /// The branch the task's newest completed Git step created or worked on.
    static func knownBranch(in history: [JetGitDelivery]) -> String? {
        for delivery in history {
            guard case let .completed(_, branch, _) = delivery.outcome else { continue }
            if case let .branch(name) = delivery.operation { return branch ?? name }
            if let branch { return branch }
        }
        return nil
    }

    // MARK: - Mark as Checked

    /// Records that the person checked an unconfirmed step. One Command ID is kept
    /// per delivery until jetd confirms it, so Try Again resends the same Command.
    /// True when jetd confirmed it.
    @discardableResult
    func acknowledge(
        _ delivery: JetGitDelivery,
        for key: Key,
        send: Acknowledge
    ) async -> Bool {
        guard delivery.needsAcknowledgement,
              acknowledgements[delivery.id] != .marking,
              acknowledgements[delivery.id] != .marked
        else { return false }
        let commandID = acknowledgementIDs[delivery.id] ?? UUID()
        acknowledgementIDs[delivery.id] = commandID
        acknowledgements[delivery.id] = .marking
        do {
            try await send(delivery.id, commandID)
        } catch {
            acknowledgements[delivery.id] = .failed
            return false
        }
        acknowledgementIDs[delivery.id] = nil
        acknowledgements[delivery.id] = .marked
        await refresh(for: key)
        return true
    }

    /// The Command ID kept for a delivery's acknowledgement.
    func acknowledgementCommandID(for deliveryID: UUID) -> UUID? {
        acknowledgementIDs[deliveryID]
    }

    // MARK: - Remembering tasks across a relaunch

    /// A task stays remembered while one of its Git steps is in flight, uncertain,
    /// pending in jetd, or couldn't be confirmed and isn't checked yet.
    private func updateOpenState(for key: Key) {
        let progress = progressByKey[key]
        let progressIsOpen = progress.map { progress in
            progress.isRunning || progress.plan.steps.contains { step in
                switch progress.state(of: step) {
                case .submitting, .pending, .admissionUncertain: true
                case let .unconfirmed(delivery): delivery?.acknowledgedBy == nil
                case .notStarted, .completed, .failed: false
                }
            }
        } ?? false
        let historyIsOpen = history(for: key).contains {
            $0.outcome == .pending || $0.needsAcknowledgement
        }
        if progressIsOpen || historyIsOpen {
            markOpen(key)
        } else if hasHistory(for: key) || progress != nil {
            markClosed(key)
        }
    }

    private func markOpen(_ key: Key) {
        guard openKeys.last != key else { return }
        openKeys.removeAll { $0 == key }
        openKeys.append(key)
        if openKeys.count > DeliveryChainStore.limit {
            openKeys.removeFirst(openKeys.count - DeliveryChainStore.limit)
        }
        store.save(openKeys)
    }

    private func markClosed(_ key: Key) {
        guard openKeys.contains(key) else { return }
        openKeys.removeAll { $0 == key }
        store.save(openKeys)
    }

#if DEBUG
    /// Seeds a task's progress and history for previews and screenshots. Nothing
    /// is submitted or fetched.
    func previewSeed(
        progress: Progress?,
        history: [JetGitDelivery],
        for key: Key,
        historyStatus: HistoryStatus = .loaded
    ) {
        if let progress {
            progressByKey[key] = progress
            var requests: [Step: JetGitDeliveryRequest] = [:]
            for step in progress.plan.steps {
                requests[step] = progress.plan.request(for: step, conversationID: key.conversationID)
            }
            bodies[key] = requests
        }
        historyByKey[key] = Self.newestFirst(history)
        historyStatusByKey[key] = historyStatus
    }

    func previewSeedAcknowledgement(_ state: Acknowledgement, for deliveryID: UUID) {
        acknowledgements[deliveryID] = state
    }
#endif
}

/// Remembers which tasks may still have a Git step that needs the person, so a
/// relaunch can check them again (critic 4.3). It holds only computer and task
/// IDs under `jet.keep-changes.v1`; plans, request bodies and Git names are never
/// persisted (lead decision L3).
struct DeliveryChainStore {
    static let defaultsKey = "jet.keep-changes.v1"
    /// The most tasks remembered; the oldest are forgotten first.
    static let limit = 64

    let load: () -> [DeliveryCoordinator.Key]
    let save: ([DeliveryCoordinator.Key]) -> Void

    static var standard: DeliveryChainStore { userDefaults(.standard) }

    static func userDefaults(_ defaults: UserDefaults) -> DeliveryChainStore {
        DeliveryChainStore(
            load: {
                guard let data = defaults.data(forKey: defaultsKey),
                      let keys = try? JSONDecoder().decode([DeliveryCoordinator.Key].self, from: data)
                else { return [] }
                return Array(keys.suffix(limit))
            },
            save: { keys in
                if keys.isEmpty {
                    defaults.removeObject(forKey: defaultsKey)
                } else if let data = try? JSONEncoder().encode(Array(keys.suffix(limit))) {
                    defaults.set(data, forKey: defaultsKey)
                }
            }
        )
    }

    static func inMemory(_ initial: [DeliveryCoordinator.Key] = []) -> DeliveryChainStore {
        let box = Box(initial)
        return DeliveryChainStore(load: { box.keys }, save: { box.keys = $0 })
    }

    static var disabled: DeliveryChainStore {
        DeliveryChainStore(load: { [] }, save: { _ in })
    }

    private final class Box {
        var keys: [DeliveryCoordinator.Key]
        init(_ keys: [DeliveryCoordinator.Key]) { self.keys = keys }
    }
}

extension JetGitDelivery {
    var step: DeliveryCoordinator.Step { DeliveryCoordinator.Step(operation) }

    /// The branch this step created or worked on, when known.
    var branchName: String? {
        if case let .completed(_, branch, _) = outcome, let branch { return branch }
        if case let .branch(name) = operation { return name }
        return nil
    }

    var remoteName: String? {
        switch operation {
        case let .push(remote), let .draftPullRequest(remote, _): remote
        case .branch, .commit: nil
        }
    }

    /// The draft pull request's link, only for https github.com URLs.
    var pullRequestURL: URL? {
        guard case let .completed(_, _, pullRequest) = outcome else { return nil }
        return GitHubPullRequestLink.url(pullRequest)
    }

    var failureCode: String? {
        if case let .failed(code) = outcome { return code }
        return nil
    }

    var commitID: String? {
        if case let .completed(head, _, _) = outcome, !head.isEmpty { return head }
        return nil
    }
}
