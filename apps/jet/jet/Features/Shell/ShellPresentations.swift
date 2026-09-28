import SwiftUI

/// Every sheet, dialog and alert the shell presents, attached once at the
/// DesktopShellView root (design §6.9, §6.10, §6.2). Views request them through
/// the session's intents; this router only maps state to presentation.
struct ShellPresentations: ViewModifier {
    @Bindable var session: DesktopSession

    func body(content: Content) -> some View {
        content
            .sheet(item: $session.presentedSheet) { sheet in
                sheetContent(sheet)
            }
            .sheet(item: $session.removalPreview) { preview in
                ProjectRemovalSheet(session: session, preview: preview)
            }
            .confirmationDialog(
                runControlTitle,
                isPresented: runControlPresented,
                titleVisibility: .visible,
                presenting: session.runControlConfirmation
            ) { control in
                switch control {
                case .interruptTurn:
                    Button("Interrupt") { Self.confirmRunControl(session) }
                        .keyboardShortcut(.defaultAction)
                    Button("Cancel", role: .cancel, action: session.cancelRunControl)
                case .stopRun:
                    Button("Stop Assistant", role: .destructive) { Self.confirmRunControl(session) }
                    Button("Cancel", role: .cancel, action: session.cancelRunControl)
                        .keyboardShortcut(.defaultAction)
                }
            } message: { control in
                switch control {
                case .interruptTurn:
                    Text("\(assistantName) stops its current reply. Messages and changes are kept, and waiting messages go next.")
                case .stopRun:
                    Text("\(assistantName) quits, including commands it started (Stop Run). Messages and changes are kept. Send a message to start it again.")
                }
            }
            .alert(
                pendingNavigationTitle,
                isPresented: pendingNavigationPresented,
                presenting: session.pendingNavigation
            ) { _ in
                Button("Save") { Self.resolvePendingNavigation(session, save: true) }
                    .keyboardShortcut(.defaultAction)
                Button("Don't Save") { Self.resolvePendingNavigation(session, save: false) }
                Button("Cancel", role: .cancel) { Self.resolvePendingNavigation(session, save: nil) }
            } message: { _ in
                Text("Your edits are lost if you don't save them.")
            }
    }

    @ViewBuilder
    private func sheetContent(_ sheet: ShellSheet) -> some View {
        switch sheet {
        case .rename:
            RenameTaskSheet(session: session)
        case let .keepChanges(ref, mode):
            KeepChangesSheet(session: session, ref: ref, mode: mode)
        case let .moveToTrash(ref, deleteEverywhere):
            MoveToTrashSheet(session: session, ref: ref, deleteEverywhere: deleteEverywhere)
        case let .repeatDaily(ref):
            RepeatDailySheet(session: session, ref: ref)
        case let .taskSettings(ref):
            TaskSettingsSheet(session: session, ref: ref)
        case let .addProject(planeRegistryID, droppedURL):
            AddProjectSheet(session: session, planeRegistryID: planeRegistryID, droppedURL: droppedURL)
        }
    }

    // MARK: - Interrupt and Stop Assistant

    private var assistantName: String {
        session.selectedAssistantName ?? "Claude Code"
    }

    private var runControlTitle: Text {
        switch session.runControlConfirmation {
        case .stopRun: Text("Stop \(assistantName) for this task?")
        case .interruptTurn, nil: Text("Interrupt \(assistantName)?")
        }
    }

    /// The buttons clear the state themselves, so dismissal never races with the
    /// confirmation. Any other dismissal (a closing window, Escape) cancels.
    private var runControlPresented: Binding<Bool> {
        Binding(get: { Self.isRunControlPresented(session) }, set: { presented in
            if !presented, session.supervisionOperation == nil, session.runControlConfirmation != nil {
                session.cancelRunControl()
            }
        })
    }

    /// Hidden while the confirmed request is in flight.
    static func isRunControlPresented(_ session: DesktopSession) -> Bool {
        session.runControlConfirmation != nil && session.supervisionOperation == nil
    }

    /// Starts the confirmed Command before the dialog finishes dismissing, so the
    /// confirmation it reads is still set. A request that fails keeps its Command ID
    /// in the session; only the dialog state is cleared so it doesn't reappear.
    @discardableResult
    static func confirmRunControl(_ session: DesktopSession) -> Task<Void, Never> {
        Task.immediate { @MainActor in
            await session.confirmRunControl()
            if session.runControlConfirmation != nil, session.supervisionOperation == nil {
                session.cancelRunControl()
            }
        }
    }

    // MARK: - Unsaved file edits

    private var pendingNavigationTitle: Text {
        Text("Save changes to \(session.pendingNavigation?.fileName ?? "")?")
    }

    /// Dismissing the alert without a button keeps the edits, like Cancel.
    private var pendingNavigationPresented: Binding<Bool> {
        Binding(get: { session.pendingNavigation != nil }, set: { presented in
            if !presented, session.pendingNavigation != nil {
                session.pendingNavigation = nil
            }
        })
    }

    /// Resolves synchronously up to the first suspension, so the held navigation is
    /// cleared before the alert finishes dismissing.
    @discardableResult
    static func resolvePendingNavigation(_ session: DesktopSession, save: Bool?) -> Task<Void, Never> {
        Task.immediate { @MainActor in
            await session.resolvePendingNavigation(save: save)
        }
    }
}
