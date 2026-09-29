import Foundation

/// What one Event says about its task's status (design §8 sources). The task
/// status store and the notification router both read Events through this, so
/// they agree on what an Event means.
enum JetEventStatusSignal: Equatable, Sendable {
    /// A new Run started for the task.
    case runCreated
    case lifecycle(from: JetRunLifecycle?, to: JetRunLifecycle)
    /// The Run's activity; nil when it stopped reporting one.
    case activity(JetRunActivity?)
    /// Output arrived: a reply the person reads, a tool the assistant used, or both.
    case output(hasReply: Bool, toolName: String?)
    /// The person's message reached the task.
    case input
    case changesRecorded
    case approvalRequested
    case trashed

    /// The notification this signal can raise (design §6.13). A request that
    /// Jet's automatic review decides quickly still raises `.approval` here; the
    /// router holds it briefly and drops it when the task moves on.
    var notificationKind: JetNotificationKind? {
        switch self {
        case .approvalRequested, .activity(.waitingForAuth), .activity(.waitingForQuota):
            // `waiting_for_approval` follows `approval.requested`, which already covers it.
            .approval
        case .activity(.waitingForUser), .lifecycle(_, .completed):
            .completion
        case .lifecycle(_, .failed), .lifecycle(_, .lost):
            .failure
        default:
            nil
        }
    }
}

extension JetEvent {
    /// Payloads larger than this are not read for status.
    static let maximumStatusPayloadBytes = 1_048_576
    /// Only the first blocks of an output Event are read, each within this size.
    static let maximumStatusBlocks = 32
    static let maximumStatusBlockBytes = 131_072

    /// The Event's meaning for task status, or nil when it has none or its
    /// payload can't be read.
    var statusSignal: JetEventStatusSignal? {
        switch kind {
        case "run.created":
            return runID == nil ? nil : .runCreated
        case "run.lifecycle_changed":
            guard let payload = signalPayload,
                  let to = (payload["to"] as? String).flatMap(JetRunLifecycle.init(rawValue:))
            else { return nil }
            let from = (payload["from"] as? String).flatMap(JetRunLifecycle.init(rawValue:))
            return .lifecycle(from: from, to: to)
        case "run.activity_changed":
            guard let payload = signalPayload else { return nil }
            switch payload["activity"] {
            case nil, is NSNull:
                return .activity(nil)
            case let raw as String:
                // An activity this version doesn't know is ignored, not guessed.
                return JetRunActivity(rawValue: raw).map { .activity($0) }
            default:
                return nil
            }
        case "run.output":
            return outputSignal()
        case "turn.input":
            return .input
        case "change.checkpoint_recorded":
            return .changesRecorded
        case "approval.requested":
            return .approvalRequested
        case "conversation.trashed":
            return .trashed
        default:
            return nil
        }
    }

    func notificationKind() -> JetNotificationKind? {
        statusSignal?.notificationKind
    }

    /// Markdown blocks are the reply; a text block that is a bare tool name says
    /// what the assistant is doing. Other text (thinking) says neither.
    private func outputSignal() -> JetEventStatusSignal? {
        guard let blocks = signalPayload?["presentation_json"] as? [String] else { return nil }
        var hasReply = false
        var toolName: String?
        for raw in blocks.prefix(Self.maximumStatusBlocks) {
            guard raw.utf8.count <= Self.maximumStatusBlockBytes,
                  let block = Self.jsonObject(raw),
                  let kind = block["kind"] as? String,
                  let text = block["text"] as? String
            else { continue }
            switch kind {
            case "markdown":
                hasReply = true
            case "text":
                let entry = JetTimelineEntry(id: "", kind: .agent, text: text, sequence: nil, rawCount: 0)
                if let name = TaskPhase.toolName(of: entry) { toolName = name }
            default:
                continue
            }
        }
        guard hasReply || toolName != nil else { return nil }
        return .output(hasReply: hasReply, toolName: toolName)
    }

    /// The payload as a bounded JSON object, read only for status.
    private var signalPayload: [String: Any]? {
        guard payload.source.utf8.count <= Self.maximumStatusPayloadBytes else { return nil }
        return Self.jsonObject(payload.source)
    }

    private static func jsonObject(_ source: String) -> [String: Any]? {
        (try? JSONSerialization.jsonObject(with: Data(source.utf8))) as? [String: Any]
    }
}
