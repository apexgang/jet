import SwiftUI

/// Details › Changes: the scope, a summary and the changed files with their diffs
/// (design §6.8).
struct DetailsChangesView: View {
    @Bindable var session: DesktopSession
    @Bindable var model: DetailsPanelModel

    @Namespace private var rotorNamespace
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private static let captionSize = JetDesign.TextSize.metadata

    var body: some View {
        VStack(spacing: 0) {
            if session.selectedRun != nil {
                ChangesScopeBar(session: session, model: model)
                    .padding(.horizontal, 12)
                    .padding(.top, 4)
                    .padding(.bottom, 8)
            }
            stateContent
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
        .onChange(of: session.latestReplyNumber) { _, latest in
            // Last Reply follows new replies as they finish.
            guard model.followsLastReply, latest > 0, session.changesScopeChoice != .lastReply else { return }
            Task { await session.selectChangesScope(.lastReply) }
        }
        .onChange(of: expansionIdentity, initial: true) { _, identity in
            expandFirstFileIfNeeded(identity)
        }
        .onChange(of: session.selectedWorkFilePath) { _, path in
            guard let path else { return }
            if model.selectionFromRow == path {
                model.selectionFromRow = nil
                return
            }
            model.expandedPaths.insert(path)
        }
    }

    private var assistant: String {
        DetailsCopy.assistant(session.selectedAssistantName, position: .mid)
    }

    // MARK: States

    @ViewBuilder
    private var stateContent: some View {
        if session.selectedRun == nil {
            ContentUnavailableView(
                "No Changes Yet",
                systemImage: "checkmark.circle",
                description: Text("Changes \(assistant) makes appear here.")
            )
        } else if model.followsLastReply, session.workDiff != nil, session.latestReplyNumber == 0 {
            ContentUnavailableView(
                "No Finished Replies Yet",
                systemImage: "clock",
                description: Text("Changes appear here after \(assistant) finishes a reply.")
            )
        } else if let diff = session.workDiff {
            changes(diff)
        } else if let error = session.workError {
            loadError(error)
        } else {
            ProgressView("Loading changes…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func loadError(_ error: JetPresentationError) -> some View {
        ContentUnavailableView {
            Label("Couldn't Load Changes", systemImage: "exclamationmark.triangle")
        } description: {
            Text(errorText(error))
        } actions: {
            Button("Try Again") { Task { await session.loadWorkPanel() } }
                .disabled(session.workOperation != nil)
            ForEach(error.recoveryActions) { action in
                Button(action.detailsTitle) { Task { await session.applyWorkRecovery(action) } }
            }
            JetSettingsRecoveryButton(session: session, error: error)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func errorText(_ error: JetPresentationError) -> String {
        DetailsCopy.error(
            error,
            computer: session.selectedPlaneName,
            assistant: session.selectedAssistantName,
            action: .other,
            fallback: session.plainMessage(for: error)
        )
    }

    // MARK: Changes

    @ViewBuilder
    private func changes(_ diff: JetChangeDiff) -> some View {
        let split = model.split(patch: session.workPatch, isComplete: session.isWorkPatchComplete, key: diff)
        if session.workFiles.isEmpty, session.workNextPage == nil {
            VStack(spacing: 0) {
                captions(diff)
                emptyFiles
                    .frame(maxHeight: .infinity)
            }
        } else {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    captions(diff)
                    summary(diff, split: split)
                        .id("changes-summary")
                    ForEach(session.workFiles) { file in
                        fileSection(file, diff: diff, split: split)
                            .id("file-\(file.path)")
                    }
                    if session.workNextPage != nil {
                        HStack(spacing: 8) {
                            Button("Show More Files") { Task { await session.loadMoreWorkFiles() } }
                                .disabled(session.workOperation != nil || session.detailsIsOffline)
                            if session.workOperation == "files" {
                                ProgressView().controlSize(.small)
                            }
                        }
                        .padding(12)
                    }
                }
                .scrollTargetLayout()
                .padding(.bottom, 8)
            }
            .scrollPosition(id: scrollAnchor)
            .accessibilityRotor("Changed Files") {
                ForEach(session.workFiles) { file in
                    AccessibilityRotorEntry(Text(verbatim: file.path), id: file.path, in: rotorNamespace)
                }
            }
        }
    }

    @ViewBuilder
    private var emptyFiles: some View {
        if session.changesScopeChoice == .all {
            ContentUnavailableView(
                "No Changes Yet",
                systemImage: "checkmark.circle",
                description: Text("Changes \(assistant) makes appear here.")
            )
        } else {
            ContentUnavailableView(
                "No Changes",
                systemImage: "checkmark.circle",
                description: Text("No files changed in this part of the task.")
            )
        }
    }

    @ViewBuilder
    private func captions(_ diff: JetChangeDiff) -> some View {
        let refreshFailed = session.workError != nil && !session.detailsIsOffline
        if session.changesSpanSeveralRuns || !diff.contentComplete || session.detailsIsOffline || refreshFailed {
            VStack(alignment: .leading, spacing: 6) {
                if session.detailsIsOffline {
                    Label("Offline · showing saved view", systemImage: "wifi.slash")
                }
                if session.changesSpanSeveralRuns {
                    Text("Showing changes since \(assistant) last started for this task.")
                }
                if !diff.contentComplete {
                    Text("Some very large files are listed without their changes.")
                }
                if refreshFailed {
                    DetailsNoticeView(kind: .error, text: String(localized: "Couldn't refresh changes.")) {
                        Button("Try Again") { Task { await session.loadWorkPanel() } }
                            .disabled(session.workOperation != nil)
                    }
                    .padding(.top, 2)
                }
            }
            .font(.system(size: Self.captionSize))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
        }
    }

    private func summary(_ diff: JetChangeDiff, split: DiffSplitResult) -> some View {
        let added = Text(verbatim: "+\(split.additions)").foregroundStyle(.green)
        let removed = Text(verbatim: "\u{2212}\(split.deletions)").foregroundStyle(.red)
        return HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text("^[\(Int(diff.totalFiles)) file](inflect: true) changed")
                .font(.system(size: JetDesign.TextSize.navigation, weight: .semibold))
            Group {
                if session.isWorkPatchComplete {
                    Text("\(added) \(removed)")
                } else {
                    Text("at least \(added) \(removed)")
                        .foregroundStyle(.secondary)
                }
            }
            .font(.system(size: JetDesign.TextSize.control).monospacedDigit())
            Spacer(minLength: 0)
            if session.workOperation == "refresh" {
                ProgressView().controlSize(.mini)
            }
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 6)
        .accessibilityElement(children: .combine)
    }

    // MARK: Files

    private func fileSection(_ file: JetChangedFile, diff: JetChangeDiff, split: DiffSplitResult) -> some View {
        let isExpanded = model.expandedPaths.contains(file.path)
        return VStack(alignment: .leading, spacing: 0) {
            ChangedFileRow(
                file: file,
                isExpanded: isExpanded,
                isSelected: session.selectedWorkFilePath == file.path,
                assistant: assistant
            ) {
                toggle(file)
            }
            .contextMenu { fileActions(file, split: split) }
            .accessibilityActions { fileAccessibilityActions(file, split: split) }
            .accessibilityRotorEntry(id: file.path, in: rotorNamespace)
            .popover(isPresented: commentBinding(file.path), arrowEdge: .leading) {
                CommentOnLineForm(session: session, path: file.path) {
                    model.commentPath = nil
                }
            }
            .accessibilityIdentifier("changed-file-\(file.path)")

            if isExpanded {
                let availability = DiffSplit.availability(for: file, in: split, artifact: diff.artifact)
                ChangedFileDiffHeader(availability: availability) {
                    fileActions(file, split: split)
                }
                if availability.showsLines {
                    FileDiffView(
                        file: file,
                        availability: availability,
                        session: session,
                        model: model
                    ) { line in
                        openComment(file.path, line: line, split: split)
                    }
                }
                Spacer().frame(height: 8)
            }
        }
    }

    @ViewBuilder
    private func fileActions(_ file: JetChangedFile, split: DiffSplitResult) -> some View {
        if session.canEditChangedFile(file) {
            Button("Edit File") { Task { await session.beginEditingFile(file.path) } }
        }
        Button("Comment on Line…") { openComment(file.path, line: nil, split: split) }
        Divider()
        Button("Copy Path") { session.copyChangedFilePath(file) }
        if session.changedFileURL(file) != nil {
            Button("Show in Finder") { session.revealChangedFile(file) }
        }
    }

    @ViewBuilder
    private func fileAccessibilityActions(_ file: JetChangedFile, split: DiffSplitResult) -> some View {
        if session.canEditChangedFile(file) {
            Button("Edit File") { Task { await session.beginEditingFile(file.path) } }
        }
        Button("Comment on Line…") { openComment(file.path, line: nil, split: split) }
        Button("Copy Path") { session.copyChangedFilePath(file) }
        if session.changedFileURL(file) != nil {
            Button("Show in Finder") { session.revealChangedFile(file) }
        }
    }

    private func toggle(_ file: JetChangedFile) {
        let path = file.path
        var transaction = Transaction()
        transaction.disablesAnimations = reduceMotion
        withTransaction(transaction) {
            if model.expandedPaths.contains(path) {
                model.expandedPaths.remove(path)
            } else {
                model.expandedPaths.insert(path)
            }
        }
        if session.selectedWorkFilePath != path {
            model.selectionFromRow = path
            session.selectedWorkFilePath = path
        }
    }

    /// Opens Comment on Line… for a file. The line comes from the clicked diff
    /// line, else the first added line, else 1. A comment kept for a retry is
    /// never changed, so sending it again stays the same request.
    private func openComment(_ path: String, line: UInt32?, split: DiffSplitResult) {
        let keepsRetainedComment = session.pendingReview?.path == path && line == nil
        if !keepsRetainedComment {
            if model.commentDraftPath != path { session.reviewComment = "" }
            let firstAdded = split.files[path]?.hunks
                .lazy.flatMap(\.lines)
                .first { $0.kind == .added }?.newNumber
            session.reviewLine = line ?? firstAdded.map { UInt32($0) } ?? 1
        }
        model.commentDraftPath = path
        model.commentPath = path
    }

    private func commentBinding(_ path: String) -> Binding<Bool> {
        Binding(
            get: { model.commentPath == path },
            set: { presented in
                if !presented, model.commentPath == path { model.commentPath = nil }
            }
        )
    }

    /// The Run, scope and first file: when it changes and none of the listed files
    /// is expanded, the first one expands.
    private var expansionIdentity: String {
        guard let diff = session.workDiff else { return "" }
        return "\(diff.runID.uuidString)|\(diff.scope.label)|\(session.workFiles.first?.path ?? "")"
    }

    private func expandFirstFileIfNeeded(_ identity: String) {
        guard !identity.isEmpty, identity != model.expansionIdentity else { return }
        model.expansionIdentity = identity
        let listed = Set(session.workFiles.map(\.path))
        guard model.expandedPaths.isDisjoint(with: listed), let first = session.workFiles.first?.path else { return }
        model.expandedPaths = [first]
    }

    private var scrollAnchor: Binding<String?> {
        Binding(
            get: { session.workScrollAnchors[.changes] },
            set: { session.workScrollAnchors[.changes] = $0 }
        )
    }
}

// MARK: - Scope bar

/// "All Changes | Last Reply" plus the Earlier menu. Choices apply at once.
struct ChangesScopeBar: View {
    @Bindable var session: DesktopSession
    @Bindable var model: DetailsPanelModel

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) {
                segments
                earlier
            }
            VStack(alignment: .leading, spacing: 8) {
                segments
                earlier
            }
        }
        .controlSize(.small)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var segments: some View {
        Picker("Changes", selection: segmentBinding) {
            Text("All Changes").tag(ChangesSegment?.some(.all))
            Text("Last Reply").tag(ChangesSegment?.some(.lastReply))
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
        .accessibilityIdentifier("changes-scope")
    }

    private var earlier: some View {
        Menu {
            let latest = session.latestReplyNumber
            if latest == 0 {
                Button("No finished replies yet") {}
                    .disabled(true)
            } else {
                ForEach(replyNumbers(latest: latest), id: \.self) { reply in
                    Toggle(isOn: toggleBinding(.reply(reply))) {
                        Text("Reply \(reply)")
                        if let recordedAt = session.replyRecordedAt(reply) {
                            Text(verbatim: JetCopy.relative(ms: recordedAt))
                        }
                    }
                }
                Divider()
                Toggle(isOn: toggleBinding(.whenStopped)) {
                    Text(verbatim: ChangesScopeChoice.whenStopped.title(assistant: session.selectedAssistantName))
                }
                .disabled(session.selectedRun?.lifecycle.isLive ?? true)
                Divider()
                Button("Custom Range…") { model.rangeEditorShown = true }
            }
        } label: {
            Text(verbatim: earlierTitle)
        }
        .fixedSize()
        .accessibilityIdentifier("changes-earlier")
        .popover(isPresented: $model.rangeEditorShown, arrowEdge: .bottom) {
            ChangesRangeForm(
                latest: session.latestReplyNumber,
                initialFrom: initialRange.from,
                initialTo: initialRange.to,
                onShow: { from, to in
                    model.rangeEditorShown = false
                    choose(.range(fromReply: from, toReply: to))
                },
                onCancel: { model.rangeEditorShown = false }
            )
            // WP8: .focusedSceneValue(\.hasOpenDialog, true) once WP5 defines the key (critic 4.8).
        }
    }

    private var earlierTitle: String {
        let choice = session.changesScopeChoice
        if choice.segment == nil, !(model.followsLastReply && session.latestReplyNumber == 0) {
            return choice.title(assistant: session.selectedAssistantName)
        }
        return String(localized: "Earlier")
    }

    /// The latest twelve replies, oldest first.
    private func replyNumbers(latest: UInt32) -> [UInt32] {
        let first = latest > 12 ? latest - 11 : 1
        return Array(first ... latest)
    }

    private var initialRange: (from: UInt32, to: UInt32) {
        let latest = max(1, session.latestReplyNumber)
        switch session.changesScopeChoice {
        case let .range(from, to): return (from, to)
        case let .reply(reply): return (reply, reply)
        case .all, .lastReply, .whenStopped: return (max(1, latest - 1), latest)
        }
    }

    private var segmentBinding: Binding<ChangesSegment?> {
        Binding(
            get: {
                if model.followsLastReply, session.latestReplyNumber == 0 { return .lastReply }
                return session.changesScopeChoice.segment
            },
            set: { segment in
                guard let segment else { return }
                choose(segment == .all ? .all : .lastReply)
            }
        )
    }

    private func toggleBinding(_ choice: ChangesScopeChoice) -> Binding<Bool> {
        Binding(
            get: { isCurrent(choice) },
            set: { isOn in
                if isOn { choose(choice) }
            }
        )
    }

    private func isCurrent(_ choice: ChangesScopeChoice) -> Bool {
        let current = session.changesScopeChoice
        if current == choice { return true }
        if case let .reply(reply) = choice, current == .lastReply {
            return reply == session.latestReplyNumber
        }
        return false
    }

    private func choose(_ choice: ChangesScopeChoice) {
        model.chosenScope = choice
        model.followsLastReply = choice == .lastReply
        Task { await session.selectChangesScope(choice) }
    }
}

/// Custom Range…: the first and last reply to show.
struct ChangesRangeForm: View {
    let latest: UInt32
    let onShow: (UInt32, UInt32) -> Void
    let onCancel: () -> Void

    @State private var from: UInt32
    @State private var to: UInt32

    init(
        latest: UInt32,
        initialFrom: UInt32,
        initialTo: UInt32,
        onShow: @escaping (UInt32, UInt32) -> Void,
        onCancel: @escaping () -> Void
    ) {
        self.latest = max(1, latest)
        self.onShow = onShow
        self.onCancel = onCancel
        let to = min(max(1, initialTo), max(1, latest))
        _to = State(initialValue: to)
        _from = State(initialValue: min(max(1, initialFrom), to))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Custom Range")
                .font(.headline)
            Stepper("From Reply \(from)", value: $from, in: 1 ... max(1, to))
            Stepper("To Reply \(to)", value: $to, in: from ... max(from, latest))
            HStack(spacing: 8) {
                Spacer(minLength: 0)
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("Show Changes") { onShow(from, to) }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(16)
        .frame(width: 260)
    }
}

// MARK: - File row

/// One changed file: a chevron, its status symbol and word, and the path
/// truncated in the middle.
struct ChangedFileRow: View {
    let file: JetChangedFile
    let isExpanded: Bool
    let isSelected: Bool
    let assistant: String
    let action: () -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    static let symbolWidth: CGFloat = 18
    /// Where the path starts, for rows under it: padding, chevron, symbol and gaps.
    static let pathInset: CGFloat = 6 + 6 + 12 + 6 + symbolWidth + 6

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .rotationEffect(.degrees(isExpanded ? 90 : 0))
                    .animation(reduceMotion ? nil : .snappy(duration: 0.15), value: isExpanded)
                    .frame(width: 12)
                Image(systemName: status.symbol)
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(status.tint)
                    .font(.system(size: JetDesign.TextSize.navigation))
                    .frame(width: ChangedFileRow.symbolWidth)
                Text(verbatim: file.path)
                    .font(.system(size: JetDesign.TextSize.navigation))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 4)
                Text(status.word)
                    .font(.system(size: JetDesign.TextSize.metadata))
                    .foregroundStyle(.secondary)
                    .fixedSize()
            }
            .padding(.horizontal, 6)
            .frame(minHeight: 28)
            .contentShape(Rectangle())
            .background {
                if isSelected {
                    RoundedRectangle(cornerRadius: JetDesign.controlRadius)
                        .fill(JetDesign.accent.opacity(0.12))
                }
            }
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 6)
        .help("\(file.path) · \(origin)")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(accessibilityLabel))
        .accessibilityValue(Text(isExpanded ? String(localized: "Expanded") : String(localized: "Collapsed")))
        .accessibilityAddTraits(.isButton)
    }

    private var status: ChangedFileStatus { ChangedFileStatus(file) }

    /// Who changed the file, for its tooltip.
    private var origin: String {
        switch file.origin {
        case "agent": String(localized: "changed by \(assistant)")
        case "user edit": String(localized: "changed by you")
        case "terminal": String(localized: "changed in a terminal")
        case "mixed": String(localized: "changed by more than one source")
        default: String(localized: "changed outside Jet")
        }
    }

    /// "login.ts, Modified, src/auth".
    private var accessibilityLabel: String {
        let url = URL(fileURLWithPath: file.path)
        let name = url.lastPathComponent
        let folder = (file.path as NSString).deletingLastPathComponent
        return folder.isEmpty ? "\(name), \(status.word)" : "\(name), \(status.word), \(folder)"
    }
}

/// A changed file's status: symbol, tint and word.
struct ChangedFileStatus {
    let symbol: String
    let tint: AnyShapeStyle
    let word: String

    init(_ file: JetChangedFile) {
        switch file.status {
        case "added":
            symbol = "plus.circle"
            tint = AnyShapeStyle(Color.green)
            word = String(localized: "Added")
        case "deleted":
            symbol = "minus.circle"
            tint = AnyShapeStyle(Color.red)
            word = String(localized: "Deleted")
        default:
            symbol = "pencil.circle"
            tint = AnyShapeStyle(.secondary)
            word = String(localized: "Modified")
        }
    }
}

/// The row above an expanded diff: the file's counts, or why no lines show, and
/// its actions for keyboard-only use.
struct ChangedFileDiffHeader<Actions: View>: View {
    let availability: DiffFileAvailability
    let actions: Actions

    init(availability: DiffFileAvailability, @ViewBuilder actions: () -> Actions) {
        self.availability = availability
        self.actions = actions()
    }

    var body: some View {
        HStack(spacing: 8) {
            summary
                .font(.system(size: JetDesign.TextSize.metadata))
            Spacer(minLength: 0)
            Menu {
                actions
            } label: {
                Label("File Actions", systemImage: "ellipsis.circle")
                    .labelStyle(.iconOnly)
            }
            .menuStyle(.button)
            .buttonStyle(.borderless)
            .menuIndicator(.hidden)
            .tint(.secondary)
            .fixedSize()
            .frame(minWidth: 28, minHeight: 28)
            .help("File Actions")
        }
        .padding(.leading, ChangedFileRow.pathInset)
        .padding(.trailing, 8)
    }

    @ViewBuilder
    private var summary: some View {
        switch availability {
        case let .lines(patch):
            counts(patch)
        case let .cutOff(partial, _, _):
            if let partial { counts(partial) }
        case .binary:
            message("Binary file changed", systemImage: "doc.badge.ellipsis")
        case .tooLarge:
            message("This file is too large to show.", systemImage: "doc")
        case .noTextChanges:
            message("No text changes to show.", systemImage: "doc.plaintext")
        }
    }

    private func message(_ text: LocalizedStringKey, systemImage: String) -> some View {
        Label(text, systemImage: systemImage)
            .foregroundStyle(.secondary)
    }

    /// "+14 −3", leaving out a side with no lines.
    @ViewBuilder
    private func counts(_ patch: DiffFilePatch) -> some View {
        let added = Text(verbatim: "+\(patch.additions)").foregroundStyle(.green)
        let removed = Text(verbatim: "\u{2212}\(patch.deletions)").foregroundStyle(.red)
        Group {
            if patch.additions > 0, patch.deletions > 0 {
                Text("\(added) \(removed)")
            } else if patch.deletions > 0 {
                removed
            } else if patch.additions > 0 {
                added
            }
        }
        .monospacedDigit()
    }
}

// MARK: - Comment on Line…

/// The Comment on Line… popover. The line and comment are session state, so a
/// failed or uncertain send keeps them and their Command.
struct CommentOnLineForm: View {
    @Bindable var session: DesktopSession
    let path: String
    let onClose: () -> Void

    static let maximumBytes = 8_192

    private enum Field: Hashable {
        case line
        case comment
    }

    @State private var failure: String?
    @FocusState private var focusedField: Field?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Comment on \(DesktopSession.detailsFileName(path))")
                .font(.headline)
                .lineLimit(1)
                .truncationMode(.middle)
            LabeledContent("Line") {
                HStack(spacing: 4) {
                    TextField("Line", value: $session.reviewLine, format: .number)
                        .labelsHidden()
                        .frame(width: 72)
                        .focused($focusedField, equals: .line)
                    Stepper("Line", value: $session.reviewLine, in: 1 ... 1_000_000)
                        .labelsHidden()
                }
            }
            TextField("Comment", text: $session.reviewComment, axis: .vertical)
                .lineLimit(3 ... 6)
                .focused($focusedField, equals: .comment)
            caption
            if let failure {
                Label {
                    Text(failure).fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: "xmark.octagon.fill").foregroundStyle(.red)
                }
                .font(.system(size: JetDesign.TextSize.metadata))
            }
            HStack(spacing: 8) {
                if session.workOperation == "review" {
                    ProgressView().controlSize(.small)
                }
                Spacer(minLength: 0)
                Button("Cancel", action: onClose)
                    .keyboardShortcut(.cancelAction)
                Button("Send Comment", action: send)
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(!canSend)
                    .accessibilityIdentifier("comment-send")
            }
        }
        .padding(16)
        .frame(width: 320)
        .reportsTextEditing(focusedField != nil)
        // WP8: .focusedSceneValue(\.hasOpenDialog, true) once WP5 defines the key (critic 4.8).
        .onAppear { focusedField = .comment }
    }

    private var trimmed: String {
        session.reviewComment.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var bytes: Int { trimmed.utf8.count }

    @ViewBuilder
    private var caption: some View {
        if Double(bytes) > Double(Self.maximumBytes) * 0.8 {
            let over = bytes > Self.maximumBytes
            Label {
                Text("\(bytes.formatted()) of \(Self.maximumBytes.formatted()) bytes")
            } icon: {
                Image(systemName: over ? "exclamationmark.triangle.fill" : "info.circle")
            }
            .font(.system(size: JetDesign.TextSize.metadata))
            .foregroundStyle(over ? AnyShapeStyle(Color.red) : AnyShapeStyle(.secondary))
        } else {
            Text("\(DetailsCopy.assistant(session.selectedAssistantName, position: .start)) gets this as a message.")
                .font(.system(size: JetDesign.TextSize.metadata))
                .foregroundStyle(.secondary)
        }
    }

    private var canSend: Bool {
        !trimmed.isEmpty
            && bytes <= Self.maximumBytes
            && session.reviewLine > 0
            && session.workOperation == nil
            && !session.detailsIsOffline
    }

    private func send() {
        guard canSend else { return }
        failure = nil
        Task {
            if await session.sendLineComment(path: path) {
                onClose()
            } else if let notice = session.detailsNotice, notice.kind != .confirmation {
                failure = notice.text
            }
        }
    }
}

#if DEBUG
#Preview { DesktopPreviewScenes.details[0].makeView() }
#endif
