import Foundation

/// Shared copy helpers for casual surfaces (design §4 lexicon).
enum JetCopy {
    /// The assistant's product name for a Craft ID, such as "Claude Code" or "Codex".
    static func assistantName(craftID: String, crafts: [JetInstalledCraft] = []) -> String {
        DesktopSession.assistantLabel(craftID: craftID, crafts: crafts)
    }

    /// An ordinal such as "1st" or "2nd", for "Waiting to send · 2nd".
    static func ordinal(_ value: Int, locale: Locale = .autoupdatingCurrent) -> String {
        let formatter = NumberFormatter()
        formatter.locale = locale
        formatter.numberStyle = .ordinal
        return formatter.string(from: NSNumber(value: value)) ?? value.formatted()
    }

    /// When something happened, from Unix milliseconds: "now", "25 min. ago",
    /// "yesterday" within a week, then a date such as "Sep 18". Future times
    /// (clock skew between computers) read as "now".
    static func relative(ms: Int64, now: Date = .now, locale: Locale = .autoupdatingCurrent) -> String {
        let date = min(Date(timeIntervalSince1970: TimeInterval(ms) / 1_000), now)
        let age = now.timeIntervalSince(date)
        if age < 7 * 24 * 60 * 60 {
            let formatter = RelativeDateTimeFormatter()
            formatter.locale = locale
            formatter.dateTimeStyle = .named
            formatter.unitsStyle = .short
            return formatter.localizedString(for: age < 60 ? now : date, relativeTo: now)
        }
        var calendar = Calendar.autoupdatingCurrent
        calendar.locale = locale
        var style = Date.FormatStyle(locale: locale, calendar: calendar).month(.abbreviated).day()
        if calendar.component(.year, from: date) != calendar.component(.year, from: now) {
            style = style.year()
        }
        return date.formatted(style)
    }

    /// Jet domain words that casual surfaces never need (design §4). They may appear
    /// only in Technical Details, tooltips and destructive reviews. Matched as
    /// case-sensitive whole words by the copy lint.
    static let avoidWords: [String] = [
        "Plane", "Harness", "Craft", "Run", "Turn", "Workspace", "Visa", "Effect",
        "checkpoint", "revision", "cursor", "outbox",
    ]

    /// The avoid words that appear in `text` as case-sensitive whole words.
    static func foundAvoidWords(in text: String) -> [String] {
        let words = Set(
            text.split(whereSeparator: { !$0.isLetter }).map(String.init)
        )
        return JetCopy.avoidWords.filter { words.contains($0) }
    }
}
