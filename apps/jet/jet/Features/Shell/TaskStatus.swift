import Foundation

/// What a working assistant is doing, mapped from the latest tool name for
/// display only. The protocol has no structured phase.
enum TaskPhase: Equatable, Sendable {
    case editingFiles
    case runningCommand
    case readingProject
    case searchingWeb
    case working

    static func from(toolName: String) -> TaskPhase {
        switch toolName.lowercased() {
        case "edit", "write", "multiedit": .editingFiles
        case "bash": .runningCommand
        case "read", "grep", "glob", "ls": .readingProject
        case "webfetch", "websearch": .searchingWeb
        default: .working
        }
    }

    /// The tool name when an assistant entry is a bare tool identifier: one line of
    /// at most 40 characters matching `^[A-Za-z_][A-Za-z0-9_.:-]*$`.
    static func toolName(of entry: JetTimelineEntry) -> String? {
        guard entry.kind == .agent else { return nil }
        let text = entry.text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, text.count <= 40, let first = text.unicodeScalars.first else {
            return nil
        }
        func isLetter(_ scalar: Unicode.Scalar) -> Bool {
            ("a" ... "z").contains(scalar) || ("A" ... "Z").contains(scalar)
        }
        guard isLetter(first) || first == "_" else { return nil }
        for scalar in text.unicodeScalars.dropFirst() {
            let allowed = isLetter(scalar)
                || ("0" ... "9").contains(scalar)
                || scalar == "_" || scalar == "." || scalar == ":" || scalar == "-"
            guard allowed else { return nil }
        }
        return text
    }

    /// The phase of the most recent tool name in the transcript.
    static func latest(in entries: [JetTimelineEntry]) -> TaskPhase? {
        for entry in entries.reversed() {
            if let name = toolName(of: entry) { return from(toolName: name) }
        }
        return nil
    }
}

/// Everything the status derivation needs about one Conversation. Lifecycle and
/// activity stay separate inputs (ADR-0065).
struct TaskStatusFacts: Equatable, Sendable {
    var lifecycle: JetRunLifecycle?
    var activity: JetRunActivity?
    var runID: UUID?
    var hasRuns = false
    var hasRecordedChanges = false
    var gitUnconfirmed = false
    /// The newest Plane cursor these facts reflect; older snapshots are fenced out.
    var lastSequence: UInt64 = 0
    var lastReplySequence: UInt64?
    var lastInputSequence: UInt64?
}

/// The one status vocabulary shared by sidebar rows, the toolbar, Activity,
/// notifications and VoiceOver (design §8).
enum TaskStatus: Equatable, Sendable {
    case unknown
    case offline
    case starting
    case working(TaskPhase?)
    case reconnecting
    case stopping
    case needsPermission
    case needsSignIn
    case usageLimit
    case gitUnconfirmed
    case failed
    case waitingForReply
    case finished
    case stopped
    case notStarted

    /// Priority: offline, then Needs You, then in progress, then failed, then the rest.
    static func derive(
        facts: TaskStatusFacts?,
        isOffline: Bool,
        phase: TaskPhase?
    ) -> TaskStatus {
        if isOffline { return .offline }
        guard let facts else { return .unknown }

        let lifecycle = facts.lifecycle
        let acceptsInput = lifecycle == .created || lifecycle == .starting || lifecycle == .active
        if acceptsInput {
            switch facts.activity {
            case .waitingForApproval: return .needsPermission
            case .waitingForAuth: return .needsSignIn
            case .waitingForQuota: return .usageLimit
            default: break
            }
        }
        if facts.gitUnconfirmed { return .gitUnconfirmed }

        switch lifecycle {
        case .created, .starting:
            return .starting
        case .active:
            switch facts.activity {
            case .reconnecting: return .reconnecting
            case .waitingForUser: return .waitingForReply
            case .working, .none: return .working(phase)
            case .waitingForApproval, .waitingForAuth, .waitingForQuota: return .working(phase)
            }
        case .stopping:
            return .stopping
        case .failed:
            return .failed
        case .completed:
            return .finished
        case .canceled, .lost:
            // A reboot leaves `lost`: neutral, never red and never Needs You.
            return .stopped
        case .none:
            return facts.hasRuns ? .unknown : .notStarted
        }
    }

    var needsYou: Bool {
        switch self {
        case .needsPermission, .needsSignIn, .usageLimit, .gitUnconfirmed: true
        default: false
        }
    }

    var isInProgress: Bool {
        switch self {
        case .starting, .working, .reconnecting, .stopping: true
        default: false
        }
    }

    /// Rows show a status glyph for Needs You and for failures; calmer states show
    /// at most the unread dot.
    var showsAlertGlyph: Bool {
        needsYou || self == .failed
    }
}
