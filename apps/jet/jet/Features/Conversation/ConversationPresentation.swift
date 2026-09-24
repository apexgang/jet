import Foundation

extension JetEvent {
    func notificationKind() -> JetNotificationKind? {
        switch kind {
        case "approval.requested":
            return .approval
        case "run.lifecycle_changed":
            guard let state = payloadObject?["to"] as? String else { return nil }
            switch state {
            case "completed": return .completion
            case "failed", "lost": return .failure
            default: return nil
            }
        default:
            return nil
        }
    }

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
            return [
                JetTimelineEntry(
                    id: turnID.lowercased(),
                    kind: .user,
                    text: bounded(text, maximumBytes: 8_192),
                    sequence: sequence,
                    rawCount: 0
                ),
            ]
        case "run.output":
            return outputProjections(payload)
        case "run.activity_changed":
            let activity = (payload["activity"] as? String)?
                .replacingOccurrences(of: "_", with: " ")
            return [activityEntry(activity.map { "Run is \($0)." } ?? "Run activity paused.")]
        case "run.lifecycle_changed":
            let state = (payload["to"] as? String)?
                .replacingOccurrences(of: "_", with: " ") ?? "updated"
            return [activityEntry("Run \(state).")]
        case "run.control_requested":
            switch payload["control"] as? String {
            case JetRunControl.interruptTurn.rawValue:
                return [activityEntry("Interrupt requested. Waiting for the active Turn to end.")]
            case JetRunControl.stopRun.rawValue:
                return [activityEntry("Stop requested. Waiting for the Run to end.")]
            default:
                return [activityEntry("Run control requested.")]
            }
        case "run.terminated":
            return [terminationEntry(payload)]
        case "approval.requested", "approval.reviewed":
            guard let approval = approvalPresentation(payload) else { return [] }
            let runKey = approval.runID?.uuidString.lowercased() ?? "run"
            return [
                JetTimelineEntry(
                    id: "approval-\(runKey)-\(approval.requestID)",
                    kind: .approval,
                    text: approvalSummary(approval),
                    sequence: sequence,
                    rawCount: 0,
                    approval: approval
                ),
            ]
        case "approval.retry_authorized":
            return [resultEntry("One exact approval retry was authorized.")]
        case "change.checkpoint_recorded":
            return [resultEntry("Jet recorded the completed turn and its changes.")]
        default:
            return []
        }
    }

    private var payloadObject: [String: Any]? {
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
                // ASVS 1.5.2 and 15.3.1: known blocks expose only inert,
                // bounded text. Markdown remains plain text in this slice.
                return JetTimelineEntry(
                    id: "\(sequence)-output-\(index)",
                    kind: .agent,
                    text: bounded(text, maximumBytes: 16_384),
                    sequence: sequence,
                    rawCount: 0
                )
            case "actions":
                guard let actions = block["actions"] as? [[String: Any]], actions.count <= 128 else {
                    return nil
                }
                return JetTimelineEntry(
                    id: "\(sequence)-actions-\(index)",
                    kind: .activity,
                    text: "\(actions.count) Run action\(actions.count == 1 ? " is" : "s are") available.",
                    sequence: sequence,
                    rawCount: 0
                )
            default:
                return JetTimelineEntry(
                    id: "\(sequence)-unknown-\(index)",
                    kind: .activity,
                    text: "The Run published an additional presentation block.",
                    sequence: sequence,
                    rawCount: 0
                )
            }
        }
    }

    private func activityEntry(_ text: String) -> JetTimelineEntry {
        JetTimelineEntry(
            id: "\(sequence)-activity",
            kind: .activity,
            text: text,
            sequence: sequence,
            rawCount: 0
        )
    }

    private func resultEntry(_ text: String) -> JetTimelineEntry {
        JetTimelineEntry(
            id: "\(sequence)-result",
            kind: .result,
            text: text,
            sequence: sequence,
            rawCount: 0
        )
    }

    private func terminationEntry(_ payload: [String: Any]) -> JetTimelineEntry {
        let value = payload["termination"] as? [String: Any] ?? payload
        let termination = (value["control"] as? String)
            .flatMap(JetRunControl.init(rawValue:))
            .flatMap { control in
                (value["stage"] as? String)
                    .flatMap(JetTerminationStage.init(rawValue:))
                    .map { JetRunTermination(control: control, stage: $0) }
            }
        return resultEntry(termination?.summary ?? "The Run control request finished.")
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
            consequence = "The Run stays paused until this request is decided. Manual approval decisions are not available through the current client protocol."
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
                consequence = "The reviewer allowed this exact action once."
            case ("denied", _), ("decided", "deny"):
                state = .denied
                consequence = "The action remains blocked. You may authorize one review retry of the unchanged request."
            case ("unavailable", _):
                state = .unavailable
                consequence = "Automatic review could not decide. The Run remains paused for a person."
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
            scope: "This action once",
            consequence: consequence,
            rationale: rationale,
            state: state,
            canAuthorizeRetry: canAuthorizeRetry
        )
    }

    private func approvalSummary(_ approval: JetApprovalPresentation) -> String {
        switch approval.state {
        case .allowed: "\(approval.tool) was allowed once."
        case .denied: "\(approval.tool) was denied."
        case .unavailable: "\(approval.tool) still needs a decision."
        case .requested: "\(approval.tool) needs approval."
        }
    }

    private func actionTarget(_ action: String) -> String {
        guard let data = action.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return "Current Run"
        }
        for key in [
            "file_path", "filePath", "path", "directory", "cwd",
            "working_directory", "workingDirectory",
        ] {
            if let value = object[key] as? String {
                let flattened = value.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
                return flattened.isEmpty
                    ? "Current Run"
                    : bounded(flattened, maximumBytes: 512)
            }
        }
        return "Current Run"
    }

    private func bounded(_ value: String, maximumBytes: Int) -> String {
        guard value.utf8.count > maximumBytes else { return value }
        var result = ""
        result.reserveCapacity(maximumBytes)
        for character in value {
            let candidate = result + String(character)
            if candidate.utf8.count > maximumBytes { break }
            result = candidate
        }
        return result + "\n\n[Output truncated by the desktop client.]"
    }
}
