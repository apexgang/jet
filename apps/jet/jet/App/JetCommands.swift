import SwiftUI

#if os(macOS)
struct JetCommands: Commands {
    let session: DesktopSession

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Task", action: session.beginNewTask)
                .keyboardShortcut("n", modifiers: [.command])

            Button("Add Project…", action: session.requestAddProject)
                .keyboardShortcut("o", modifiers: [.command, .shift])

            Button("Manage Projects", action: session.showProjects)
        }

        CommandGroup(after: .textEditing) {
            Button("Search Jet", action: session.selectSearch)
                .keyboardShortcut("k", modifiers: [.command])
        }

        CommandMenu("Conversation") {
            Button("Send") {
                Task { await session.submitDraft() }
            }
                .keyboardShortcut(.return, modifiers: [.command])
                .disabled(!session.canSubmitDraft)
            Divider()
            Button("Interrupt Turn…") {
                session.requestRunControl(.interruptTurn)
            }
                .disabled(!session.canInterruptTurn || session.supervisionOperation != nil)
            Button("Stop Run…") {
                session.requestRunControl(.stopRun)
            }
                .disabled(!session.canStopRun || session.supervisionOperation != nil)
            Divider()
            Button("Rename") {}
                .disabled(true)
            Button("Fork") {}
                .disabled(true)
            Button("Archive") {}
                .disabled(true)
        }

        CommandMenu("Delivery") {
            Button("Review Branch…") { session.showGitDelivery(.branch) }
            Button("Review Commit…") { session.showGitDelivery(.commit) }
            Button("Review Push…") { session.showGitDelivery(.push) }
            Button("Review GitHub Draft Pull Request…") {
                session.showGitDelivery(.draftPullRequest)
            }
        }

        SidebarCommands()

        CommandGroup(after: .sidebar) {
            Button(session.isWorkPanelPresented ? "Hide Work Panel" : "Show Work Panel") {
                session.isWorkPanelPresented.toggle()
            }
            .keyboardShortcut("0", modifiers: [.command, .option])

            Divider()

            ForEach(Array(WorkPanelTab.allCases.enumerated()), id: \.element) { index, tab in
                Button(tab.title) {
                    session.selectedWorkPanel = tab
                    session.isWorkPanelPresented = true
                }
                .keyboardShortcut(KeyEquivalent(Character(String(index + 1))), modifiers: [.command, .option])
            }
        }
    }
}
#endif
