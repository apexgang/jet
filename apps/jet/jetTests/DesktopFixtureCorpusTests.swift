import Foundation
import Testing
@testable import jet

struct DesktopFixtureCorpusTests {
    @Test("The shared corpus covers every required desktop state")
    func requiredStateCoverage() throws {
        let corpus = try DesktopFixtureCorpus(data: fixtureData())

        #expect(corpus.protocolVersion == FixtureProtocolVersion(major: 1, minor: 43))
        #expect(corpus.scenarios.count == DesktopFixtureState.allCases.count)
        #expect(corpus.scenario(for: .queued)?.queue.count == 2)
        #expect(corpus.scenario(for: .approval)?.contract.backendDependencies == [
            "approval_decision_command",
        ])
        #expect(corpus.scenario(for: .unsupported)?.primaryAction?.availability == .disabled)
    }

    @Test("Unknown fixture formats fail closed")
    func unknownFormatFailsClosed() {
        let data = Data(
            """
            {
              "format_version": 2,
              "protocol_version": { "major": 1, "minor": 43 },
              "scenarios": []
            }
            """.utf8
        )

        #expect(throws: DesktopFixtureCorpusError.unsupportedFormatVersion(2)) {
            try DesktopFixtureCorpus(data: data)
        }
    }

    private func fixtureData() throws -> Data {
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        return try Data(
            contentsOf: repositoryRoot
                .appending(path: "fixtures/desktop/presentation-v1.json")
        )
    }
}
