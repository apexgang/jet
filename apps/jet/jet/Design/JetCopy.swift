import Foundation

/// Shared copy helpers for casual surfaces (design §4 lexicon).
enum JetCopy {
    /// The locale for dates, times and numbers: the language the app's interface
    /// is shown in, with the person's region (so a Russian-region Mac showing the
    /// English interface reads "25 min. ago", with its own hour cycle).
    /// `Locale.current` would take the system language instead.
    nonisolated static var uiLocale: Locale {
        uiLocale(
            preferredLocalization: Bundle.main.preferredLocalizations.first,
            region: Locale.autoupdatingCurrent.region
        )
    }

    nonisolated static func uiLocale(preferredLocalization: String?, region: Locale.Region?) -> Locale {
        var language = preferredLocalization ?? "en"
        if language.isEmpty || language == "Base" { language = "en" }
        var components = Locale.Components(identifier: language)
        // "en-GB" carries its own region; a bare "en" takes the person's.
        if components.languageComponents.region == nil, components.region == nil, let region {
            components.region = region
        }
        return Locale(components: components)
    }

    /// A whole number in the interface's locale, such as "4,096".
    nonisolated static func number<Value: BinaryInteger>(_ value: Value) -> String {
        value.formatted(IntegerFormatStyle<Value>(locale: uiLocale))
    }

    /// A file size such as "12.4 MB", in the interface's locale.
    nonisolated static func byteCount(_ bytes: Int64) -> String {
        bytes.formatted(.byteCount(style: .file).locale(uiLocale))
    }

    /// A short date such as "Sep 18", in the interface's locale.
    nonisolated static func shortDate(_ date: Date) -> String {
        date.formatted(.dateTime.month(.abbreviated).day().locale(uiLocale))
    }

    /// A date and time such as "Sep 18, 2026 at 14:05", in the interface's locale.
    nonisolated static func dateTime(_ date: Date) -> String {
        date.formatted(Date.FormatStyle(date: .abbreviated, time: .shortened, locale: uiLocale))
    }

    /// The assistant's product name for a Craft ID, such as "Claude Code" or "Codex".
    static func assistantName(craftID: String, crafts: [JetInstalledCraft] = []) -> String {
        DesktopSession.assistantLabel(craftID: craftID, crafts: crafts)
    }

    /// An ordinal such as "1st" or "2nd", for "Waiting to send · 2nd".
    static func ordinal(_ value: Int, locale: Locale = JetCopy.uiLocale) -> String {
        let formatter = NumberFormatter()
        formatter.locale = locale
        formatter.numberStyle = .ordinal
        return formatter.string(from: NSNumber(value: value)) ?? number(value)
    }

    /// When something happened, from Unix milliseconds: "now", "25 min. ago",
    /// "yesterday" within a week, then a date such as "Sep 18". Future times
    /// (clock skew between computers) read as "now".
    static func relative(ms: Int64, now: Date = .now, locale: Locale = JetCopy.uiLocale) -> String {
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
