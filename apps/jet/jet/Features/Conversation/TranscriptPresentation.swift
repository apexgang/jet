import Foundation

// MARK: - Rows

/// A waiting message's place in the queue.
struct QueuePlacement: Equatable {
    let ordinal: Int
    let entry: JetTurnQueueEntry
}

/// Everything besides the entries that decides how the transcript reads.
struct TranscriptContext: Equatable {
    var assistantName: String?
    var showsTechnical = false
    var isReplyInProgress = false
    var needsPermission = false
    /// Waiting "You" entries by their timeline id.
    var queued: [String: QueuePlacement] = [:]
}

enum TranscriptStep: Equatable {
    case tool(String)
    case reasoning(String)
}

struct YouRow: Equatable {
    let id: String
    let text: String
    let recordedAt: Int64?
    let queue: QueuePlacement?
}

struct AssistantRow: Equatable {
    let id: String
    let text: String
    let recordedAt: Int64?
    var showsLabel = true
}

struct StepsRow: Equatable {
    let id: String
    var items: [TranscriptStep]
    var recordedAt: Int64?
    var phase: TaskPhase?
    var isLive = false
    var showsLabel = true

    /// The tool names in order, one per use.
    var tools: [String] {
        items.compactMap { item in
            if case let .tool(name) = item { return name }
            return nil
        }
    }

    var lastTool: String? { tools.last }

    /// "Working… · Editing files" while live, "Used 3 tools · Read, Edit, Bash" after.
    var title: String {
        if isLive {
            if let phase, phase != .working { return String(localized: "Working… · \(phase.title)") }
            return String(localized: "Working…")
        }
        guard let first = tools.first else { return String(localized: "Reasoning") }
        if tools.count == 1 { return String(localized: "Used 1 tool · \(first)") }
        var unique: [String] = []
        for name in tools where !unique.contains(name) { unique.append(name) }
        var names = unique.prefix(3).joined(separator: ", ")
        if unique.count > 3 { names += ", +\(unique.count - 3)" }
        return String(localized: "Used \(tools.count) tools · \(names)")
    }
}

struct ChangesRow: Equatable {
    let id: String
    let runID: UUID?
    let turn: UInt32
    var offersKeep = false
}

struct StatusRow: Equatable {
    let id: String
    let kind: TranscriptStatusKind
    let runID: UUID?
    var sendAgainText: String?
}

struct PermissionRow: Equatable {
    enum Presentation: Equatable {
        /// The latest request while the reply waits for it: the full card.
        case pending
        /// An earlier or no longer waiting request.
        case asked
        case allowed
        case blocked(offersRetry: Bool)
    }

    let id: String
    let approval: JetApprovalPresentation
    let subject: String
    let command: String?
    var presentation: Presentation
}

enum TranscriptRow: Identifiable, Equatable {
    case you(YouRow)
    case assistant(AssistantRow)
    case steps(StepsRow)
    case changes(ChangesRow)
    case status(StatusRow)
    case permission(PermissionRow)
    case technical(id: String, text: String)

    var id: String {
        switch self {
        case let .you(row): row.id
        case let .assistant(row): row.id
        case let .steps(row): row.id
        case let .changes(row): row.id
        case let .status(row): row.id
        case let .permission(row): row.id
        case let .technical(id, _): id
        }
    }
}

/// What sits above the rows while the task itself loads (history has its own row).
enum TranscriptTop: Equatable {
    case none
    case loadingTask
    case couldNotLoad
}

enum TranscriptPresentation {
    static func rows(from entries: [JetTimelineEntry], context: TranscriptContext) -> [TranscriptRow] {
        // 1. The first entry per id wins.
        var seen = Set<String>()
        let unique = entries.filter { seen.insert($0.id).inserted }
        let roles = unique.map(TranscriptEntryRole.of)

        var withdrawn = Set<String>()
        var deliveredAt: [String: Int] = [:]
        for (index, role) in roles.enumerated() {
            switch role {
            case let .withdrawn(turnID): withdrawn.insert(turnID)
            case let .delivered(turnID): deliveredAt[turnID] = deliveredAt[turnID] ?? index
            default: break
            }
        }

        // 2–4. Hide withdrawn messages, move waiting ones to the tail and delivered
        // ones to their marker, so a follow-up never lands inside the previous reply.
        var ordered: [(JetTimelineEntry, TranscriptEntryRole)] = []
        var heldForMarker: [String: JetTimelineEntry] = [:]
        var waiting: [(JetTimelineEntry, QueuePlacement)] = []
        for (index, entry) in unique.enumerated() {
            let role = roles[index]
            switch role {
            case .message:
                let turnID = entry.id.lowercased()
                if withdrawn.contains(turnID) { continue }
                if let placement = context.queued[entry.id] {
                    waiting.append((entry, placement))
                } else if let marker = deliveredAt[turnID], marker > index {
                    heldForMarker[turnID] = entry
                } else {
                    ordered.append((entry, role))
                }
            case let .delivered(turnID):
                if let held = heldForMarker.removeValue(forKey: turnID) { ordered.append((held, .message)) }
            case .withdrawn:
                continue
            default:
                ordered.append((entry, role))
            }
        }

        // 5–6. Map entries to rows, folding consecutive steps.
        var rows: [TranscriptRow] = []
        for (entry, role) in ordered {
            switch role {
            case .message:
                rows.append(.you(YouRow(id: entry.id, text: entry.text, recordedAt: entry.recordedAtUnixMilliseconds, queue: nil)))
            case .markdown:
                rows.append(.assistant(AssistantRow(id: entry.id, text: entry.text, recordedAt: entry.recordedAtUnixMilliseconds)))
            case .text:
                let step: TranscriptStep = TaskPhase.toolName(of: entry).map { .tool($0) } ?? .reasoning(entry.text)
                if case var .steps(fold) = rows.last {
                    fold.items.append(step)
                    rows[rows.count - 1] = .steps(fold)
                } else {
                    rows.append(.steps(StepsRow(id: "steps-\(entry.id)", items: [step], recordedAt: entry.recordedAtUnixMilliseconds)))
                }
            case .approval:
                guard let approval = entry.approval else { continue }
                rows.append(.permission(PermissionRow(
                    id: entry.id,
                    approval: approval,
                    subject: ApprovalDisplay.subject(approval),
                    command: ApprovalDisplay.command(from: approval.action),
                    presentation: .asked
                )))
            case .changes:
                rows.append(.changes(ChangesRow(id: entry.id, runID: entry.runID, turn: entry.checkpointTurn ?? 1)))
            case let .status(kind):
                rows.append(.status(StatusRow(id: entry.id, kind: kind, runID: entry.runID)))
            case .technical:
                // 13. Hidden technical entries don't break a fold.
                if context.showsTechnical { rows.append(.technical(id: entry.id, text: entry.text)) }
            case .delivered, .withdrawn:
                continue
            }
        }

        // 8. A stop that says more than "canceled" replaces it.
        let explained = Set(rows.compactMap { row -> UUID?? in
            guard case let .status(status) = row,
                  [.interrupted, .stopped, .stopUnconfirmed].contains(status.kind)
            else { return nil }
            return .some(status.runID)
        })
        rows.removeAll { row in
            guard case let .status(status) = row, status.kind == .canceled else { return false }
            return explained.contains(status.runID)
        }

        // 7. The trailing fold is live while a reply runs.
        if let last = rows.lastIndex(where: { if case .technical = $0 { false } else { true } }),
           case var .steps(fold) = rows[last]
        {
            fold.isLive = context.isReplyInProgress
            rows[last] = .steps(fold)
        }
        for index in rows.indices {
            guard case var .steps(fold) = rows[index] else { continue }
            fold.phase = fold.lastTool.map(TaskPhase.from(toolName:))
            rows[index] = .steps(fold)
        }

        // 9. Keep Changes… only on the newest changes row, and never mid-reply.
        if !context.isReplyInProgress,
           let last = rows.lastIndex(where: { if case .changes = $0 { true } else { false } }),
           case var .changes(changes) = rows[last]
        {
            changes.offersKeep = true
            rows[last] = .changes(changes)
        }

        // 10. Send Again on the newest failure, while nothing followed it.
        if !context.isReplyInProgress,
           let last = rows.lastIndex(where: { if case let .status(s) = $0 { s.kind == .failed } else { false } }),
           case var .status(failure) = rows[last]
        {
            let followed = rows[(last + 1)...].contains { row in
                switch row {
                case .you, .assistant: true
                default: false
                }
            }
            let earlierText = rows[..<last].reversed().lazy.compactMap { row -> String? in
                if case let .you(you) = row { return you.text }
                return nil
            }.first
            if !followed, let earlierText {
                failure.sendAgainText = earlierText
                rows[last] = .status(failure)
            }
        }

        // 11. Only the latest request can still be pending or retried.
        let lastApproval = rows.lastIndex { if case .permission = $0 { true } else { false } }
        for index in rows.indices {
            guard case var .permission(permission) = rows[index] else { continue }
            let isLast = index == lastApproval
            switch permission.approval.state {
            case .allowed:
                permission.presentation = .allowed
            case .denied:
                permission.presentation = .blocked(offersRetry: permission.approval.canAuthorizeRetry
                    && isLast && (context.isReplyInProgress || context.needsPermission))
            case .requested, .unavailable:
                permission.presentation = isLast && context.needsPermission ? .pending : .asked
            }
            rows[index] = .permission(permission)
        }

        // 3. Waiting messages go last, in queue order.
        for (entry, placement) in waiting.sorted(by: { $0.1.ordinal < $1.1.ordinal }) {
            rows.append(.you(YouRow(id: entry.id, text: entry.text, recordedAt: entry.recordedAtUnixMilliseconds, queue: placement)))
        }

        // 12. The assistant is named once per stretch of its own rows.
        var previous: TranscriptRow?
        for index in rows.indices {
            let continues: Bool = switch previous {
            case .assistant?, .steps?, .permission?: true
            default: false
            }
            switch rows[index] {
            case var .assistant(row):
                row.showsLabel = !continues
                rows[index] = .assistant(row)
            case var .steps(row):
                row.showsLabel = !continues
                rows[index] = .steps(row)
            default:
                break
            }
            if case .technical = rows[index] { continue }
            previous = rows[index]
        }
        return rows
    }

    /// The snapshot-level state above the rows. History states come from the
    /// transcript's history notice instead.
    static func top(rowsAreEmpty: Bool, freshness: JetConversationFreshness, hasSnapshot: Bool) -> TranscriptTop {
        guard rowsAreEmpty, !hasSnapshot else { return .none }
        return freshness == .failed ? .couldNotLoad : .loadingTask
    }
}

// MARK: - Times

enum TranscriptFormat {
    /// "14:32" on the same day, otherwise "Sep 26, 14:32" (with the year when it differs).
    static func time(
        _ ms: Int64,
        now: Date = .now,
        calendar: Calendar = .autoupdatingCurrent,
        locale: Locale = .autoupdatingCurrent
    ) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(ms) / 1_000)
        var style = Date.FormatStyle(locale: locale, calendar: calendar, timeZone: calendar.timeZone)
            .hour().minute()
        if !calendar.isDate(date, inSameDayAs: now) {
            style = style.month(.abbreviated).day()
            if calendar.component(.year, from: date) != calendar.component(.year, from: now) {
                style = style.year()
            }
        }
        return date.formatted(style)
    }

    /// "Sep 26", with the year when it differs from now's.
    static func day(
        _ ms: Int64,
        now: Date = .now,
        calendar: Calendar = .autoupdatingCurrent,
        locale: Locale = .autoupdatingCurrent
    ) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(ms) / 1_000)
        var style = Date.FormatStyle(locale: locale, calendar: calendar, timeZone: calendar.timeZone)
            .month(.abbreviated).day()
        if calendar.component(.year, from: date) != calendar.component(.year, from: now) {
            style = style.year()
        }
        return date.formatted(style)
    }
}

// MARK: - Following the latest output

/// One reading of the transcript's scroll geometry, in SwiftUI's terms: the
/// container excludes the insets (toolbar above, composer below), and the offset
/// is `-topInset` at the top, so the visible content ends at
/// `offsetY + topInset + containerHeight`.
struct ScrollSample: Equatable {
    var offsetY: CGFloat
    var contentHeight: CGFloat
    var containerHeight: CGFloat
    var topInset: CGFloat

    var isAtBottom: Bool {
        offsetY + topInset + containerHeight >= contentHeight - 32 || contentHeight <= containerHeight
    }
}

/// Whether new output scrolls into view. It follows only while the transcript is at
/// the bottom; scrolling away stops it and Jump to Latest resumes it.
struct TranscriptFollow: Equatable {
    var followsLatest = true

    mutating func observe(previous: ScrollSample?, current: ScrollSample, userIsScrolling: Bool) {
        if current.isAtBottom {
            followsLatest = true
        } else if userIsScrolling {
            followsLatest = false
        } else if let previous,
                  previous.contentHeight == current.contentHeight,
                  previous.containerHeight == current.containerHeight,
                  current.offsetY < previous.offsetY - 1
        {
            // A wheel scroll may not report a scroll phase; the offset still drops.
            // A resize moves the offset too, so it only counts at an unchanged size.
            followsLatest = false
        }
    }

    mutating func jumpToLatest() {
        followsLatest = true
    }
}
