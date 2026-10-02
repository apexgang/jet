import Foundation

/// How a reply ended, as a status row shows it (design §6.5 "Status rows").
enum TranscriptStatusKind: String, Equatable, Sendable, CaseIterable {
    case interrupted
    case stopped
    case stopUnconfirmed = "stop-unconfirmed"
    case canceled
    case failed
    case lost

    /// The entry text; the row view adds its symbol and actions.
    var title: String {
        switch self {
        case .interrupted: String(localized: "Interrupted.")
        case .stopped, .canceled: String(localized: "Stopped. Send a message to continue.")
        case .stopUnconfirmed: String(localized: "Jet couldn't confirm that the assistant stopped.")
        case .failed: String(localized: "Stopped with an error.")
        case .lost: String(localized: "Stopped unexpectedly. Send a message to continue.")
        }
    }
}

/// What a transcript entry is, read from the id its projection gave it. Entries
/// from older caches or previews fall back to their kind.
enum TranscriptEntryRole: Equatable, Sendable {
    case message
    case markdown
    case text
    case approval
    case changes
    case status(TranscriptStatusKind)
    /// The Turn with this lowercased id became active.
    case delivered(String)
    /// The Turn with this lowercased id was withdrawn from the queue.
    case withdrawn(String)
    case technical

    static func of(_ entry: JetTimelineEntry) -> TranscriptEntryRole {
        let id = entry.id
        if id.hasPrefix("turn-active-") { return .delivered(String(id.dropFirst(12))) }
        if id.hasPrefix("turn-withdrawn-") { return .withdrawn(String(id.dropFirst(15))) }
        switch entry.kind {
        case .user: return .message
        case .approval: return entry.approval == nil ? .technical : .approval
        case .agent, .activity, .result: break
        }
        let parts = id.split(separator: "-", maxSplits: 2, omittingEmptySubsequences: false)
        if parts.count >= 2, UInt64(parts[0]) != nil {
            switch parts[1] {
            case "output": return .markdown
            case "text": return .text
            case "changes": return .changes
            case "status":
                if parts.count == 3, let kind = TranscriptStatusKind(rawValue: String(parts[2])) {
                    return .status(kind)
                }
                return .technical
            default: break
            }
        }
        if entry.isTechnical { return .technical }
        switch entry.kind {
        case .agent: return TaskPhase.toolName(of: entry) == nil ? .markdown : .text
        case .result: return entry.checkpointTurn == nil ? .technical : .changes
        case .user, .approval, .activity: return .technical
        }
    }
}

extension JetEvent {
    /// The transcript entries one Event contributes. Every entry carries the Event's
    /// time and Run; ids encode the role (`TranscriptEntryRole.of`).
    func timelineProjections() -> [JetTimelineEntry] {
        guard let payload = payloadObject else { return [] }

        switch kind {
        case "turn.input":
            guard let turnID = payload["turn_id"] as? String,
                  UUID(uuidString: turnID) != nil,
                  let text = payload["text"] as? String
            else {
                return []
            }
            return [entry(turnID.lowercased(), .user, bounded(text, maximumBytes: 8_192))]
        case "run.output":
            return outputProjections(payload)
        case "run.activity_changed":
            let activity = (payload["activity"] as? String)?
                .replacingOccurrences(of: "_", with: " ")
            return [technical("\(sequence)-activity", activity.map { "Run is \($0)." } ?? "Run activity paused.")]
        case "run.lifecycle_changed":
            let to = payload["to"] as? String
            let state = to?.replacingOccurrences(of: "_", with: " ") ?? "updated"
            var entries = [technical("\(sequence)-lifecycle", "Run \(state).")]
            let status: TranscriptStatusKind? = switch to.flatMap(JetRunLifecycle.init(rawValue:)) {
            case .failed: .failed
            case .lost: .lost
            case .canceled: .canceled
            default: nil
            }
            if let status { entries.append(statusEntry(status)) }
            return entries
        case "run.control_requested":
            let text = switch payload["control"] as? String {
            case JetRunControl.interruptTurn.rawValue: "Interrupt requested. Waiting for the active Turn to end."
            case JetRunControl.stopRun.rawValue: "Stop requested. Waiting for the Run to end."
            default: "Run control requested."
            }
            return [technical("\(sequence)-activity", text)]
        case "run.terminated":
            return terminationProjections(payload)
        case "approval.requested", "approval.reviewed":
            guard let approval = approvalPresentation(payload) else { return [] }
            let runKey = approval.runID?.uuidString.lowercased() ?? "run"
            var approvalEntry = entry(
                "approval-\(runKey)-\(approval.requestID)",
                .approval,
                approvalSummary(approval)
            )
            approvalEntry.approval = approval
            return [approvalEntry]
        case "approval.retry_authorized":
            return [technical("\(sequence)-retry", String(localized: "Authorized one more safety review."))]
        case "change.checkpoint_recorded":
            return checkpointProjections(payload)
        case "turn.changed":
            // Markers only: they place a delivered message and hide a withdrawn one.
            guard let turn = payload["turn"] as? [String: Any],
                  let turnID = turn["turn_id"] as? String,
                  UUID(uuidString: turnID) != nil
            else {
                return []
            }
            switch (turn["state"] as? String).flatMap(JetTurnState.init(rawValue:)) {
            case .active: return [technical("turn-active-\(turnID.lowercased())", "Turn became active.")]
            case .withdrawn: return [technical("turn-withdrawn-\(turnID.lowercased())", "Turn was withdrawn.")]
            default: return []
            }
        default:
            return []
        }
    }

    /// The event payload as a bounded JSON object. Shared with the notification projection.
    var payloadObject: [String: Any]? {
        guard let data = payload.source.data(using: .utf8),
              data.count <= 1_048_576,
              let object = try? JSONSerialization.jsonObject(with: data),
              let payload = object as? [String: Any]
        else {
            return nil
        }
        return payload
    }

    private func outputProjections(_ payload: [String: Any]) -> [JetTimelineEntry] {
        guard let rawBlocks = payload["presentation_json"] as? [String] else { return [] }
        return rawBlocks.prefix(32).enumerated().compactMap { index, raw in
            guard let data = raw.data(using: .utf8),
                  data.count <= 131_072,
                  let value = try? JSONSerialization.jsonObject(with: data),
                  let block = value as? [String: Any],
                  let kind = block["kind"] as? String
            else {
                return nil
            }
            switch kind {
            case "text", "markdown":
                guard let text = block["text"] as? String else { return nil }
                // ASVS 1.5.2 and 15.3.1: known blocks expose only inert, bounded
                // text. MessageText renders Markdown without links or images.
                let role = kind == "markdown" ? "output" : "text"
                return entry("\(sequence)-\(role)-\(index)", .agent, bounded(text, maximumBytes: 16_384))
            case "actions":
                guard let actions = block["actions"] as? [[String: Any]], actions.count <= 128 else {
                    return nil
                }
                return technical(
                    "\(sequence)-actions-\(index)",
                    "\(actions.count) Run action\(actions.count == 1 ? " is" : "s are") available."
                )
            default:
                return technical("\(sequence)-unknown-\(index)", "The Run published an additional presentation block.")
            }
        }
    }

    private func terminationProjections(_ payload: [String: Any]) -> [JetTimelineEntry] {
        let value = payload["termination"] as? [String: Any] ?? payload
        let termination = (value["control"] as? String)
            .flatMap(JetRunControl.init(rawValue:))
            .flatMap { control in
                (value["stage"] as? String)
                    .flatMap(JetTerminationStage.init(rawValue:))
                    .map { JetRunTermination(control: control, stage: $0) }
            }
        let summary = technical("\(sequence)-termination", termination?.summary ?? "The Run control request finished.")
        guard let termination else { return [summary] }
        let status: TranscriptStatusKind = switch (termination.control, termination.stage) {
        case (.interruptTurn, _): .interrupted
        case (.stopRun, .unobserved): .stopUnconfirmed
        case (.stopRun, _): .stopped
        }
        return [summary, statusEntry(status)]
    }

    /// Reads only the bounded `turn` number and the artifact size (ASVS 1.5.2).
    private func checkpointProjections(_ payload: [String: Any]) -> [JetTimelineEntry] {
        guard let number = payload["turn"] as? NSNumber,
              number.int64Value > 0, number.int64Value <= Int64(UInt32.max)
        else {
            return []
        }
        let turn = UInt32(number.int64Value)
        let size = ((payload["artifact"] as? [String: Any])?["size"] as? NSNumber)?.int64Value ?? 0
        guard size > 0 else {
            return [technical("\(sequence)-checkpoint", "Reply \(turn) recorded no file changes.")]
        }
        var changes = entry(
            "\(sequence)-changes",
            .result,
            String(localized: "The assistant changed files in the working copy.")
        )
        changes.checkpointTurn = turn
        return [changes]
    }

    private func entry(_ id: String, _ kind: JetTimelineKind, _ text: String) -> JetTimelineEntry {
        JetTimelineEntry(
            id: id,
            kind: kind,
            text: text,
            sequence: sequence,
            rawCount: 0,
            recordedAtUnixMilliseconds: recordedAtUnixMilliseconds,
            runID: runID
        )
    }

    /// Background detail shown only with View › Show Technical Activity. It may use
    /// Jet's own words.
    private func technical(_ id: String, _ text: String) -> JetTimelineEntry {
        var value = entry(id, .activity, text)
        value.isTechnical = true
        return value
    }

    private func statusEntry(_ status: TranscriptStatusKind) -> JetTimelineEntry {
        entry("\(sequence)-status-\(status.rawValue)", .result, status.title)
    }

    private func approvalPresentation(
        _ payload: [String: Any]
    ) -> JetApprovalPresentation? {
        let request: [String: Any]
        let reviewID: UUID?
        let state: JetApprovalState
        let canAuthorizeRetry: Bool
        let rationale: String?
        let consequence: String

        if kind == "approval.requested" {
            guard let value = payload["request"] as? [String: Any] else { return nil }
            request = value
            reviewID = nil
            state = .requested
            canAuthorizeRetry = false
            rationale = nil
            consequence = String(localized: "This reply stays paused until the request is answered.")
        } else {
            guard let review = payload["review"] as? [String: Any],
                  let value = review["request"] as? [String: Any],
                  let outcome = review["outcome"] as? [String: Any],
                  let status = outcome["status"] as? String
            else {
                return nil
            }
            request = value
            reviewID = (review["review_id"] as? String).flatMap(UUID.init(uuidString:))
            let decision = outcome["decision"] as? String
            switch (status, decision) {
            case ("decided", "allow"):
                state = .allowed
                consequence = String(localized: "The automatic safety review allowed this exact action once.")
            case ("denied", _), ("decided", "deny"):
                state = .denied
                consequence = String(localized: "The action stays blocked. You can ask the safety reviewer to check the unchanged request once more.")
            case ("unavailable", _):
                state = .unavailable
                consequence = String(localized: "The automatic safety review couldn't decide, so the request waits for you.")
            default:
                return nil
            }
            canAuthorizeRetry = state == .denied && reviewID != nil
            let verdict = outcome["verdict"] as? [String: Any]
            rationale = (verdict?["rationale"] as? String ?? outcome["reason"] as? String)
                .map { bounded($0, maximumBytes: 1_024) }
        }

        guard let requestID = request["request_id"] as? String,
              !requestID.isEmpty,
              requestID.utf8.count <= 128,
              !requestID.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains),
              let tool = request["tool"] as? String,
              !tool.isEmpty,
              tool.utf8.count <= 128,
              !tool.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains),
              let action = request["action"] as? String,
              action.utf8.count <= 4_096
        else {
            return nil
        }
        return JetApprovalPresentation(
            requestID: requestID,
            reviewID: reviewID,
            runID: runID,
            tool: tool,
            action: action,
            target: actionTarget(action),
            scope: String(localized: "This action once"),
            consequence: consequence,
            rationale: rationale,
            state: state,
            canAuthorizeRetry: canAuthorizeRetry
        )
    }

    private func approvalSummary(_ approval: JetApprovalPresentation) -> String {
        switch approval.state {
        case .allowed: String(localized: "\(approval.tool) was allowed once.")
        case .denied: String(localized: "\(approval.tool) was blocked.")
        case .unavailable, .requested: String(localized: "\(approval.tool) needs permission.")
        }
    }

    private func actionTarget(_ action: String) -> String {
        let fallback = String(localized: "This task's working copy")
        guard let data = action.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return fallback
        }
        for key in [
            "file_path", "filePath", "path", "directory", "cwd",
            "working_directory", "workingDirectory",
        ] {
            if let value = object[key] as? String {
                let flattened = value.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
                return flattened.isEmpty ? fallback : bounded(flattened, maximumBytes: 512)
            }
        }
        return fallback
    }

    private func bounded(_ value: String, maximumBytes: Int) -> String {
        guard value.utf8.count > maximumBytes else { return value }
        var result = ""
        var bytes = 0
        for character in value {
            let size = character.utf8.count
            if bytes + size > maximumBytes { break }
            result.append(character)
            bytes += size
        }
        return result + "\n\n" + String(localized: "[Jet shortened this output.]")
    }
}

/// Display-only views of an approval request. Nothing here is executed or used to
/// decide anything (ASVS 1.5.2): the command is extracted only to be read.
enum ApprovalDisplay {
    static let maximumCommandLength = 200

    /// The requested shell command from the action's JSON `command`: a string, or an
    /// argument list where `sh -c <script>` shows only the script.
    static func command(from action: String) -> String? {
        guard let data = action.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return nil
        }
        let raw: String
        if let value = object["command"] as? String {
            raw = value
        } else if let arguments = object["command"] as? [String] {
            let shells: Set<String> = ["sh", "bash", "zsh"]
            if arguments.count == 3,
               shells.contains(URL(fileURLWithPath: arguments[0]).lastPathComponent),
               arguments[1] == "-c" || arguments[1] == "-lc"
            {
                raw = arguments[2]
            } else {
                raw = arguments.joined(separator: " ")
            }
        } else {
            return nil
        }
        let collapsed = raw.split(whereSeparator: { $0.isWhitespace || $0.isNewline }).joined(separator: " ")
        guard !collapsed.isEmpty else { return nil }
        guard collapsed.count > maximumCommandLength else { return collapsed }
        return String(collapsed.prefix(maximumCommandLength)) + "…"
    }

    /// What a one-line summary names: the command, or else the tool.
    static func subject(_ approval: JetApprovalPresentation) -> String {
        command(from: approval.action) ?? approval.tool
    }
}
