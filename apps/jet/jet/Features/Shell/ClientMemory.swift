import Foundation
import Observation

/// Small client-owned memories kept in UserDefaults. They are rebuildable
/// conveniences, never authoritative Jet state: only IDs, sequence numbers and
/// Jet Trash titles are stored, never prompts or output (ASVS 14.2.3).
@MainActor
@Observable
final class ClientMemory {
    static let seenKey = "jet.seen.v1"
    static let assistantsKey = "jet.task-assistants.v1"
    static let trashTitlesKey = "jet.trash-titles.v1"
    static let offerShownKey = "jet.notifications.offer-shown"
    /// Each memory keeps at most this many Conversations and drops the oldest.
    static let capacity = 512

    @ObservationIgnored private let defaults: UserDefaults
    private var seen: RecencyLedger<UInt64>
    private var assistants: RecencyLedger<String>
    private var trashTitles: RecencyLedger<String>
    private var offerShown: Bool

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        seen = Self.load(Self.seenKey, from: defaults)
        assistants = Self.load(Self.assistantsKey, from: defaults)
        trashTitles = Self.load(Self.trashTitlesKey, from: defaults)
        offerShown = defaults.bool(forKey: Self.offerShownKey)
    }

    // MARK: Seen replies

    /// The newest reply sequence the person has seen in this Conversation.
    func seenSequence(_ conversationID: UUID) -> UInt64? {
        seen[conversationID]
    }

    /// Records a seen reply sequence. Sequences only move forward.
    func markSeen(_ conversationID: UUID, sequence: UInt64) {
        if let current = seen[conversationID], current >= sequence { return }
        seen.set(sequence, for: conversationID, capacity: Self.capacity)
        save(seen, as: Self.seenKey)
    }

    // MARK: Assistants

    /// The Craft ID this client used to start the Conversation's first Run.
    /// Runs don't expose their Craft, so other clients' tasks stay unknown.
    func assistant(for conversationID: UUID) -> String? {
        assistants[conversationID]
    }

    func recordAssistant(_ craftID: String, for conversationID: UUID) {
        guard assistants[conversationID] != craftID else { return }
        assistants.set(craftID, for: conversationID, capacity: Self.capacity)
        save(assistants, as: Self.assistantsKey)
    }

    // MARK: Jet Trash titles

    /// Trash entries carry no title, so Jet remembers titles it moved to Jet Trash.
    func trashedTitle(_ conversationID: UUID) -> String? {
        trashTitles[conversationID]
    }

    func recordTrashedTitle(_ title: String, for conversationID: UUID) {
        trashTitles.set(title, for: conversationID, capacity: Self.capacity)
        save(trashTitles, as: Self.trashTitlesKey)
    }

    func forgetTrashedTitle(_ conversationID: UUID) {
        guard trashTitles[conversationID] != nil else { return }
        trashTitles.remove(conversationID)
        save(trashTitles, as: Self.trashTitlesKey)
    }

    // MARK: Notification offer

    /// Whether the one-time "Get a notification…" offer was already shown.
    var notificationOfferShown: Bool {
        get { offerShown }
        set {
            offerShown = newValue
            defaults.set(newValue, forKey: Self.offerShownKey)
        }
    }

    // MARK: Storage

    private static func load<Value: Codable & Sendable>(
        _ key: String,
        from defaults: UserDefaults
    ) -> RecencyLedger<Value> {
        guard let data = defaults.data(forKey: key),
              let ledger = try? JSONDecoder().decode(RecencyLedger<Value>.self, from: data)
        else { return RecencyLedger() }
        return ledger.trimmed(to: capacity)
    }

    private func save<Value: Codable & Sendable>(_ ledger: RecencyLedger<Value>, as key: String) {
        guard let data = try? JSONEncoder().encode(ledger) else { return }
        defaults.set(data, forKey: key)
    }
}

/// A bounded map from Conversation ID to a value, ordered from least to most
/// recently written.
nonisolated struct RecencyLedger<Value: Codable & Sendable>: Codable, Sendable {
    private struct Entry: Codable, Sendable {
        let id: UUID
        var value: Value
    }

    private var entries: [Entry] = []
    private var index: [UUID: Value] = [:]

    init() {}

    init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        var seen = Set<UUID>()
        // Keep the newest entry when a stored ledger repeats an ID.
        let decoded = try container.decode([Entry].self)
        entries = Array(decoded.reversed().filter { seen.insert($0.id).inserted }.reversed())
        index = Dictionary(uniqueKeysWithValues: entries.map { ($0.id, $0.value) })
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(entries)
    }

    var count: Int { entries.count }
    var ids: [UUID] { entries.map(\.id) }

    subscript(id: UUID) -> Value? { index[id] }

    mutating func set(_ value: Value, for id: UUID, capacity: Int) {
        if index[id] != nil { entries.removeAll { $0.id == id } }
        entries.append(Entry(id: id, value: value))
        index[id] = value
        trim(to: capacity)
    }

    mutating func remove(_ id: UUID) {
        guard index.removeValue(forKey: id) != nil else { return }
        entries.removeAll { $0.id == id }
    }

    func trimmed(to capacity: Int) -> Self {
        var copy = self
        copy.trim(to: capacity)
        return copy
    }

    private mutating func trim(to capacity: Int) {
        guard entries.count > capacity else { return }
        for entry in entries.prefix(entries.count - capacity) {
            index.removeValue(forKey: entry.id)
        }
        entries.removeFirst(entries.count - capacity)
    }
}
