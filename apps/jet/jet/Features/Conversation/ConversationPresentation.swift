import Foundation

extension JetEvent {
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
        case "change.checkpoint_recorded":
            return [
                JetTimelineEntry(
                    id: "\(sequence)-checkpoint",
                    kind: .result,
                    text: "Jet recorded the completed turn and its changes.",
                    sequence: sequence,
                    rawCount: 0
                ),
            ]
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
