import SwiftUI
#if os(macOS)
import AppKit
#endif

/// View › Bigger / Smaller for the transcript's text-size scale.
enum TranscriptTextScale {
    static let steps = JetDesign.transcriptScaleSteps

    /// The nearest step above `value`, or nil at the largest size.
    static func bigger(than value: Double) -> Double? {
        steps.first { $0 > value + 0.001 }
    }

    /// The nearest step below `value`, or nil at the smallest size.
    static func smaller(than value: Double) -> Double? {
        steps.last { $0 < value - 0.001 }
    }
}

/// Help menu destinations. Both open the documentation in the browser.
enum JetHelpLinks {
    static let help = URL(string: "https://github.com/apexgang/jet#readme")!
    static let safety = URL(string: "https://github.com/apexgang/jet#what-jet-manages")!
}

#if os(macOS)
/// The menu bar (design §6.14). Items that don't apply stay visible and dimmed.
struct JetCommands: Commands {
    let session: DesktopSession

    @FocusedValue(\.isEditingText) private var isEditingText
    @FocusedValue(\.isComposerFocused) private var isComposerFocused
    @FocusedValue(\.hasOpenDialog) private var hasOpenDialog
    @FocusedValue(\.isMainWindow) private var isMainWindow
    @AppStorage(JetDesign.transcriptScaleKey) private var textScale = 1.0
    @AppStorage("jet.transcript.show-technical") private var showsTechnicalActivity = false
    @Environment(\.openURL) private var openURL

    var body: some Commands {
        let s = ShellCommandState(
            session: session,
            isEditingText: isEditingText == true,
            isComposerFocused: isComposerFocused == true,
            hasOpenDialog: hasOpenDialog == true
        )
        // The Task and Changes menus act on the main window, never from Settings.
        let inMainWindow = isMainWindow == true

        CommandGroup(replacing: .newItem) {
            Button("New Task") {
                session.guardUnsavedEdits {
                    session.sidebarItem = .newTask
                    session.composerFocusRequest += 1
                }
            }
            .keyboardShortcut("n", modifiers: .command)

            Menu("New Task in Project") {
                newTaskInProjectItems
            }
            .disabled(session.allProjects.isEmpty)

            Button("Add Project…") { session.presentAddProject(droppedURL: nil) }
                .keyboardShortcut("o", modifiers: [.command, .shift])
        }

        // SwiftUI can't add to the system Find submenu, so Find Task sits just above it.
        CommandGroup(before: .textEditing) {
            Button("Find Task…", action: session.findTask)
                .keyboardShortcut("k", modifiers: .command)
        }

        SidebarCommands()

        CommandGroup(after: .sidebar) {
            Button(session.isWorkPanelPresented ? "Hide Details" : "Show Details", action: session.toggleDetails)
                .keyboardShortcut("0", modifiers: [.command, .option])
                .disabled(!s.detailsAvailable && !session.isWorkPanelPresented)
            ForEach(Array(WorkPanelTab.inspectorTabs.enumerated()), id: \.element) { index, tab in
                Button(tab.title) {
                    // Leaving edit mode asks about an unsaved file first.
                    session.guardUnsavedEdits { session.showDetails(tab) }
                }
                .keyboardShortcut(KeyEquivalent(Character(String(index + 1))), modifiers: [.command, .option])
                .disabled(!s.detailsAvailable)
            }
            Divider()
            Button("Show Jet Trash", action: session.showTrash)
            Toggle("Show Technical Activity", isOn: $showsTechnicalActivity)
            Divider()
            Button("Bigger") {
                if let scale = TranscriptTextScale.bigger(than: textScale) { textScale = scale }
            }
            // ⌘= (the unshifted key most Mac apps accept for Bigger on US layouts).
            .keyboardShortcut("=", modifiers: .command)
            .disabled(TranscriptTextScale.bigger(than: textScale) == nil)
            Button("Smaller") {
                if let scale = TranscriptTextScale.smaller(than: textScale) { textScale = scale }
            }
            .keyboardShortcut("-", modifiers: .command)
            .disabled(TranscriptTextScale.smaller(than: textScale) == nil)
            Button("Actual Size") { textScale = 1.0 }
                .keyboardShortcut("0", modifiers: .command)
                .disabled(abs(textScale - 1.0) < 0.001)
        }

        CommandMenu("Task") {
            Group {
                Button(s.sendTitle, action: send)
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(!s.canSend)
                Button("Interrupt…") { session.requestInterrupt(thenReply: false) }
                    .keyboardShortcut(".", modifiers: .command)
                    .disabled(!s.canInterrupt)
                Button("Stop Assistant…", action: session.requestStopAssistant)
                    .disabled(!s.canStopAssistant)
                Divider()
                Button("Rename…") { session.presentRename() }
                    .disabled(!s.canRename)
                Button("Repeat Daily…") { session.presentRepeatDaily() }
                    .disabled(!s.canEditTask)
                Button("Task Settings…") { session.presentTaskSettings() }
                    .disabled(!s.canEditTask)
                Divider()
                Button("Show Working Copy in Finder", action: session.revealWorkingCopy)
                    .disabled(!s.canRevealWorkingCopy)
                Button("Copy Working Copy Path", action: session.copyWorkingCopyPath)
                    .disabled(!s.canCopyWorkingCopyPath)
                Divider()
                Button("Move to Jet Trash…") {
                    if let ref = s.taskRef { session.presentMoveToTrash(ref) }
                }
                .keyboardShortcut(.delete, modifiers: .command)
                .disabled(!s.canMoveToTrash)
                .modifierKeyAlternate(.option) {
                    Button("Delete Everywhere…") {
                        if let ref = s.taskRef { session.presentMoveToTrash(ref, deleteEverywhere: true) }
                    }
                    // Same gate as the primary item, so ⌥⌘⌫ never fires while typing.
                    .disabled(!s.canMoveToTrash)
                }
            }
            .disabled(!inMainWindow)
        }

        CommandMenu("Changes") {
            Group {
                Button("Review Changes") { session.showChanges(.all) }
                    .disabled(!s.canReviewChanges)
                Button("Keep Changes…") { session.presentKeepChanges(mode: .plan) }
                    .keyboardShortcut("k", modifiers: [.command, .shift])
                    .disabled(!s.canKeepChanges)
                Divider()
                Button("Create Branch…") { session.presentKeepChanges(mode: .single(.branch)) }
                    .disabled(!s.canKeepChanges)
                Button("Commit…") { session.presentKeepChanges(mode: .single(.commit)) }
                    .disabled(!s.canKeepChanges)
                Button("Push…") { session.presentKeepChanges(mode: .single(.push)) }
                    .disabled(!s.canKeepChanges)
                Button("Open Draft Pull Request…") { session.presentKeepChanges(mode: .single(.draftPullRequest)) }
                    .disabled(!s.canKeepChanges)
                Divider()
                Button("Check Git Status") {
                    Task { await session.checkGitStatus() }
                }
                .disabled(!s.canCheckGitStatus)
                Button("Mark as Checked…") {
                    guard let delivery = s.gitStepToCheck else { return }
                    session.requestMarkAsChecked(delivery)
                }
                .disabled(s.gitStepToCheck == nil || s.modal)
            }
            .disabled(!inMainWindow)
        }

        CommandGroup(replacing: .help) {
            Button("Jet Help") { openURL(JetHelpLinks.help) }
            Button("How Jet Keeps Your Project Safe") { openURL(JetHelpLinks.safety) }
        }
    }

    /// Every project, grouped by computer when there are several.
    @ViewBuilder
    private var newTaskInProjectItems: some View {
        if session.showsComputerNames {
            ForEach(session.planes.filter { plane in session.allProjects.contains { $0.planeRegistryID == plane.id } }) { plane in
                Section(plane.name) {
                    ForEach(session.allProjects.filter { $0.planeRegistryID == plane.id }) { item in
                        newTaskButton(item)
                    }
                }
            }
        } else {
            ForEach(session.allProjects) { item in
                newTaskButton(item)
            }
        }
    }

    private func newTaskButton(_ item: JetPlaneProject) -> some View {
        Button(item.project.name) {
            session.startNewTask(in: item.project.id, on: item.planeRegistryID)
        }
    }

    /// Sends the draft, unless an input method is still composing text: marked text
    /// is never sent.
    private func send() {
        if ComposerTextInput.hasMarkedText { return }
        Task { await session.submitDraft() }
    }
}
#endif
