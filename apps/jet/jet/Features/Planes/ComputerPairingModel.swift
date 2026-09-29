import Foundation
import Observation

/// The Pairing Commands and Query that let another Mac connect to a computer.
nonisolated protocol JetPairingAccess: Sendable {
    func pairing() async throws -> JetPairingSummary
    func setPairingGate(_ gate: String, commandID: UUID) async throws -> String
    func openManualPairing(commandID: UUID) async throws -> JetOpenedPairing
    func confirmPairing(
        offerID: UUID,
        authenticationString: String,
        commandID: UUID
    ) async throws -> JetPendingPairing
    func setPairedClientAccess(
        clientID: UUID,
        access: JetPairedClientAccess,
        commandID: UUID
    ) async throws -> JetPairedClientSummary
    func revokePairedClient(clientID: UUID, commandID: UUID) async throws -> UUID
}

extension JetClient: JetPairingAccess {}

typealias JetPairingAccessProvider = @MainActor (UUID) async throws -> any JetPairingAccess

/// The eight-digit code another Mac shows, entered as `1234-5678` or `12345678`.
nonisolated enum PairingCode {
    static let digitCount = 8

    /// Whether `text` is eight digits, optionally grouped with a hyphen or spaces.
    static func isValid(_ text: String) -> Bool {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty,
              trimmed.allSatisfy({ $0.isASCII && ($0.isNumber || $0 == "-" || $0 == " ") })
        else { return false }
        return digits(trimmed).count == digitCount
    }

    /// Only the digits of `text`.
    static func digits(_ text: String) -> String {
        String(text.filter { $0.isASCII && $0.isNumber })
    }
}

/// Allowing another Mac to connect to one of your computers, and the Jet apps
/// already connected to it. It opens the Pairing gate only when it is closed and
/// closes it again only when it opened it.
@MainActor
@Observable
final class ComputerPairingModel {
    enum AllowPhase: Equatable, Sendable {
        case idle
        case gettingCode
        /// The code is shown; nobody entered it yet.
        case showingCode(String)
        /// The other Mac entered the code; both show these numbers.
        case comparing(String)
        /// This Mac confirmed; the other Mac still has to finish.
        case waitingForOtherMac
        case connected
        case ended(AllowEnd)
        /// Jet couldn't get a code. See `issue`.
        case failed
    }

    enum AllowEnd: Equatable, Sendable {
        case expired
        case tooManyAttempts
        /// Don't Match, or the gate closed some other way.
        case stopped
    }

    private enum Intent: Hashable {
        case gate(plane: UUID, open: Bool)
        case open(plane: UUID)
        case confirm(plane: UUID, offerID: UUID)
        case access(plane: UUID, clientID: UUID, enabled: Bool)
        case revoke(plane: UUID, clientID: UUID)
    }

    private let makeAccess: JetPairingAccessProvider
    private let now: @Sendable () -> Date
    private let sleep: @Sendable (Duration) async throws -> Void
    private var commandIDs: [Intent: UUID] = [:]
    private var pollTask: Task<Void, Never>?
    private var allowGeneration = 0

    /// The latest Pairing of each computer this model read. It can be newer than
    /// the setup snapshot, so the Computers pane prefers it.
    var summaries: [UUID: JetPairingSummary] = [:]
    var allowPhase = AllowPhase.idle
    /// The computer the Allow sheet is for.
    var allowPlaneRegistryID: UUID?
    var offerID: UUID?
    var offerExpiresAt: Date?
    /// The Jet app that entered the code, once it has.
    var claimingClientID: UUID?
    /// This model opened the gate, so it closes it when the sheet goes away.
    var ownsGate = false
    var isWorking = false
    var issue: JetPresentationError?
    var notice: String?
    /// Access and revoke operations in flight, by client ID.
    var clientOperations: Set<UUID> = []

    init(
        makeAccess: @escaping JetPairingAccessProvider,
        now: @escaping @Sendable () -> Date = Date.init,
        sleep: @escaping @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
    ) {
        self.makeAccess = makeAccess
        self.now = now
        self.sleep = sleep
    }

    /// The Jet apps connected to a computer, from the freshest summary available.
    func clients(of planeRegistryID: UUID, fallback: JetPairingSummary?) -> [JetPairedClientSummary] {
        (summaries[planeRegistryID] ?? fallback)?.clients ?? []
    }

    /// Reads each computer's Pairing. Failures keep the last summary.
    func refresh(_ planeRegistryIDs: [UUID]) async {
        for planeRegistryID in planeRegistryIDs {
            guard let access = try? await makeAccess(planeRegistryID),
                  let summary = try? await access.pairing()
            else { continue }
            summaries[planeRegistryID] = summary
        }
    }

    // MARK: - Allow Another Mac to Connect

    /// Opens the gate if it is closed, opens a manual-code offer and starts
    /// checking every 2 seconds.
    func beginAllowing(on planeRegistryID: UUID) async {
        stopPolling()
        allowGeneration += 1
        let generation = allowGeneration
        allowPlaneRegistryID = planeRegistryID
        allowPhase = .gettingCode
        offerID = nil
        offerExpiresAt = nil
        claimingClientID = nil
        issue = nil
        isWorking = true
        defer { if generation == allowGeneration { isWorking = false } }
        do {
            let access = try await makeAccess(planeRegistryID)
            let summary = try await access.pairing()
            guard generation == allowGeneration else { return }
            summaries[planeRegistryID] = summary
            if summary.gate != "open" {
                _ = try await withCommandID(.gate(plane: planeRegistryID, open: true)) { commandID in
                    try await access.setPairingGate("open", commandID: commandID)
                }
                ownsGate = true
                guard generation == allowGeneration else {
                    // The sheet closed while the gate was opening.
                    await closeOwnedGate(on: planeRegistryID)
                    return
                }
            }
            let opened = try await withCommandID(.open(plane: planeRegistryID)) { commandID in
                try await access.openManualPairing(commandID: commandID)
            }
            guard generation == allowGeneration else { return }
            offerID = opened.pending.id
            offerExpiresAt = Date(
                timeIntervalSince1970: TimeInterval(opened.pending.expiresAtUnixMilliseconds) / 1_000
            )
            switch opened.disclosure {
            case let .manualCode(code):
                allowPhase = .showingCode(code)
                startPolling(generation: generation)
            case .qrPayload, .alreadyDisclosed:
                // Only a manual code can be read out to the other Mac.
                issue = .invalidInput(
                    code: "pairing.code_unavailable",
                    message: String(localized: "Jet couldn't show a code. Get a new code to try again.")
                )
                allowPhase = .failed
            }
        } catch {
            guard generation == allowGeneration else { return }
            issue = Self.presentationError(error)
            allowPhase = .failed
        }
    }

    /// Reads the Pairing once and moves the sheet on. Returns false once there is
    /// nothing left to wait for.
    @discardableResult
    func pollOnce() async -> Bool {
        guard let planeRegistryID = allowPlaneRegistryID, let offerID else { return false }
        let generation = allowGeneration
        let summary: JetPairingSummary
        do {
            let access = try await makeAccess(planeRegistryID)
            summary = try await access.pairing()
        } catch {
            // A missed check isn't an answer; keep waiting until the offer expires.
            guard generation == allowGeneration else { return false }
            if let offerExpiresAt, now() >= offerExpiresAt, case .showingCode = allowPhase {
                finish(.ended(.expired))
                return false
            }
            return true
        }
        guard generation == allowGeneration else { return false }
        summaries[planeRegistryID] = summary

        if let pending = summary.pending, pending.id == offerID {
            offerExpiresAt = Date(timeIntervalSince1970: TimeInterval(pending.expiresAtUnixMilliseconds) / 1_000)
            switch pending.progress {
            case .offered:
                return true
            case let .awaitingConfirmation(clientID, authenticationString):
                claimingClientID = clientID
                if case .waitingForOtherMac = allowPhase { return true }
                allowPhase = .comparing(authenticationString)
                return true
            case let .confirmed(clientID, _):
                claimingClientID = clientID
                allowPhase = .waitingForOtherMac
                return true
            case let .ended(reason):
                finish(.ended(Self.end(reason)))
                return false
            }
        }

        // The offer is gone: the other Mac finished, or it ended.
        if let claimingClientID, summary.clients.contains(where: { $0.id == claimingClientID }) {
            finish(.connected)
        } else if let offerExpiresAt, now() >= offerExpiresAt {
            finish(.ended(.expired))
        } else {
            finish(.ended(.stopped))
        }
        return false
    }

    /// Codes Match: confirms the numbers both Macs show.
    func confirmCodesMatch() async {
        guard case let .comparing(authenticationString) = allowPhase,
              let planeRegistryID = allowPlaneRegistryID,
              let offerID,
              !isWorking
        else { return }
        let generation = allowGeneration
        isWorking = true
        issue = nil
        defer { if generation == allowGeneration { isWorking = false } }
        do {
            let access = try await makeAccess(planeRegistryID)
            let pending = try await withCommandID(.confirm(plane: planeRegistryID, offerID: offerID)) { commandID in
                try await access.confirmPairing(
                    offerID: offerID,
                    authenticationString: authenticationString,
                    commandID: commandID
                )
            }
            guard generation == allowGeneration else { return }
            if case let .ended(reason) = pending.progress {
                finish(.ended(Self.end(reason)))
            } else {
                allowPhase = .waitingForOtherMac
            }
        } catch {
            guard generation == allowGeneration else { return }
            issue = Self.presentationError(error)
        }
    }

    /// Don't Match: closes the gate so the offer ends. It never confirms.
    func codesDontMatch() async {
        guard let planeRegistryID = allowPlaneRegistryID else { return }
        let generation = allowGeneration
        stopPolling()
        isWorking = true
        issue = nil
        defer { if generation == allowGeneration { isWorking = false } }
        do {
            let access = try await makeAccess(planeRegistryID)
            _ = try await withCommandID(.gate(plane: planeRegistryID, open: false)) { commandID in
                try await access.setPairingGate("closed", commandID: commandID)
            }
            ownsGate = false
            guard generation == allowGeneration else { return }
            allowPhase = .ended(.stopped)
            await refresh([planeRegistryID])
        } catch {
            guard generation == allowGeneration else { return }
            issue = Self.presentationError(error)
            allowPhase = .ended(.stopped)
        }
    }

    /// The sheet went away: stops checking and closes the gate if this model
    /// opened it.
    func endAllowing() async {
        stopPolling()
        allowGeneration += 1
        let planeRegistryID = allowPlaneRegistryID
        allowPhase = .idle
        offerID = nil
        offerExpiresAt = nil
        claimingClientID = nil
        issue = nil
        isWorking = false
        guard let planeRegistryID else { return }
        await closeOwnedGate(on: planeRegistryID)
    }

    /// Closes the gate only when this model opened it.
    private func closeOwnedGate(on planeRegistryID: UUID) async {
        guard ownsGate else { return }
        do {
            let access = try await makeAccess(planeRegistryID)
            _ = try await withCommandID(.gate(plane: planeRegistryID, open: false)) { commandID in
                try await access.setPairingGate("closed", commandID: commandID)
            }
            ownsGate = false
        } catch {
            // The offer expires on its own; the gate stays as the core has it.
        }
        await refresh([planeRegistryID])
    }

    private func finish(_ phase: AllowPhase) {
        stopPolling()
        allowPhase = phase
    }

    private func startPolling(generation: Int) {
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, generation == self.allowGeneration else { return }
                do { try await self.sleep(.seconds(2)) } catch { return }
                guard !Task.isCancelled, generation == self.allowGeneration else { return }
                guard await self.pollOnce() else { return }
            }
        }
    }

    private func stopPolling() {
        pollTask?.cancel()
        pollTask = nil
    }

    private static func end(_ reason: String) -> AllowEnd {
        switch reason {
        case "expired": .expired
        case "too_many_attempts": .tooManyAttempts
        default: .stopped
        }
    }

    // MARK: - Connected Jet apps

    func setAccess(_ client: JetPairedClientSummary, enabled: Bool, on planeRegistryID: UUID) async {
        guard !clientOperations.contains(client.id) else { return }
        clientOperations.insert(client.id)
        defer { clientOperations.remove(client.id) }
        notice = nil
        do {
            let access = try await makeAccess(planeRegistryID)
            let intent = Intent.access(plane: planeRegistryID, clientID: client.id, enabled: enabled)
            _ = try await withCommandID(intent) { commandID in
                try await access.setPairedClientAccess(
                    clientID: client.id,
                    access: enabled ? .enabled : .disabled,
                    commandID: commandID
                )
            }
            await refresh([planeRegistryID])
            notice = enabled
                ? String(localized: "That Jet app can connect again.")
                : String(localized: "That Jet app can't connect until you turn its access back on.")
        } catch {
            notice = Self.failureNotice(error)
        }
    }

    func revoke(_ client: JetPairedClientSummary, on planeRegistryID: UUID) async {
        guard !clientOperations.contains(client.id) else { return }
        clientOperations.insert(client.id)
        defer { clientOperations.remove(client.id) }
        notice = nil
        do {
            let access = try await makeAccess(planeRegistryID)
            _ = try await withCommandID(.revoke(plane: planeRegistryID, clientID: client.id)) { commandID in
                try await access.revokePairedClient(clientID: client.id, commandID: commandID)
            }
            await refresh([planeRegistryID])
            notice = String(localized: "That Jet app can no longer connect.")
        } catch {
            notice = Self.failureNotice(error)
        }
    }

    // MARK: - Commands

    /// Runs one Command with the ID kept for `intent`. The ID is dropped on
    /// success or a definite error and kept after an unknown outcome, so the
    /// person's explicit retry replays the same Command.
    private func withCommandID<T>(
        _ intent: Intent,
        _ send: (UUID) async throws -> T
    ) async throws -> T {
        let commandID = commandIDs[intent] ?? UUID()
        commandIDs[intent] = commandID
        do {
            let value = try await send(commandID)
            commandIDs.removeValue(forKey: intent)
            return value
        } catch {
            if case JetClientFailure.commandOutcomeUnknown = error {} else {
                commandIDs.removeValue(forKey: intent)
            }
            throw error
        }
    }

    private static func failureNotice(_ error: Error) -> String {
        if case JetClientFailure.commandOutcomeUnknown = error {
            return String(localized: "Jet couldn't confirm this change. Check the list before trying again.")
        }
        return String(localized: "Jet couldn't change that Jet app's access.")
    }

    private static func presentationError(_ error: Error) -> JetPresentationError {
        switch error {
        case let JetClientFailure.presentation(error): error
        case let error as JetPresentationError: error
        case JetClientFailure.commandOutcomeUnknown:
            JetPresentationError(
                category: .outcomeUnknown,
                code: "command.outcome_unknown",
                message: String(localized: "Jet couldn't confirm this change."),
                retryable: false
            )
        default: .invalidResponse
        }
    }
}
