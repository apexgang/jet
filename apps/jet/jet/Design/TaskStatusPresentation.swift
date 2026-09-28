import SwiftUI

/// Copy, symbols and tints for the one status vocabulary (design §8). Rows, the
/// toolbar, Activity, notifications and VoiceOver all read these values.
extension TaskPhase {
    /// What the assistant is doing, as the second half of "Working · Editing files".
    var title: String {
        switch self {
        case .editingFiles: String(localized: "Editing files")
        case .runningCommand: String(localized: "Running a command")
        case .readingProject: String(localized: "Reading the project")
        case .searchingWeb: String(localized: "Searching the web")
        case .working: String(localized: "Working")
        }
    }

    /// The phase inside a spoken sentence, such as "Working, editing files".
    var accessibilityDescription: String {
        switch self {
        case .editingFiles: String(localized: "editing files")
        case .runningCommand: String(localized: "running a command")
        case .readingProject: String(localized: "reading the project")
        case .searchingWeb: String(localized: "searching the web")
        case .working: String(localized: "working")
        }
    }

    /// A phase worth naming next to "Working"; the generic phase adds nothing.
    fileprivate var isSpecific: Bool { self != .working }
}

extension TaskStatus {
    /// The visible label. `.unknown` has no text because an unknown status shows nothing.
    var title: String {
        switch self {
        case .unknown:
            ""
        case .offline:
            String(localized: "Offline · showing saved view")
        case .starting:
            String(localized: "Starting…")
        case let .working(phase):
            if let phase, phase.isSpecific {
                String(localized: "Working · \(phase.title)")
            } else {
                String(localized: "Working")
            }
        case .reconnecting:
            String(localized: "Reconnecting…")
        case .stopping:
            String(localized: "Stopping…")
        case .needsPermission:
            String(localized: "Needs permission")
        case .needsSignIn:
            String(localized: "Sign-in needed")
        case .usageLimit:
            String(localized: "Usage limit reached")
        case .gitUnconfirmed:
            String(localized: "Couldn't confirm a Git step")
        case .failed:
            String(localized: "Stopped with an error")
        case .waitingForReply:
            String(localized: "Waiting for your reply")
        case .finished:
            String(localized: "Finished · send a message to continue")
        case .stopped:
            String(localized: "Stopped · send a message to continue")
        case .notStarted:
            String(localized: "Not started")
        }
    }

    /// The status symbol, or nil when the status shows a spinner or nothing.
    var systemImage: String? {
        switch self {
        case .unknown, .starting, .working, .reconnecting, .stopping:
            nil
        case .offline: "wifi.slash"
        case .needsPermission: "hand.raised.fill"
        case .needsSignIn: "person.crop.circle.badge.exclamationmark"
        case .usageLimit: "hourglass"
        case .gitUnconfirmed: "questionmark.circle"
        case .failed: "xmark.octagon.fill"
        case .waitingForReply: "bubble.left"
        case .finished: "checkmark.circle"
        case .stopped: "stop.circle"
        case .notStarted: "circle.dashed"
        }
    }

    /// The symbol's or spinner's colour. Colour is never the only signal: the title
    /// always accompanies it.
    var tint: Color {
        switch self {
        case .needsPermission, .needsSignIn, .usageLimit, .gitUnconfirmed:
            .orange
        case .failed:
            .red
        case .starting, .working:
            JetDesign.accent
        case .unknown, .offline, .reconnecting, .stopping, .waitingForReply, .finished, .stopped, .notStarted:
            .secondary
        }
    }

    /// In progress: a small system spinner stands in for the symbol.
    var showsSpinner: Bool {
        switch self {
        case .starting, .working, .reconnecting, .stopping: true
        default: false
        }
    }

    /// What VoiceOver reads for this status. Empty for `.unknown`.
    var accessibilityDescription: String {
        switch self {
        case .unknown:
            ""
        case .offline:
            String(localized: "Offline, showing saved view")
        case .starting:
            String(localized: "Starting")
        case let .working(phase):
            if let phase, phase.isSpecific {
                String(localized: "Working, \(phase.accessibilityDescription)")
            } else {
                String(localized: "Working")
            }
        case .reconnecting:
            String(localized: "Reconnecting")
        case .stopping:
            String(localized: "Stopping")
        case .needsPermission:
            String(localized: "Needs permission")
        case .needsSignIn:
            String(localized: "Sign-in needed")
        case .usageLimit:
            String(localized: "Usage limit reached")
        case .gitUnconfirmed:
            String(localized: "Couldn't confirm a Git step")
        case .failed:
            String(localized: "Stopped with an error")
        case .waitingForReply:
            String(localized: "Waiting for your reply")
        case .finished:
            String(localized: "Finished, send a message to continue")
        case .stopped:
            String(localized: "Stopped, send a message to continue")
        case .notStarted:
            String(localized: "Not started")
        }
    }

    /// What a sidebar row shows in its 16-point glyph column (design §8 "Row glyph").
    enum RowGlyph: Equatable {
        case none
        case spinner
        case symbol(String)
        case unreadDot
    }

    func rowGlyph(isUnread: Bool) -> RowGlyph {
        if showsSpinner { return .spinner }
        switch self {
        case .offline, .needsPermission, .needsSignIn, .usageLimit, .gitUnconfirmed, .failed:
            return systemImage.map(RowGlyph.symbol) ?? .none
        case .unknown, .waitingForReply, .finished, .stopped, .notStarted,
             .starting, .working, .reconnecting, .stopping:
            return isUnread ? .unreadDot : .none
        }
    }
}
