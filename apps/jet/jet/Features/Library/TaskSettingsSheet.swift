import Foundation
import Observation
import SwiftUI

// MARK: - When a Reply Finishes: presets and the write planner

/// The four Git policy flags, in policy order: branch, commit, push, draft pull request.
struct ReplyFinishFlags: Hashable, Sendable {
    var branch = false
    var commit = false
    var push = false
    var draftPullRequest = false

    static let keys: [SettingKey] = [
        "git.auto_branch", "git.auto_commit", "git.auto_push", "git.auto_draft_pull_request",
    ].map { SettingKey(rawValue: $0)! }

    init(branch: Bool = false, commit: Bool = false, push: Bool = false, draftPullRequest: Bool = false) {
        self.branch = branch
        self.commit = commit
        self.push = push
        self.draftPullRequest = draftPullRequest
    }

    init(values: [Bool]) {
        self.init(branch: values[0], commit: values[1], push: values[2], draftPullRequest: values[3])
    }

    /// The resolved flags in a settings snapshot; a missing key is off (its built-in default).
    init(snapshot: JetSettingSnapshot) {
        self.init(values: Self.keys.map { key in
            if case let .flag(value)? = snapshot.value(for: key) { return value }
            return false
        })
    }

    var values: [Bool] { [branch, commit, push, draftPullRequest] }

    /// Which flags `snapshot` stores at `scope` itself rather than inheriting.
    static func explicit(in snapshot: JetSettingSnapshot, scope: JetSettingScope) -> [Bool] {
        keys.map { snapshot.source(for: $0) == .scope(scope) }
    }
}

/// The named choices under "When a Reply Finishes" (design §6.11).
enum ReplyFinishPreset: CaseIterable, Hashable, Sendable {
    case doNothing
    case saveToBranch
    case saveAndPush
    case saveAndOpenPullRequest

    var flags: ReplyFinishFlags {
        switch self {
        case .doNothing: ReplyFinishFlags()
        case .saveToBranch: ReplyFinishFlags(branch: true, commit: true)
        case .saveAndPush: ReplyFinishFlags(branch: true, commit: true, push: true)
        case .saveAndOpenPullRequest:
            ReplyFinishFlags(branch: true, commit: true, push: true, draftPullRequest: true)
        }
    }

    /// The preset with exactly these flags, or nil for a custom mix.
    static func matching(_ flags: ReplyFinishFlags) -> ReplyFinishPreset? {
        allCases.first { $0.flags == flags }
    }

    var title: String {
        switch self {
        case .doNothing: String(localized: "Do Nothing")
        case .saveToBranch: String(localized: "Save to a Branch")
        case .saveAndPush: String(localized: "Save and Push")
        case .saveAndOpenPullRequest: String(localized: "Save, Push and Open a Draft Pull Request")
        }
    }
}

/// What the picker shows as selected.
enum ReplyFinishChoice: Hashable, Sendable {
    case preset(ReplyFinishPreset)
    /// A mix no preset matches. Shown only while it is current; it can't be chosen.
    case custom
    /// A task follows its project; the value is the project's preset, nil when custom.
    case projectDefault(ReplyFinishPreset?)
}

/// One `set_setting` or `clear_setting` Command.
enum PolicyWrite: Equatable, Sendable {
    case set(SettingKey, JetSettingValue)
    case clear(SettingKey)

    var key: SettingKey {
        switch self {
        case let .set(key, _), let .clear(key): key
        }
    }
}

/// What a scope stores for the four flags: their resolved values and which it sets itself.
struct ReplyFinishScopeState: Equatable, Sendable {
    var effective: ReplyFinishFlags
    var explicit: [Bool]

    init(effective: ReplyFinishFlags, explicit: [Bool] = [false, false, false, false]) {
        self.effective = effective
        self.explicit = explicit
    }
}

/// The writes a choice needs.
enum ReplyFinishTarget: Equatable, Sendable {
    /// A project preset writes only the flags that change.
    case projectPreset(ReplyFinishFlags)
    /// A task preset pins all four flags on the task.
    case taskPreset(ReplyFinishFlags)
    /// A task clears its own flags and follows the project's values (given here).
    case projectDefault(ReplyFinishFlags)
}

// MARK: - Model

/// "When a Reply Finishes" and the other project or task Git settings, written as
/// one typed Command per key, in a fixed safe order.
@MainActor
@Observable
final class ReplyFinishModel {
    enum Scope: Equatable, Sendable {
        case project(UUID)
        case task(conversationID: UUID, projectID: UUID?)
    }

    enum Row: Equatable, Sendable {
        case replyFinish
        case branchPrefix
        case automaticNaming
    }

    enum Phase: Equatable, Sendable {
        case loading
        case loaded
        /// Loading failed with nothing to show.
        case unavailable
    }

    /// What Try Again or a confirmed Send Same Request Again goes on with.
    enum Resume: Equatable, Sendable {
        case choice(ReplyFinishChoice)
        case branchPrefix(String)
        case automaticNaming(Bool)
        case resetBranchPrefix
        case resetAutomaticNaming
    }

    /// A write whose outcome is unknown: kept with its Command ID and exact body.
    struct UncertainWrite: Equatable, Sendable {
        let write: PolicyWrite
        let commandID: UUID
        let row: Row
        let resume: Resume
    }

    static let branchPrefixKey = SettingKey(rawValue: "git.branch_prefix")!
    static let automaticNamingKey = SettingKey(rawValue: "utility.automatic_naming")!
    static let builtInBranchPrefix = "jet/"

    let planeRegistryID: UUID
    let scope: Scope
    let computer: String
    let isLocal: Bool
    @ObservationIgnored private let makeAccess: JetLibraryAccessProvider
    @ObservationIgnored private var isSeeded = false

    private(set) var phase: Phase = .loading
    private(set) var scopeSnapshot: JetSettingSnapshot?
    /// The project's settings, for a task's "Use Project Default".
    private(set) var projectSnapshot: JetSettingSnapshot?
    /// This computer's settings, for a project's "Reset to Default".
    private(set) var planeSnapshot: JetSettingSnapshot?
    private(set) var savingRow: Row?
    private(set) var isOffline = false
    var issue: LibraryIssue?
    private(set) var issueRow: Row?
    /// A choice that turns on pushing waits for the push confirmation.
    var pendingConfirmation: ReplyFinishChoice?
    var branchPrefixDraft = ""
    private(set) var branchPrefixError: String?
    private(set) var retry: (row: Row, resume: Resume)?
    private(set) var uncertain: UncertainWrite?

    init(
        planeRegistryID: UUID,
        scope: Scope,
        makeAccess: @escaping JetLibraryAccessProvider,
        computer: String = String(localized: "This Mac"),
        isLocal: Bool = true
    ) {
        self.planeRegistryID = planeRegistryID
        self.scope = scope
        self.makeAccess = makeAccess
        self.computer = computer
        self.isLocal = isLocal
    }

    var settingScope: JetSettingScope {
        switch scope {
        case let .project(projectID): .project(projectID)
        case let .task(conversationID, _): .conversation(conversationID)
        }
    }

    var isTask: Bool {
        if case .task = scope { return true }
        return false
    }

    var isBusy: Bool { savingRow != nil }

    // MARK: Reading

    var state: ReplyFinishScopeState? {
        guard let scopeSnapshot else { return nil }
        return ReplyFinishScopeState(
            effective: ReplyFinishFlags(snapshot: scopeSnapshot),
            explicit: ReplyFinishFlags.explicit(in: scopeSnapshot, scope: settingScope)
        )
    }

    /// The project's flags, for a task following them.
    var projectFlags: ReplyFinishFlags? {
        if let state, isTask, !state.explicit.contains(true) { return state.effective }
        return projectSnapshot.map(ReplyFinishFlags.init(snapshot:))
    }

    var choice: ReplyFinishChoice? {
        guard let state else { return nil }
        if isTask, !state.explicit.contains(true) {
            return .projectDefault(ReplyFinishPreset.matching(state.effective))
        }
        return ReplyFinishPreset.matching(state.effective).map(ReplyFinishChoice.preset) ?? .custom
    }

    /// The options in picker order. Custom appears only while it is current.
    var choices: [ReplyFinishChoice] {
        var options: [ReplyFinishChoice] = []
        if isTask, projectFlags != nil {
            options.append(.projectDefault(projectFlags.flatMap(ReplyFinishPreset.matching)))
        }
        options += ReplyFinishPreset.allCases.map(ReplyFinishChoice.preset)
        if choice == .custom { options.append(.custom) }
        return options
    }

    func title(for choice: ReplyFinishChoice) -> String {
        switch choice {
        case let .preset(preset): preset.title
        case .custom: String(localized: "Custom")
        case let .projectDefault(preset?): String(localized: "Use Project Default (\(preset.title))")
        case .projectDefault(nil): String(localized: "Use Project Default (Custom)")
        }
    }

    var branchPrefix: String {
        if case let .text(prefix)? = scopeSnapshot?.value(for: Self.branchPrefixKey) { return prefix }
        return Self.builtInBranchPrefix
    }

    var branchPrefixIsExplicit: Bool {
        scopeSnapshot?.source(for: Self.branchPrefixKey) == .scope(settingScope)
    }

    /// What Reset to Default returns to. Only projects and tasks store a prefix.
    var defaultBranchPrefix: String {
        if case let .text(prefix)? = planeSnapshot?.value(for: Self.branchPrefixKey) { return prefix }
        return Self.builtInBranchPrefix
    }

    var automaticNaming: Bool {
        if case let .flag(value)? = scopeSnapshot?.value(for: Self.automaticNamingKey) { return value }
        return true
    }

    var automaticNamingIsExplicit: Bool {
        scopeSnapshot?.source(for: Self.automaticNamingKey) == .scope(settingScope)
    }

    var defaultAutomaticNaming: Bool {
        if case let .flag(value)? = planeSnapshot?.value(for: Self.automaticNamingKey) { return value }
        return true
    }

    /// The flags a choice leads to.
    func targetFlags(for choice: ReplyFinishChoice) -> ReplyFinishFlags? {
        switch choice {
        case let .preset(preset): preset.flags
        case .projectDefault: projectFlags
        case .custom: nil
        }
    }

    /// The push confirmation's message for the pending choice.
    var pushConfirmationMessage: String {
        guard let pendingConfirmation, let target = targetFlags(for: pendingConfirmation) else { return "" }
        return target.draftPullRequest
            ? String(localized: "Jet pushes each reply's changes to origin and opens or updates a draft pull request on GitHub without asking.")
            : String(localized: "Jet pushes each reply's changes to origin without asking.")
    }

    // MARK: Planning

    /// The writes for a choice. Writes that turn something off come first, in
    /// reverse policy order; then writes that turn something on, in policy order;
    /// then writes that change nothing. A partial run never enables more than the
    /// chosen preset.
    static func writes(current: ReplyFinishScopeState, target: ReplyFinishTarget) -> [PolicyWrite] {
        var offs: [PolicyWrite] = []
        var ons: [PolicyWrite] = []
        var unchanged: [PolicyWrite] = []
        for (index, key) in ReplyFinishFlags.keys.enumerated() {
            let old = current.effective.values[index]
            let write: PolicyWrite
            let new: Bool
            switch target {
            case let .projectPreset(flags):
                new = flags.values[index]
                guard old != new else { continue }
                write = .set(key, .flag(new))
            case let .taskPreset(flags):
                new = flags.values[index]
                guard !(current.explicit[index] && old == new) else { continue }
                write = .set(key, .flag(new))
            case let .projectDefault(flags):
                new = flags.values[index]
                guard current.explicit[index] else { continue }
                write = .clear(key)
            }
            if old && !new {
                offs.append(write)
            } else if !old && new {
                ons.append(write)
            } else {
                unchanged.append(write)
            }
        }
        return offs.reversed() + ons + unchanged
    }

    /// Pushing or opening a draft pull request that wasn't on before needs its own confirmation.
    static func needsPushConfirmation(from current: ReplyFinishFlags, to target: ReplyFinishFlags) -> Bool {
        (!current.push && target.push) || (!current.draftPullRequest && target.draftPullRequest)
    }

    /// Branch names reach Git, so the characters Git refuses are caught before sending.
    static func branchPrefixProblem(_ prefix: String) -> String? {
        let forbidden = CharacterSet(charactersIn: "~^:?*[\\")
            .union(.whitespacesAndNewlines)
            .union(.controlCharacters)
        guard !prefix.unicodeScalars.contains(where: forbidden.contains) else {
            return String(localized: "Branch names can't contain spaces or ~ ^ : ? * [ \\.")
        }
        return nil
    }

    // MARK: Loading

    func load() async {
        guard !isSeeded else { return }
        if scopeSnapshot == nil { phase = .loading }
        do {
            let access = try await makeAccess(planeRegistryID)
            try await read(access)
            phase = .loaded
            isOffline = false
            if uncertain == nil, retry == nil {
                issue = nil
                issueRow = nil
            }
        } catch {
            guard !(error is CancellationError) else { return }
            isOffline = DesktopSession.presentationError(error).category == .offline
            issue = LibraryIssue.from(error, computer: computer, isLocal: isLocal)
            issueRow = nil
            if scopeSnapshot == nil { phase = .unavailable }
        }
    }

    private func read(_ access: any JetLibraryAccess) async throws {
        let scoped = try await access.settings(scope: settingScope)
        switch scope {
        case .project:
            planeSnapshot = (try? await access.settings(scope: .plane)) ?? planeSnapshot
        case let .task(_, projectID?):
            projectSnapshot = (try? await access.settings(scope: .project(projectID))) ?? projectSnapshot
        case .task(_, nil):
            break
        }
        scopeSnapshot = scoped
        if branchPrefixError == nil { branchPrefixDraft = branchPrefix }
    }

    // MARK: Changing

    /// Chooses an option. One that newly pushes asks first (`pendingConfirmation`).
    func choose(_ choice: ReplyFinishChoice) async {
        guard choice != self.choice, choice != .custom, !isBusy,
              let current = state?.effective, let target = targetFlags(for: choice)
        else { return }
        if Self.needsPushConfirmation(from: current, to: target) {
            pendingConfirmation = choice
            return
        }
        await apply(choice)
    }

    /// Push Automatically.
    func confirmPending() async {
        guard let choice = pendingConfirmation else { return }
        pendingConfirmation = nil
        await apply(choice)
    }

    func cancelPending() {
        pendingConfirmation = nil
    }

    private func apply(_ choice: ReplyFinishChoice) async {
        guard let state else { return }
        let target: ReplyFinishTarget
        switch (scope, choice) {
        case let (.project, .preset(preset)):
            target = .projectPreset(preset.flags)
        case let (.task, .preset(preset)):
            target = .taskPreset(preset.flags)
        case (.task, .projectDefault):
            guard let projectFlags = projectSnapshot.map(ReplyFinishFlags.init(snapshot:)) ?? projectFlags
            else { return }
            target = .projectDefault(projectFlags)
        default:
            return
        }
        await perform(Self.writes(current: state, target: target), row: .replyFinish, resume: .choice(choice))
    }

    /// Applies "Branch names start with" on Return or when the field loses focus.
    func setBranchPrefix(_ prefix: String) async {
        if let problem = Self.branchPrefixProblem(prefix) {
            branchPrefixError = problem
            return
        }
        branchPrefixError = nil
        guard !isBusy, scopeSnapshot != nil, prefix != branchPrefix else { return }
        await perform(
            [.set(Self.branchPrefixKey, .text(prefix))],
            row: .branchPrefix,
            resume: .branchPrefix(prefix)
        )
    }

    func setAutomaticNaming(_ isOn: Bool) async {
        guard !isBusy, scopeSnapshot != nil, isOn != automaticNaming else { return }
        await perform(
            [.set(Self.automaticNamingKey, .flag(isOn))],
            row: .automaticNaming,
            resume: .automaticNaming(isOn)
        )
    }

    func resetBranchPrefix() async {
        guard !isBusy, branchPrefixIsExplicit else { return }
        branchPrefixError = nil
        await perform([.clear(Self.branchPrefixKey)], row: .branchPrefix, resume: .resetBranchPrefix)
    }

    func resetAutomaticNaming() async {
        guard !isBusy, automaticNamingIsExplicit else { return }
        await perform([.clear(Self.automaticNamingKey)], row: .automaticNaming, resume: .resetAutomaticNaming)
    }

    /// Try Again: reloads, or goes on with what stopped. Steps already saved aren't repeated.
    func tryAgain() async {
        guard let retry else {
            await load()
            return
        }
        self.retry = nil
        await resume(retry.resume)
    }

    /// Send Same Request Again: the uncertain write with its original Command ID and body.
    func sendSameRequestAgain() async {
        guard let uncertain, !isBusy else { return }
        savingRow = uncertain.row
        issue = nil
        issueRow = nil
        defer { savingRow = nil }
        do {
            let access = try await makeAccess(planeRegistryID)
            let succeeded = await send(
                uncertain.write,
                commandID: uncertain.commandID,
                access: access,
                row: uncertain.row,
                index: 0,
                resume: uncertain.resume
            )
            guard succeeded else { return }
            self.uncertain = nil
            savingRow = nil
            await resume(uncertain.resume)
        } catch {
            fail(error, row: uncertain.row, savedSome: false, resume: uncertain.resume)
        }
    }

    private func resume(_ resume: Resume) async {
        await load()
        switch resume {
        case let .choice(choice): await apply(choice)
        case let .branchPrefix(prefix): await setBranchPrefix(prefix)
        case let .automaticNaming(isOn): await setAutomaticNaming(isOn)
        case .resetBranchPrefix: await resetBranchPrefix()
        case .resetAutomaticNaming: await resetAutomaticNaming()
        }
    }

    /// Sends the writes one at a time and stops at the first that fails or can't be confirmed.
    private func perform(_ writes: [PolicyWrite], row: Row, resume: Resume) async {
        guard !writes.isEmpty, !isBusy else { return }
        savingRow = row
        issue = nil
        issueRow = nil
        retry = nil
        defer { savingRow = nil }
        let access: any JetLibraryAccess
        do {
            access = try await makeAccess(planeRegistryID)
        } catch {
            fail(error, row: row, savedSome: false, resume: resume)
            return
        }
        for (index, write) in writes.enumerated() {
            let succeeded = await send(write, commandID: UUID(), access: access, row: row, index: index, resume: resume)
            guard succeeded else { return }
        }
        try? await read(access)
    }

    /// One write. On an unknown outcome one confirming Query decides; otherwise the
    /// write is kept for Send Same Request Again and nothing more is sent.
    private func send(
        _ write: PolicyWrite,
        commandID: UUID,
        access: any JetLibraryAccess,
        row: Row,
        index: Int,
        resume: Resume
    ) async -> Bool {
        do {
            switch write {
            case let .set(key, value):
                _ = try await access.setSetting(key, value: value, scope: settingScope, commandID: commandID)
            case let .clear(key):
                _ = try await access.clearSetting(key, scope: settingScope, commandID: commandID)
            }
            return true
        } catch let error where LibraryIssue.isOutcomeUnknown(error) {
            try? await read(access)
            if shows(write) { return true }
            uncertain = UncertainWrite(write: write, commandID: commandID, row: row, resume: resume)
            issue = LibraryIssue(
                kind: .warning,
                text: String(localized: "Jet couldn't confirm this change."),
                action: .sendSameRequestAgain,
                code: LibraryIssue.code(of: error)
            )
            issueRow = row
            return false
        } catch {
            try? await read(access)
            fail(error, row: row, savedSome: index > 0, resume: resume)
            return false
        }
    }

    private func fail(_ error: Error, row: Row, savedSome: Bool, resume: Resume) {
        if uncertain?.resume == resume { uncertain = nil }
        let failure = DesktopSession.presentationError(error)
        issueRow = row
        if failure.category == .conflict {
            issue = LibraryIssue(
                kind: .info,
                text: String(localized: "This setting changed in another Jet app. Jet refreshed it."),
                code: failure.code
            )
            return
        }
        isOffline = failure.category == .offline
        retry = (row, resume)
        if savedSome {
            issue = LibraryIssue(
                kind: .error,
                text: String(localized: "Couldn't finish changing this setting. Some steps were saved."),
                action: .tryAgain,
                code: failure.code
            )
        } else {
            issue = LibraryIssue.from(error, computer: computer, isLocal: isLocal)
        }
    }

    /// Whether the scope's snapshot shows the write took effect.
    private func shows(_ write: PolicyWrite) -> Bool {
        guard let scopeSnapshot else { return false }
        switch write {
        case let .set(key, value):
            return scopeSnapshot.value(for: key) == value && scopeSnapshot.source(for: key) == .scope(settingScope)
        case let .clear(key):
            return scopeSnapshot.source(for: key) != .scope(settingScope)
        }
    }

#if DEBUG
    /// A fixed state for previews and screenshots; `load` then does nothing.
    func seedForPreview(
        scopeSnapshot: JetSettingSnapshot?,
        projectSnapshot: JetSettingSnapshot? = nil,
        planeSnapshot: JetSettingSnapshot? = nil,
        phase: Phase = .loaded,
        isOffline: Bool = false,
        issue: LibraryIssue? = nil,
        issueRow: Row? = nil,
        savingRow: Row? = nil
    ) {
        isSeeded = true
        self.scopeSnapshot = scopeSnapshot
        self.projectSnapshot = projectSnapshot
        self.planeSnapshot = planeSnapshot
        self.phase = phase
        self.isOffline = isOffline
        self.issue = issue
        self.issueRow = issueRow
        self.savingRow = savingRow
        branchPrefixDraft = branchPrefix
    }
#endif
}

// MARK: - Shared picker

/// The "When a Reply Finishes" choices with a caption for the selected one. Used
/// by the project page and Task Settings.
struct ReplyFinishSection: View {
    @Bindable var model: ReplyFinishModel
    let projectName: String
    /// The computer that holds the GitHub token, for "How to Set Up…".
    let computerName: String
    let isDisabled: Bool
    let perform: @MainActor (LibraryIssue.Action) -> Void

    var body: some View {
        if let choice = model.choice {
            Picker("When a Reply Finishes", selection: selection(current: choice)) {
                ForEach(model.choices, id: \.self) { option in
                    Text(model.title(for: option))
                        // The radio circles alone dim too faintly to read as disabled.
                        .foregroundStyle(isDisabled ? AnyShapeStyle(.secondary) : AnyShapeStyle(.primary))
                        .tag(option)
                        .disabled(option == .custom)
                }
            }
            .labelsHidden()
#if os(macOS)
            .pickerStyle(.radioGroup)
#else
            .pickerStyle(.inline)
#endif
            .disabled(isDisabled || model.isBusy)
            .accessibilityIdentifier("project-reply-finish")

            VStack(alignment: .leading, spacing: 4) {
                ForEach(captions(for: choice), id: \.self) { line in
                    Text(line)
                }
                if model.savingRow == .replyFinish {
                    SavingLabel()
                }
            }
            .font(.callout)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)

            // Outside the caption stack, so its popover doesn't inherit the dimmed style.
            if choice == .preset(.saveAndOpenPullRequest) {
                GitHubSetupButton(computer: computerName)
                    .controlSize(.small)
            }

            if model.issueRow == .replyFinish, let issue = model.issue {
                LibraryNoticeRow(issue: issue, perform: { perform($0) })
            }
        } else if model.phase == .loading {
            Text("Loading…").foregroundStyle(.secondary)
        }
    }

    private func selection(current: ReplyFinishChoice) -> Binding<ReplyFinishChoice> {
        Binding(
            get: { current },
            set: { choice in Task { await model.choose(choice) } }
        )
    }

    /// The selected option's caption. Presets that build on saving to a branch
    /// repeat what that does, so each caption stands on its own.
    private func captions(for choice: ReplyFinishChoice) -> [String] {
        let branch = String(localized: "After each reply with changes, Jet commits them to a new \(model.branchPrefix) branch in \(projectName). Your checked-out files don't change.")
        let push = String(localized: "Also pushes the branch to origin.")
        let pullRequest = String(localized: "Also opens or updates a draft pull request on GitHub. Needs a GitHub token saved for Jet in your Keychain.")
        switch choice {
        case .preset(.doNothing):
            return [String(localized: "Changes stay in the task's working copy until you choose Keep Changes….")]
        case .preset(.saveToBranch):
            return [branch]
        case .preset(.saveAndPush):
            return [branch, push]
        case .preset(.saveAndOpenPullRequest):
            return [branch, push, pullRequest]
        case .custom:
            return [model.isTask
                ? String(localized: "This task uses its own mix of Git steps. Choose an option to replace it.")
                : String(localized: "This project uses its own mix of Git steps. Choose an option to replace it.")]
        case .projectDefault:
            return [String(localized: "Follows \(projectName)'s setting.")]
        }
    }
}

/// A small spinner with "Saving…".
struct SavingLabel: View {
    var body: some View {
        HStack(spacing: 6) {
            ProgressView().controlSize(.small)
            Text("Saving…")
        }
        .accessibilityElement(children: .combine)
    }
}

extension View {
    /// The push confirmation for a choice that newly pushes (design §6.11).
    func replyFinishPushConfirmation(_ model: ReplyFinishModel) -> some View {
        alert(
            "Push automatically after every reply?",
            isPresented: Binding(
                get: { model.pendingConfirmation != nil },
                set: { if !$0 { model.cancelPending() } }
            )
        ) {
            Button("Push Automatically") { Task { await model.confirmPending() } }
            Button("Cancel", role: .cancel) { model.cancelPending() }
        } message: {
            Text(model.pushConfirmationMessage)
        }
    }
}

// MARK: - Task Settings sheet

/// Task Settings…: this task's "When a Reply Finishes". Changes apply at once.
struct TaskSettingsSheet: View {
    let session: DesktopSession
    let ref: ConversationRef
    @State private var model: ReplyFinishModel

    init(session: DesktopSession, ref: ConversationRef, model: ReplyFinishModel? = nil) {
        self.session = session
        self.ref = ref
        let projectID = session.conversations.first { $0.id == ref.conversationID }?.projectID
        _model = State(initialValue: model ?? ReplyFinishModel(
            planeRegistryID: ref.planeRegistryID,
            scope: .task(conversationID: ref.conversationID, projectID: projectID),
            makeAccess: session.libraryAccessProvider,
            computer: session.planeDisplayName(ref.planeRegistryID),
            isLocal: session.isLocalPlane(ref.planeRegistryID)
        ))
    }

    private var summary: JetConversationSummary? {
        session.conversations.first { $0.id == ref.conversationID }
    }

    private var taskTitle: String {
        let title = summary?.title.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return title.isEmpty ? LibraryCopy.untitledTask : title
    }

    private var projectName: String {
        session.projectName(for: ref.conversationID) ?? String(localized: "the project")
    }

    private var isOffline: Bool {
        model.isOffline || session.isComputerOffline(ref.planeRegistryID)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: JetDesign.gap) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Task Settings")
                    .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
                Text(taskTitle)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }

            if isOffline {
                LibraryNoticeRow(
                    issue: LibraryIssue(
                        kind: .warning,
                        text: String(localized: "Can't reach \(session.planeDisplayName(ref.planeRegistryID)). Settings can't be changed right now."),
                        action: .tryAgain
                    ),
                    perform: { _ in Task { await model.load() } }
                )
            } else if model.issueRow == nil, let issue = model.issue {
                LibraryNoticeRow(issue: issue, perform: perform)
            }

            VStack(alignment: .leading, spacing: JetDesign.smallGap) {
                Text("When a Reply Finishes")
                    .font(.headline)
                ReplyFinishSection(
                    model: model,
                    projectName: projectName,
                    computerName: session.planeDisplayName(ref.planeRegistryID),
                    isDisabled: isOffline,
                    perform: perform
                )
            }

            Text("Applies to replies that finish from now on.")
                .font(.caption)
                .foregroundStyle(.secondary)

            HStack {
                Spacer()
                Button("Done") { session.dismissSheet() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(JetDesign.sectionGap)
        .frame(width: 480, alignment: .leading)
        .accessibilityIdentifier("task-settings-sheet")
        .replyFinishPushConfirmation(model)
        .task { await model.load() }
    }

    private func perform(_ action: LibraryIssue.Action) {
        switch action {
        case .tryAgain: Task { await model.tryAgain() }
        case .sendSameRequestAgain: Task { await model.sendSameRequestAgain() }
        case let .review(pane): session.perform(.openSettings(pane))
        default: break
        }
    }
}

#if DEBUG
#Preview("Task Settings") {
    DesktopPreviewScenes.view("library-task-settings")
        .frame(width: 480, height: 420)
}
#endif
