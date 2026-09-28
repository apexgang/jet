import Foundation
import Testing
@testable import jet

@MainActor
struct ClientMemoryTests {
    @Test
    func memoriesSurviveAReload() {
        let (defaults, suite) = Self.isolatedDefaults()
        defer { defaults.removePersistentDomain(forName: suite) }
        let task = UUID()
        let trashed = UUID()

        let memory = ClientMemory(defaults: defaults)
        #expect(memory.seenSequence(task) == nil)
        #expect(!memory.notificationOfferShown)
        memory.markSeen(task, sequence: 42)
        memory.recordAssistant("jet-craft-claude", for: task)
        memory.recordTrashedTitle("Fix login redirect loop", for: trashed)
        memory.notificationOfferShown = true

        let reloaded = ClientMemory(defaults: defaults)
        #expect(reloaded.seenSequence(task) == 42)
        #expect(reloaded.assistant(for: task) == "jet-craft-claude")
        #expect(reloaded.trashedTitle(trashed) == "Fix login redirect loop")
        #expect(reloaded.notificationOfferShown)
        #expect(defaults.bool(forKey: "jet.notifications.offer-shown"))

        reloaded.forgetTrashedTitle(trashed)
        #expect(ClientMemory(defaults: defaults).trashedTitle(trashed) == nil)
    }

    @Test
    func seenSequencesOnlyMoveForward() {
        let memory = Self.isolatedMemory()
        let task = UUID()
        memory.markSeen(task, sequence: 10)
        memory.markSeen(task, sequence: 7)
        #expect(memory.seenSequence(task) == 10)
        memory.markSeen(task, sequence: 12)
        #expect(memory.seenSequence(task) == 12)
    }

    @Test
    func eachMemoryKeepsTheNewest512() {
        let (defaults, suite) = Self.isolatedDefaults()
        defer { defaults.removePersistentDomain(forName: suite) }
        let memory = ClientMemory(defaults: defaults)
        let ids = (0 ... ClientMemory.capacity).map { _ in UUID() }

        for (index, id) in ids.enumerated() {
            memory.recordAssistant("craft-\(index)", for: id)
            if index == 1 {
                // Writing the first entry again makes it the newest.
                memory.recordAssistant("craft-0-again", for: ids[0])
            }
        }

        #expect(ClientMemory.capacity == 512)
        #expect(memory.assistant(for: ids[0]) == "craft-0-again")
        #expect(memory.assistant(for: ids[1]) == nil)
        #expect(memory.assistant(for: ids[2]) == "craft-2")
        #expect(memory.assistant(for: ids[512]) == "craft-512")

        let reloaded = ClientMemory(defaults: defaults)
        #expect(reloaded.assistant(for: ids[1]) == nil)
        #expect(reloaded.assistant(for: ids[512]) == "craft-512")
        let stored = try? JSONSerialization.jsonObject(
            with: defaults.data(forKey: "jet.task-assistants.v1") ?? Data()
        ) as? [Any]
        #expect(stored?.count == 512)
    }

    @Test
    func storedValuesHoldOnlyIDsNumbersAndTitles() throws {
        let (defaults, suite) = Self.isolatedDefaults()
        defer { defaults.removePersistentDomain(forName: suite) }
        let memory = ClientMemory(defaults: defaults)
        let task = UUID()
        memory.markSeen(task, sequence: 5)

        let data = try #require(defaults.data(forKey: "jet.seen.v1"))
        let entries = try #require(try JSONSerialization.jsonObject(with: data) as? [[String: Any]])
        #expect(entries.count == 1)
        #expect(Set(entries[0].keys) == ["id", "value"])
        #expect((entries[0]["value"] as? NSNumber)?.uint64Value == 5)
    }

    static func isolatedDefaults() -> (UserDefaults, String) {
        let suite = "jet.tests.memory.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        return (defaults, suite)
    }

    static func isolatedMemory() -> ClientMemory {
        ClientMemory(defaults: isolatedDefaults().0)
    }
}
