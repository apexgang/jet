import Foundation

/// One row of the native sidebar list. It maps onto `sidebarSelection`,
/// `selectedConversationID` and `selectedProjectID` in `DesktopSession`.
enum SidebarItem: Hashable, Sendable {
    case newTask
    case task(UUID)
    case project(UUID)
    case trash
}

/// Each destination keeps its own in-memory draft.
enum DraftKey: Hashable, Sendable {
    case newTask
    case conversation(UUID)
}

/// An operation the person started and is waiting on. Background refreshes are
/// tracked separately so they never change the Send button or block Rename.
enum UserOperation: String, Sendable {
    case starting
    case sending
    case renaming
}

/// A Conversation together with the Plane that owns it.
struct ConversationRef: Hashable, Sendable {
    let conversationID: UUID
    let planeRegistryID: UUID
}

enum KeepChangesMode: Hashable, Sendable {
    /// The reviewed chain: branch, then commit, then optionally push and a draft pull request.
    case plan
    /// One Git step from the Changes menu.
    case single(JetGitDeliveryChoice)
}

/// The one sheet the shell root presents at a time.
enum ShellSheet: Identifiable, Hashable, Sendable {
    case rename(ConversationRef)
    case keepChanges(ConversationRef, KeepChangesMode)
    case moveToTrash(ConversationRef, deleteEverywhere: Bool)
    case repeatDaily(ConversationRef)
    case taskSettings(ConversationRef)
    case addProject(planeRegistryID: UUID, droppedURL: URL?)

    var id: Self { self }
}

/// A navigation held back until the person decides what to do with an unsaved file edit.
struct PendingNavigation: Identifiable {
    let id: UUID
    let fileName: String
    let perform: @MainActor () -> Void

    init(id: UUID = UUID(), fileName: String, perform: @escaping @MainActor () -> Void) {
        self.id = id
        self.fileName = fileName
        self.perform = perform
    }
}

/// The single line above the composer: a symbol, text and at most one action.
struct ComposerNotice: Equatable, Sendable {
    enum Kind: Equatable, Sendable {
        case info
        /// Clears on the next edit of the draft.
        case confirmation
        case warning
        case error
    }

    enum Action: Equatable, Sendable {
        case tryAgainConnection
        case addProject
        case checkAssistantsAgain
        case reviewGitStep
        case openSettings(JetSettingsPane)
        case undoMoveToTrash(ConversationRef)
        /// Opens the assistant's usage in Settings (the usage-limit notice).
        case showUsage

        var title: String {
            switch self {
            case .tryAgainConnection: String(localized: "Try Again")
            case .addProject: String(localized: "Add Project…")
            case .checkAssistantsAgain: String(localized: "Check Again")
            case .reviewGitStep: String(localized: "Review…")
            case .openSettings(.agents): String(localized: "Assistant Settings…")
            case .openSettings: String(localized: "Open Settings…")
            case .undoMoveToTrash: String(localized: "Undo")
            case .showUsage: String(localized: "Usage…")
            }
        }
    }

    var kind: Kind
    var text: String
    var action: Action?

    init(kind: Kind, text: String, action: Action? = nil) {
        self.kind = kind
        self.text = text
        self.action = action
    }
}

/// Why Send / Start Task is unavailable. One value feeds the button, the menu
/// item and the composer caption.
enum SendBlocker: Equatable, Sendable {
    case empty
    case notConnected(computer: String)
    case gettingReady
    case noProject
    case noAssistant
    case queueFull
    case tooLong
    case gitStepUnconfirmed
    /// The person's own operation is running, or the task is still loading.
    case busy

    var message: String? {
        switch self {
        case .empty, .busy:
            nil
        case let .notConnected(computer):
            String(localized: "Not connected to \(computer). Your message is kept.")
        case .gettingReady:
            String(localized: "Jet is getting ready on this Mac…")
        case .noProject:
            String(localized: "Choose a project to start.")
        case .noAssistant:
            String(localized: "Install Claude Code or Codex to start.")
        case .queueFull:
            String(localized: "Too many messages are waiting. Remove one or wait.")
        case .tooLong:
            String(localized: "Message is too long.")
        case .gitStepUnconfirmed:
            String(localized: "Jet paused this task until you check a Git step.")
        }
    }

    var action: ComposerNotice.Action? {
        switch self {
        case .notConnected: .tryAgainConnection
        case .noProject: .addProject
        case .noAssistant: .checkAssistantsAgain
        case .gitStepUnconfirmed: .reviewGitStep
        case .empty, .gettingReady, .queueFull, .tooLong, .busy: nil
        }
    }
}

/// How the next message reaches the core. A task that already has a Run always
/// continues with `submit_turn`; `start_run` is only for a task without Runs.
enum SendRoute: Equatable, Sendable {
    case startRun
    case submitTurn
}

/// Which changes Details › Changes should show.
enum ChangesRequest: Hashable, Sendable {
    case all
    case lastReply
    case reply(turn: UInt32, runID: UUID?)
}
