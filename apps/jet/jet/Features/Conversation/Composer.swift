import SwiftUI
#if os(macOS)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

// MARK: - Pure helpers (design §6.6)

/// Where the composer sits: under a task's transcript, or inline on New Task.
enum ComposerPlacement: Equatable {
    case task
    case newTask

    var lineRange: ClosedRange<Int> {
        switch self {
        case .task: 1 ... 8
        case .newTask: 3 ... 8
        }
    }

    var maxWidth: CGFloat {
        switch self {
        case .task: JetDesign.readingWidth
        case .newTask: JetDesign.writingWidth
        }
    }
}

enum ComposerPreferences {
    /// Settings › General › Press Return to Send (Bool, off by default). Off,
    /// Return inserts a newline and ⌘↩ sends; on, Return sends and Shift-Return
    /// inserts a newline.
    static let returnSendsKey = "jet.composer.return-sends"
}

enum ComposerReturnAction: Equatable {
    case send
    /// The text view handles the key: it inserts a newline or commits IME text.
    case passThrough
}

enum ComposerKeyRouting {
    /// What Return does in the composer. Marked IME text is never sent, and
    /// ⌘↩ belongs to the menu command, so only a bare Return can send.
    static func returnAction(
        returnSends: Bool,
        modifiers: EventModifiers,
        hasMarkedText: Bool
    ) -> ComposerReturnAction {
        guard !hasMarkedText else { return .passThrough }
        let counted = modifiers.intersection([.shift, .option, .control, .command])
        guard counted.isDisjoint(with: [.command, .option, .control]) else { return .passThrough }
        return returnSends && counted.isEmpty ? .send : .passThrough
    }
}

#if os(macOS)
enum ComposerTextInput {
    /// True while an input method holds uncommitted (marked) text in the key
    /// window's text view, such as a Japanese conversion before it is confirmed.
    static var hasMarkedText: Bool {
        (NSApp.keyWindow?.firstResponder as? NSTextInputClient)?.hasMarkedText() ?? false
    }
}
#endif

/// The byte counter that replaces the key hint above 80% of the prompt limit.
enum ComposerSizeLimit: Equatable {
    case hidden
    case near(Int)
    case over(Int)

    static func state(bytes: Int, limit: Int = JetTurnQueue.maximumPromptBytes) -> ComposerSizeLimit {
        if bytes > limit { return .over(bytes) }
        if bytes > limit * 4 / 5 { return .near(bytes) }
        return .hidden
    }

    static func label(bytes: Int, limit: Int = JetTurnQueue.maximumPromptBytes) -> String {
        String(localized: "\(bytes.formatted()) of \(limit.formatted()) bytes")
    }
}

enum ComposerHint {
    /// The key hint under the field.
    static func text(returnSends: Bool, startsTask: Bool) -> String {
        switch (returnSends, startsTask) {
        case (false, false): String(localized: "⌘↩ Send")
        case (true, false): String(localized: "↩ Send")
        case (false, true): String(localized: "⌘↩ Start Task")
        case (true, true): String(localized: "↩ Start Task")
        }
    }

    /// The Send button's tooltip: its shortcut, or why it is unavailable.
    static func sendHelp(blocker: SendBlocker?, returnSends: Bool, startsTask: Bool) -> String {
        switch blocker {
        case .none:
            switch (returnSends, startsTask) {
            case (false, false): String(localized: "Send (⌘↩)")
            case (true, false): String(localized: "Send (↩)")
            case (false, true): String(localized: "Start Task (⌘↩)")
            case (true, true): String(localized: "Start Task (↩)")
            }
        case .empty?: String(localized: "Type a message first.")
        case .busy?: String(localized: "Wait until Jet finishes your last action.")
        case let blocker?: blocker.message ?? ""
        }
    }
}

/// The line under the field: a blocker with its fix, otherwise what sending does.
struct ComposerCaption: Equatable {
    enum Tone: Equatable {
        case plain
        case progress
        case warning
        case error
    }

    var tone: Tone
    var text: String
    var action: ComposerNotice.Action?

    static func resolve(
        blocker: SendBlocker?,
        sessionCaption: String?,
        placement: ComposerPlacement,
        checklistVisible: Bool,
        newTaskProjectName: String?
    ) -> ComposerCaption? {
        let checklistCovers = placement == .newTask && checklistVisible
        if let blocker, let message = blocker.message,
           !(checklistCovers && isCoveredByChecklist(blocker))
        {
            return ComposerCaption(tone: tone(for: blocker), text: message, action: blocker.action)
        }
        if checklistCovers {
            let text = newTaskProjectName.map {
                String(localized: "Jet works in a separate copy of \($0). Your folder doesn't change until you keep the changes.")
            } ?? String(localized: "Jet works in a separate copy of your project. Your folder doesn't change until you keep the changes.")
            return ComposerCaption(tone: .plain, text: text)
        }
        return sessionCaption.map { ComposerCaption(tone: .plain, text: $0) }
    }

    /// New Task's checklist already explains these, with their fixes.
    static func isCoveredByChecklist(_ blocker: SendBlocker) -> Bool {
        switch blocker {
        case .notConnected, .gettingReady, .noProject, .noAssistant: true
        case .empty, .queueFull, .tooLong, .gitStepUnconfirmed, .busy: false
        }
    }

    private static func tone(for blocker: SendBlocker) -> Tone {
        switch blocker {
        case .tooLong: .error
        case .gettingReady: .progress
        default: .warning
        }
    }
}

/// The single line above the field.
enum ComposerNoticeSlot: Equatable {
    case notice(ComposerNotice)
    case notificationOffer

    /// Errors and warnings first, then the task's sign-in or usage notice, then
    /// info and confirmations, then the one-time notification offer.
    static func resolve(
        notice: ComposerNotice?,
        statusNotice: ComposerNotice?,
        offerEligible: Bool
    ) -> ComposerNoticeSlot? {
        if let notice, notice.kind == .error || notice.kind == .warning { return .notice(notice) }
        if let statusNotice { return .notice(statusNotice) }
        if let notice { return .notice(notice) }
        return offerEligible ? .notificationOffer : nil
    }
}

// WP7: the lead may swap NotificationOffer for WP4's session.shouldOfferNotifications,
// turnOnNotifications() and declineNotificationOffer() in wave 3 (critic 2.9). The
// rule and the writes below already match WP4's amended behaviour.
/// The one-time offer after a task started in this app (design §6.6).
enum NotificationOffer {
    static let preferenceKeys = [
        JetNotificationPreferences.approvalsKey,
        JetNotificationPreferences.completionsKey,
        JetNotificationPreferences.failuresKey,
    ]

    static func isEligible(
        offerShown: Bool,
        authorization: JetNotificationAuthorization,
        anyPreferenceOn: Bool,
        startedHere: Bool
    ) -> Bool {
        !offerShown && authorization != .denied && !anyPreferenceOn && startedHere
    }

    /// Turn On: asks macOS for permission, and only when it is granted turns on
    /// every notification preference.
    static func turnOn(session: DesktopSession, defaults: UserDefaults = .standard) async {
        guard !session.isPreviewSession else { return }
        session.memory.notificationOfferShown = true
        let conversationID = session.selectedConversationID
        let granted = await session.requestNotificationAuthorization()
        if granted {
            for key in preferenceKeys { defaults.set(true, forKey: key) }
        }
        guard session.selectedConversationID == conversationID else { return }
        session.composerNotice = granted
            ? ComposerNotice(
                kind: .confirmation,
                text: String(localized: "Notifications are on. You can change them in Settings.")
            )
            : ComposerNotice(
                kind: .info,
                text: String(localized: "Notifications for Jet are off in System Settings.")
            )
    }

    /// Not Now, or the offer went away: it is never shown again.
    static func decline(session: DesktopSession) {
        guard !session.isPreviewSession, !session.memory.notificationOfferShown else { return }
        session.memory.notificationOfferShown = true
    }
}

/// A New Task project choice: a project on one computer.
struct NewTaskProjectChoice: Hashable {
    let planeRegistryID: UUID
    let projectID: UUID
}

enum ComposerAssistantChoices {
    /// Each assistant's product name; two assistants with the same name are told
    /// apart by version.
    static func labels(for crafts: [JetInstalledCraft]) -> [String: String] {
        let names = crafts.map { DesktopSession.harnessLabel($0.harnesses.first ?? $0.id) }
        var counts: [String: Int] = [:]
        for name in names { counts[name, default: 0] += 1 }
        var labels: [String: String] = [:]
        for (craft, name) in zip(crafts, names) {
            labels[craft.id] = (counts[name] ?? 0) > 1 ? "\(name) (\(craft.version))" : name
        }
        return labels
    }
}

// MARK: - Composer

struct WorkspaceComposer: View {
    @Bindable var session: DesktopSession
    let composerFocused: FocusState<Bool>.Binding
    var placement: ComposerPlacement = .task

    @AppStorage(ComposerPreferences.returnSendsKey) private var returnSends = false
    @AppStorage(JetDesign.transcriptScaleKey) private var textScale = 1.0
    @AppStorage(JetNotificationPreferences.approvalsKey) private var approvalsOn = false
    @AppStorage(JetNotificationPreferences.completionsKey) private var completionsOn = false
    @AppStorage(JetNotificationPreferences.failuresKey) private var failuresOn = false
    @Environment(\.colorSchemeContrast) private var contrast
#if os(macOS)
    @Environment(\.openSettings) private var openSettings
#endif

    var body: some View {
        let content = VStack(alignment: .leading, spacing: 6) {
            noticeSlot
            container
            captionRow
        }
        .frame(maxWidth: placement.maxWidth)
        .onChange(of: session.draft) { old, new in
            if !new.isEmpty, new != old, session.composerNotice?.kind == .confirmation {
                session.composerNotice = nil
            }
        }
        .onChange(of: session.composerNotice) { old, new in
            guard let new, new.kind == .error, new != old else { return }
            AccessibilityNotification.Announcement(new.text).post()
        }
        .task {
            // The notification offer isn't shown once notifications were refused.
            if placement == .task { await session.refreshNotificationAuthorization() }
        }
        // WP7: the lead applies WP5's `isComposerFocused` focused value here in
        // wave 3 (critic 4.2), next to `reportsTextEditing` on the field.

        switch placement {
        case .task:
            content
                .padding(EdgeInsets(top: 8, leading: 16, bottom: 12, trailing: 16))
                .frame(maxWidth: .infinity)
        case .newTask:
            content
        }
    }

    // MARK: Notice slot

    private var slot: ComposerNoticeSlot? {
        ComposerNoticeSlot.resolve(
            notice: session.composerNotice,
            statusNotice: placement == .task ? session.statusNotice : nil,
            offerEligible: placement == .task && offerEligible
        )
    }

    private var offerEligible: Bool {
        guard let conversationID = session.selectedConversationID else { return false }
        return NotificationOffer.isEligible(
            offerShown: session.memory.notificationOfferShown,
            authorization: session.notificationAuthorization,
            anyPreferenceOn: approvalsOn || completionsOn || failuresOn,
            startedHere: session.memory.assistant(for: conversationID) != nil
        )
    }

    @ViewBuilder private var noticeSlot: some View {
        switch slot {
        case let .notice(notice)?:
            InlineNotice(notice: notice, perform: perform)
        case .notificationOffer?:
            NotificationOfferRow {
                Task { await NotificationOffer.turnOn(session: session) }
            } notNow: {
                NotificationOffer.decline(session: session)
            }
            .onDisappear { NotificationOffer.decline(session: session) }
        case nil:
            EmptyView()
        }
    }

    // MARK: Container

    private var showsControlsRow: Bool {
        placement == .newTask || session.showsAssistantPicker
    }

    private var container: some View {
        VStack(alignment: .leading, spacing: 10) {
            if showsControlsRow {
                field
                controlsRow
            } else {
                // The buttons line up with the field's last line of text.
                HStack(alignment: .lastTextBaseline, spacing: 8) {
                    field
                    trailingControls
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background {
            RoundedRectangle(cornerRadius: JetDesign.fieldRadius, style: .continuous)
                .fill(ComposerColors.textBackground)
                .onTapGesture { composerFocused.wrappedValue = true }
        }
        .overlay {
            RoundedRectangle(cornerRadius: JetDesign.fieldRadius, style: .continuous)
                .strokeBorder(borderColor, lineWidth: composerFocused.wrappedValue ? 2 : 1)
                .allowsHitTesting(false)
        }
    }

    private var borderColor: Color {
        if composerFocused.wrappedValue { return JetDesign.accent }
        return contrast == .increased ? Color.primary.opacity(0.6) : ComposerColors.separator
    }

    private var fieldFont: Font {
        .system(size: JetDesign.TextSize.content * min(max(textScale, 0.85), 2.0))
    }

    // MARK: Field

    @ViewBuilder private var field: some View {
#if os(macOS)
        macField
#else
        TextField(session.composerPlaceholder, text: $session.draft, axis: .vertical)
            .textFieldStyle(.plain)
            .lineLimit(placement.lineRange)
            .font(fieldFont)
            .focused(composerFocused)
            .accessibilityLabel("Task message")
            .accessibilityHint(session.composerPlaceholder)
            .reportsTextEditing(composerFocused.wrappedValue)
#endif
    }

#if os(macOS)
    /// A TextEditor, so Return inserts a newline by default. Two hidden texts in
    /// the same font size it between the placement's minimum and maximum lines.
    private var macField: some View {
        let draft = session.draft
        return ZStack(alignment: .topLeading) {
            Text(verbatim: " ")
                .lineLimit(placement.lineRange.lowerBound, reservesSpace: true)
            Text(verbatim: draft.hasSuffix("\n") ? draft + " " : draft)
                .lineLimit(placement.lineRange.upperBound)
                .padding(.horizontal, 5)
        }
        .hidden()
        .accessibilityHidden(true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .topLeading) {
            TextEditor(text: $session.draft)
                .textEditorStyle(.plain)
                .scrollContentBackground(.hidden)
                .focused(composerFocused)
                .onKeyPress(.return, phases: .down) { press in
                    switch ComposerKeyRouting.returnAction(
                        returnSends: returnSends,
                        modifiers: press.modifiers,
                        hasMarkedText: ComposerTextInput.hasMarkedText
                    ) {
                    case .send:
                        send()
                        return .handled
                    case .passThrough:
                        return .ignored
                    }
                }
                .accessibilityLabel("Task message")
                .accessibilityHint(session.composerPlaceholder)
                .reportsTextEditing(composerFocused.wrappedValue)
        }
        .overlay(alignment: .topLeading) {
            if draft.isEmpty {
                Text(session.composerPlaceholder)
                    .foregroundStyle(ComposerColors.placeholder)
                    .lineLimit(placement.lineRange.upperBound)
                    .padding(.leading, 5)
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            }
        }
        .font(fieldFont)
    }
#endif

    // MARK: Controls

    private var controlsRow: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 12) {
                leadingControls
                Spacer(minLength: 12)
                trailingControls
            }
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 12) { leadingControls }
                HStack(spacing: 8) {
                    Spacer(minLength: 0)
                    trailingControls
                }
            }
            VStack(alignment: .leading, spacing: 8) {
                VStack(alignment: .leading, spacing: 6) { leadingControls }
                HStack(spacing: 8) {
                    Spacer(minLength: 0)
                    trailingControls
                }
            }
        }
    }

    @ViewBuilder private var leadingControls: some View {
        if placement == .newTask {
            projectMenu
        }
        if placement == .newTask || session.showsAssistantPicker {
            assistantMenu
        }
        if placement == .newTask, session.planes.count >= 2 {
            computerMenu
        }
    }

    private var projectMenu: some View {
        let name = session.selectedProject?.name ?? String(localized: "Choose Project")
        return Menu {
            Picker("Project", selection: projectSelection) {
                if session.planes.count >= 2 {
                    ForEach(projectGroups, id: \.planeRegistryID) { group in
                        Section(group.name) {
                            ForEach(group.choices, id: \.self) { choice in
                                Text(group.name(of: choice)).tag(Optional(choice))
                            }
                        }
                    }
                } else {
                    ForEach(projectGroups, id: \.planeRegistryID) { group in
                        ForEach(group.choices, id: \.self) { choice in
                            Text(group.name(of: choice)).tag(Optional(choice))
                        }
                    }
                }
            }
            .pickerStyle(.inline)
            .labelsHidden()
            Divider()
            Button("Add Project…") { session.presentAddProject(droppedURL: nil) }
        } label: {
            Label(name, systemImage: "folder")
        }
        .modifier(ComposerMenuStyle(disabled: session.userOperation != nil))
        .help("Choose the project for this task")
        .accessibilityLabel("Project")
        .accessibilityValue(name)
    }

    private struct ProjectGroup {
        let planeRegistryID: UUID
        let name: String
        let projects: [JetProjectSummary]

        var choices: [NewTaskProjectChoice] {
            projects.map { NewTaskProjectChoice(planeRegistryID: planeRegistryID, projectID: $0.id) }
        }

        func name(of choice: NewTaskProjectChoice) -> String {
            projects.first { $0.id == choice.projectID }?.name ?? ""
        }
    }

    /// Computers with projects, in the sidebar's computer order.
    private var projectGroups: [ProjectGroup] {
        session.planes.compactMap { plane in
            let projects = session.projects(on: plane.id)
            guard !projects.isEmpty else { return nil }
            return ProjectGroup(planeRegistryID: plane.id, name: plane.name, projects: projects)
        }
    }

    private var projectSelection: Binding<NewTaskProjectChoice?> {
        Binding {
            session.selectedProject.map {
                NewTaskProjectChoice(planeRegistryID: session.newTaskPlaneRegistryID, projectID: $0.id)
            }
        } set: { choice in
            guard let choice else { return }
            session.chooseNewTaskProject(choice.projectID, on: choice.planeRegistryID)
        }
    }

    private var assistantMenu: some View {
        let crafts = session.selectedSetupSnapshot?.capabilities.crafts ?? []
        let labels = ComposerAssistantChoices.labels(for: crafts)
        let name = session.selectedAssistantName ?? String(localized: "Choose Assistant")
        return Menu {
            Picker("Assistant", selection: craftSelection) {
                ForEach(crafts) { craft in
                    Text(labels[craft.id] ?? craft.id).tag(Optional(craft.id))
                }
            }
            .pickerStyle(.inline)
            .labelsHidden()
        } label: {
            Text(name)
        }
        .modifier(ComposerMenuStyle(disabled: session.userOperation != nil))
        .help("Choose the assistant for this task")
        .accessibilityLabel("Assistant")
        .accessibilityValue(name)
    }

    private var craftSelection: Binding<String?> {
        Binding {
            session.selectedCraftID
        } set: { craftID in
            guard let craftID else { return }
            session.chooseCraft(craftID)
        }
    }

    private var computerMenu: some View {
        let name = session.planeName(session.newTaskPlaneRegistryID)
        return Menu {
            Picker("Computer", selection: computerSelection) {
                ForEach(session.planes) { plane in
                    Text(computerTitle(plane)).tag(plane.id)
                }
            }
            .pickerStyle(.inline)
            .labelsHidden()
        } label: {
            Text(name)
        }
        .modifier(ComposerMenuStyle(disabled: session.userOperation != nil))
        .help("Choose the computer this task runs on")
        .accessibilityLabel("Computer")
        .accessibilityValue(name)
    }

    private func computerTitle(_ plane: JetPlanePresentation) -> String {
        session.isComputerOffline(plane.id)
            ? String(localized: "\(plane.name) · Offline")
            : plane.name
    }

    private var computerSelection: Binding<UUID> {
        Binding {
            session.newTaskPlaneRegistryID
        } set: { planeRegistryID in
            session.chooseNewTaskComputer(planeRegistryID)
        }
    }

    @ViewBuilder private var trailingControls: some View {
        if placement == .task, session.canInterruptTurn {
            Button {
                session.requestInterrupt(thenReply: false)
            } label: {
                Label("Interrupt", systemImage: "stop.fill")
            }
            .buttonStyle(.bordered)
            .help("Interrupt the current reply (⌘.)")
        }
        sendButton
    }

    private var sendButton: some View {
        Button(action: send) {
            HStack(spacing: 6) {
                Text(session.sendButtonTitle)
                Image(systemName: "arrow.up")
            }
        }
        .buttonStyle(.borderedProminent)
        .disabled(!session.canSend)
        .help(ComposerHint.sendHelp(
            blocker: session.sendBlocker,
            returnSends: returnSends,
            startsTask: session.nextSendStartsNewRun
        ))
        .accessibilityHint("Command-Return")
        .accessibilityIdentifier("send-task")
#if !os(macOS)
        // On macOS the Task menu owns ⌘↩.
        .keyboardShortcut(.return, modifiers: .command)
#endif
    }

    // MARK: Caption

    private var caption: ComposerCaption? {
        ComposerCaption.resolve(
            blocker: session.sendBlocker,
            sessionCaption: session.composerCaption,
            placement: placement,
            checklistVisible: placement == .newTask && !session.canStartTask,
            newTaskProjectName: session.selectedProject?.name
        )
    }

    private var captionRow: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            if let caption {
                ComposerCaptionView(caption: caption, perform: perform)
            }
            Spacer(minLength: 12)
            sizeOrHint
        }
        .font(.system(size: JetDesign.TextSize.metadata))
        .padding(.horizontal, 2)
    }

    @ViewBuilder private var sizeOrHint: some View {
        let bytes = session.draftBytes
        switch ComposerSizeLimit.state(bytes: bytes) {
        case .hidden:
            Text(ComposerHint.text(returnSends: returnSends, startsTask: session.nextSendStartsNewRun))
                .foregroundStyle(.secondary)
        case .near:
            Text(ComposerSizeLimit.label(bytes: bytes))
                .monospacedDigit()
                .foregroundStyle(.secondary)
        case .over:
            Text(ComposerSizeLimit.label(bytes: bytes))
                .monospacedDigit()
                .foregroundStyle(.red)
        }
    }

    // MARK: Actions

    private func send() {
#if os(macOS)
        if ComposerTextInput.hasMarkedText { return }
#endif
        guard session.canSend else { return }
        Task { await session.submitDraft() }
    }

    private func perform(_ action: ComposerNotice.Action) {
        switch action {
        case let .openSettings(pane):
            showSettings(pane)
        case .showUsage:
            showSettings(.agents)
        default:
            session.perform(action)
        }
    }

    private func showSettings(_ pane: JetSettingsPane) {
        session.requestSettings(pane)
#if os(macOS)
        openSettings()
#endif
    }
}

// MARK: - Pieces

private struct ComposerMenuStyle: ViewModifier {
    let disabled: Bool

    func body(content: Content) -> some View {
        content
            .menuStyle(.button)
            .buttonStyle(.borderless)
            .fixedSize()
            .font(.system(size: JetDesign.TextSize.control))
            .tint(JetDesign.accentText)
            .disabled(disabled)
    }
}

private struct ComposerCaptionView: View {
    let caption: ComposerCaption
    let perform: (ComposerNotice.Action) -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            switch caption.tone {
            case .plain:
                EmptyView()
            case .progress:
                ProgressView()
                    .controlSize(.mini)
                    .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 3 }
            case .warning:
                Image(systemName: "exclamationmark.triangle.fill")
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(.orange)
            case .error:
                Image(systemName: "xmark.octagon.fill")
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(.red)
            }
            Text(caption.text)
                .foregroundStyle(caption.tone == .warning || caption.tone == .error ? .primary : .secondary)
                .fixedSize(horizontal: false, vertical: true)
            if let action = caption.action {
                Button(action.title) { perform(action) }
#if os(macOS)
                    .buttonStyle(.link)
#else
                    .buttonStyle(.borderless)
#endif
                    .foregroundStyle(JetDesign.accentText)
                    .fixedSize()
            }
        }
        .accessibilityElement(children: .contain)
    }
}

private struct NotificationOfferRow: View {
    let turnOn: () -> Void
    let notNow: () -> Void

    @ScaledMetric(relativeTo: .callout) private var textSize = JetDesign.TextSize.control

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Image(systemName: "bell")
                    .foregroundStyle(.secondary)
                Text("Get a notification when a reply is ready or a task needs you?")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .combine)
            Spacer(minLength: 8)
            Button("Turn On", action: turnOn)
                .buttonStyle(.bordered)
                .controlSize(.small)
            Button("Not Now", action: notNow)
                .buttonStyle(.borderless)
                .controlSize(.small)
                .tint(JetDesign.accentText)
        }
        .font(.system(size: textSize))
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Platform colours for the composer container.
private enum ComposerColors {
    static var textBackground: Color {
#if os(macOS)
        Color(nsColor: .textBackgroundColor)
#else
        Color(uiColor: .systemBackground)
#endif
    }

    static var separator: Color {
#if os(macOS)
        Color(nsColor: .separatorColor)
#else
        Color(uiColor: .separator)
#endif
    }

#if os(macOS)
    static var placeholder: Color {
        Color(nsColor: .placeholderTextColor)
    }
#endif
}

#if DEBUG
#Preview("Composer") {
    DesktopPreviewScenes.composer[0].makeView()
        .frame(width: 1000, height: 260)
}
#endif
