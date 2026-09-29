import Foundation
import Testing
@testable import jet

/// The Avoid-word lint (design §4): casual surfaces never need Jet's domain words.
/// It scans the user-facing string literals in the feature views (Text, Label,
/// Button, Toggle, Section, Picker, Menu, help, titles, accessibility text and
/// `String(localized:)`) for `JetCopy.avoidWords`, matched as case-sensitive whole
/// words. Technical Details, destructive reviews and Settings › Advanced may use
/// them; those places are listed in `allowed` with the reason.
@MainActor
struct CopyLintTests {
    /// One permitted use of an Avoid word.
    struct Allowance {
        /// The path under `jet/Features/`.
        let file: String
        /// A fragment of the literal, or nil for every literal in the file.
        let fragment: String?
        let reason: String
    }

    static let allowed: [Allowance] = [
        // Settings › Advanced names Crafts (design §4 lexicon: Harness / Craft).
        Allowance(file: "Settings/AdvancedSettingsPane.swift", fragment: nil, reason: "Settings › Advanced"),
        Allowance(file: "Recovery/JetSystemSections.swift", fragment: "Craft \\(craft.id)", reason: "Settings › Advanced service facts"),
        // Computers › Connection Details shows the Plane ID.
        Allowance(file: "Planes/PlaneManagementView.swift", fragment: "Plane", reason: "Connection Details"),
        // The Stop Assistant confirmation names the Stop Run it sends.
        Allowance(file: "Shell/ShellPresentations.swift", fragment: "(Stop Run)", reason: "Stop confirmation"),
        // Move to Jet Trash stops the assistant first: a destructive review.
        Allowance(file: "Library/MoveToTrashSheet.swift", fragment: "(Stop Run)", reason: "destructive review"),
        // "Run up to [N] tasks at once" is the verb, verbatim from design §6.12.
        Allowance(file: "Settings/SafetySettingsPane.swift", fragment: "Run up to", reason: "verb, design copy"),
    ]

    /// Views that show user-facing text, followed by a string literal. The literal
    /// may start on the next line (`String(localized:\n "…")`).
    static let literalPattern = try! NSRegularExpression(
        pattern: #"(?:\bText|\bLabel|\bButton|\bToggle|\bSection|\bPicker|\bMenu|\bLabeledContent|\bContentUnavailableView|\bTextField|\bStepper|\bDisclosureGroup|\bProgressView|\.help|\.navigationTitle|\.navigationSubtitle|\.accessibilityLabel|\.accessibilityHint|\.accessibilityValue|\.confirmationDialog|\.alert|String\(localized:)\s*\(?\s*(?:localized:\s*)?"((?:[^"\\\n]|\\.)*)""#
    )

    /// The words a literal uses, with interpolations removed and the verb phrases
    /// "Turn On" / "Turn Off" (a switch, not a Turn) left out.
    static func avoidWords(inLiteral literal: String) -> [String] {
        var text = literal.replacingOccurrences(of: #"\\\([^)]*\)"#, with: " ", options: .regularExpression)
        text = text.replacingOccurrences(of: #"\bTurn (On|Off|on|off)\b"#, with: " ", options: .regularExpression)
        return JetCopy.foundAvoidWords(in: text)
    }

    static func isAllowed(file: String, literal: String) -> Bool {
        allowed.contains { allowance in
            file.hasSuffix(allowance.file) && (allowance.fragment.map { literal.contains($0) } ?? true)
        }
    }

    /// `jet/Features`, found from this file's path, so the lint reads the sources
    /// the app was built from: `apps/jet/jet` next to `jetTests` in the Xcode
    /// project, or `Sources/jet` in a SwiftPM copy of it.
    static var featuresDirectory: URL {
        let tests = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
        let candidates = [
            tests.appendingPathComponent("jet/Features", isDirectory: true),
            tests.deletingLastPathComponent().appendingPathComponent("Sources/jet/Features", isDirectory: true),
        ]
        return candidates.first { FileManager.default.fileExists(atPath: $0.path) } ?? candidates[0]
    }

    struct Leak: CustomStringConvertible {
        let file: String
        let line: Int
        let literal: String
        let words: [String]
        var description: String { "\(file):\(line) \(words) “\(literal)”" }
    }

    static func scan(_ directory: URL) throws -> (literals: Int, leaks: [Leak]) {
        let files = FileManager.default.enumerator(at: directory, includingPropertiesForKeys: nil)?
            .compactMap { $0 as? URL }
            .filter { $0.pathExtension == "swift" } ?? []
        var literals = 0
        var leaks: [Leak] = []
        for url in files {
            let source = try String(contentsOf: url, encoding: .utf8)
            let relative = String(url.path.dropFirst(directory.path.count + 1))
            let range = NSRange(source.startIndex..., in: source)
            for match in literalPattern.matches(in: source, range: range) {
                guard let literalRange = Range(match.range(at: 1), in: source) else { continue }
                literals += 1
                let literal = String(source[literalRange])
                let words = avoidWords(inLiteral: literal)
                guard !words.isEmpty, !isAllowed(file: relative, literal: literal) else { continue }
                let line = source[..<literalRange.lowerBound].filter { $0 == "\n" }.count + 1
                leaks.append(Leak(file: relative, line: line, literal: literal, words: words))
            }
        }
        return (literals, leaks.sorted { ($0.file, $0.line) < ($1.file, $1.line) })
    }

    @Test
    func featureViewsKeepDomainWordsOutOfCasualCopy() throws {
        let result = try Self.scan(Self.featuresDirectory)
        // Guards against a path or pattern change that silently scans nothing.
        #expect(result.literals > 500)
        #expect(result.leaks.isEmpty, "Avoid words in casual copy:\n\(result.leaks.map(\.description).joined(separator: "\n"))")
    }

    @Test
    func theLintFindsLeaksAndSkipsVerbsAndInterpolations() {
        #expect(Self.avoidWords(inLiteral: "Start a new Run") == ["Run"])
        #expect(Self.avoidWords(inLiteral: "Workspace unavailable") == ["Workspace"])
        #expect(Self.avoidWords(inLiteral: "Turn On Clean Up").isEmpty)
        #expect(Self.avoidWords(inLiteral: "Turn off clean up?").isEmpty)
        #expect(Self.avoidWords(inLiteral: "Last run ended \\(day(ended))").isEmpty)
        #expect(Self.avoidWords(inLiteral: "Paired \\(run.revision)").isEmpty)
        #expect(Self.isAllowed(file: "Settings/AdvancedSettingsPane.swift", literal: "Craft"))
        #expect(!Self.isAllowed(file: "Settings/GeneralSettingsPane.swift", literal: "Craft"))
    }

    @Test
    func fixedCasualCopyListsAvoidDomainWords() {
        var strings = LibraryCopy.casual + DetailsCopy.casualStrings
        for kind in JetNotificationKind.allCases {
            strings += [kind.title, kind.body, kind.preferenceTitle]
        }
        let leaks = strings.filter { !Self.avoidWords(inLiteral: $0).isEmpty }
        #expect(leaks.isEmpty, "Avoid words: \(leaks)")
    }
}
