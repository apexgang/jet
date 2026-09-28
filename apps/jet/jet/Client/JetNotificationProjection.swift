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
}
