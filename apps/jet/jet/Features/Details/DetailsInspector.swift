import SwiftUI

/// The name WP5's host used before the redesign.
typealias WorkPanelView = DetailsInspector

/// The Details inspector (design §6.8): Changes | Terminal | Activity, with no
/// header and no Close button. The host owns the column width.
struct DetailsInspector: View {
    @Bindable var session: DesktopSession
    @State private var model: DetailsPanelModel

    init(session: DesktopSession) {
        self.session = session
        _model = State(initialValue: DetailsPanelModel())
    }

#if DEBUG
    /// Previews start from a seeded panel state, such as expanded files.
    init(session: DesktopSession, previewModel: DetailsPanelModel) {
        self.session = session
        _model = State(initialValue: previewModel)
    }
#endif

    var body: some View {
        content
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .safeAreaBar(edge: .top) {
                if session.selectedConversationID != nil {
                    tabPicker
                }
            }
            .safeAreaBar(edge: .bottom) {
                bottomBar
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("work-panel")
            .onChange(of: session.selectedConversationID) {
                model.reset()
            }
            .onChange(of: session.detailsTab) {
                if session.workNoticeError == nil { session.workNotice = nil }
            }
            .onChange(of: session.isEditingFile) { _, isEditing in
                if !isEditing { model.savedDuringEdit = false }
            }
            .onChange(of: session.editableFile) { old, new in
                // A save from the unsaved-edit alert also counts as saved while editing.
                if let old, let new, old.path == new.path, old.revision != new.revision {
                    model.savedDuringEdit = true
                }
            }
    }

    @ViewBuilder
    private var content: some View {
        if session.selectedConversationID == nil {
            ContentUnavailableView(
                "No Task Selected",
                systemImage: "sidebar.right",
                description: Text("Details appear once a task starts.")
            )
        } else {
            switch session.selectedWorkPanel {
            case .changes, .files:
                if session.isEditingFile {
                    DetailsFileEditor(session: session, model: model)
                } else {
                    DetailsChangesView(session: session, model: model)
                }
            case .terminal:
                DetailsTerminalView(session: session, model: model)
            case .run:
                DetailsActivityView(session: session, model: model)
            }
        }
    }

    private var tabPicker: some View {
        Picker("Details", selection: tabBinding) {
            ForEach(WorkPanelTab.inspectorTabs, id: \.self) { tab in
                Text(tab.title).tag(tab)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .accessibilityIdentifier("details-tab-picker")
    }

    private var tabBinding: Binding<WorkPanelTab> {
        Binding(
            get: { session.detailsTab },
            set: { session.selectDetailsTab($0, reloadChanges: model.savedDuringEdit) }
        )
    }

    @ViewBuilder
    private var bottomBar: some View {
        VStack(spacing: 0) {
            if let notice = session.detailsNotice, session.selectedConversationID != nil {
                DetailsNoticeView(kind: notice.kind, text: notice.text) {
                    if let error = notice.error {
                        ForEach(error.recoveryActions) { action in
                            Button(action.detailsTitle) {
                                Task { await session.applyWorkRecovery(action) }
                            }
                        }
                        JetSettingsRecoveryButton(session: session, error: error)
                    }
                }
                .padding(12)
            }
            if session.selectedConversationID != nil {
                if session.isEditingFile {
                    DetailsEditorFooter(session: session, model: model)
                } else if session.detailsTab == .changes, session.selectedRun != nil {
                    DetailsKeepFooter(session: session)
                }
            }
        }
    }
}

// MARK: - Panel model

/// View state of the Details inspector that isn't session state. Held with
/// `@State` in DetailsInspector and reset when the task changes.
@MainActor
@Observable
final class DetailsPanelModel {
    /// Files whose diffs are shown.
    var expandedPaths: Set<String> = []
    /// Last Reply stays on the newest reply as replies finish.
    var followsLastReply = false
    /// The scope the person chose in this panel, if any.
    var chosenScope: ChangesScopeChoice?
    /// The file whose Comment on Line… popover is open.
    var commentPath: String?
    /// The file the comment draft in the session belongs to.
    var commentDraftPath: String?
    var rangeEditorShown = false
    /// The open file was saved at least once in this edit, so Changes reloads on Done.
    var savedDuringEdit = false
    /// The terminal whose Close Terminal… confirmation is showing.
    var terminalPendingClose: UUID?
    var showsTechnicalDetails = false
    /// Files whose diffs show every line, past the 1,000-line limit.
    var lineLimitOverrides: Set<String> = []
    /// Set by a row click so the selection change doesn't re-expand the row.
    @ObservationIgnored var selectionFromRow: String?
    /// The last expansion identity (Run, scope and first file).
    @ObservationIgnored var expansionIdentity: String?

    @ObservationIgnored private var cacheKey: DiffCacheKey?
    @ObservationIgnored private var cachedSplit: DiffSplitResult?

    init() {}

    /// The split patch, memoised by artifact, loaded length, scope and
    /// completeness, so `body` can call it.
    func split(patch: String, isComplete: Bool, key diff: JetChangeDiff) -> DiffSplitResult {
        let key = DiffCacheKey(
            sha256: diff.artifact.sha256,
            loadedBytes: patch.utf8.count,
            scope: diff.scope.label,
            isComplete: isComplete
        )
        if key == cacheKey, let cachedSplit { return cachedSplit }
        let result = DiffSplit.split(patch, patchIsComplete: isComplete)
        cacheKey = key
        cachedSplit = result
        return result
    }

    func reset() {
        expandedPaths = []
        followsLastReply = false
        chosenScope = nil
        commentPath = nil
        commentDraftPath = nil
        rangeEditorShown = false
        savedDuringEdit = false
        terminalPendingClose = nil
        showsTechnicalDetails = false
        lineLimitOverrides = []
        selectionFromRow = nil
        expansionIdentity = nil
        cacheKey = nil
        cachedSplit = nil
    }
}

private struct DiffCacheKey: Hashable {
    let sha256: String
    let loadedBytes: Int
    let scope: String
    let isComplete: Bool
}

// MARK: - Shared pieces

/// One line of feedback: a symbol and colour for its kind, the text, and small
/// bordered actions.
struct DetailsNoticeView<Actions: View>: View {
    let kind: DetailsNotice.Kind
    let text: String
    let actions: Actions

    init(kind: DetailsNotice.Kind, text: String, @ViewBuilder actions: () -> Actions) {
        self.kind = kind
        self.text = text
        self.actions = actions()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Image(systemName: symbol)
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(tint)
                    .accessibilityHidden(true)
                Text(text)
                    .foregroundStyle(kind == .confirmation ? .secondary : .primary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Text(accessibilityText))
            HStack(spacing: 8) {
                actions
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
        .font(.system(size: JetDesign.TextSize.control))
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var symbol: String {
        switch kind {
        case .confirmation: "checkmark.circle"
        case .warning: "exclamationmark.triangle.fill"
        case .error: "xmark.octagon.fill"
        }
    }

    private var tint: Color {
        switch kind {
        case .confirmation: .secondary
        case .warning: .orange
        case .error: .red
        }
    }

    private var accessibilityText: String {
        switch kind {
        case .confirmation: text
        case .warning: String(localized: "Warning: \(text)")
        case .error: String(localized: "Error: \(text)")
        }
    }
}

extension DetailsNoticeView where Actions == EmptyView {
    init(kind: DetailsNotice.Kind, text: String) {
        self.init(kind: kind, text: text) { EmptyView() }
    }
}

/// The Changes footer: the Keep status line and Keep Changes….
struct DetailsKeepFooter: View {
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let ref = session.detailsConversationRef {
                KeepChangesStatusLine(session: session, ref: ref)
            }
            HStack {
                Spacer(minLength: 0)
                // Bordered: the toolbar holds the prominent Keep.
                Button("Keep Changes…") { session.presentKeepChanges(mode: .plan) }
                    .buttonStyle(.bordered)
                    .disabled(!session.canKeepChanges)
                    .help(session.keepChangesUnavailableReason
                        ?? String(localized: "Review the changes and choose how to keep them."))
                    .accessibilityIdentifier("keep-changes-details")
            }
        }
        .padding(12)
    }
}

// MARK: - Kept for other packages

// Kept for DeliveryViews.swift (WP9); wave 3 removes if unused.
struct WorkSectionHeader<Actions: View>: View {
    let title: String
    let detail: String
    let actions: Actions

    init(
        title: String,
        detail: String,
        @ViewBuilder actions: () -> Actions
    ) {
        self.title = title
        self.detail = detail
        self.actions = actions()
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.subheadline.weight(.semibold))
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 6)
            actions
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }
}

extension WorkSectionHeader where Actions == EmptyView {
    init(title: String, detail: String) {
        self.init(title: title, detail: detail) { EmptyView() }
    }
}

// Kept for DeliveryViews.swift (WP9); wave 3 removes if unused.
struct WorkPanelEmptyState: View {
    let title: String
    let message: String
    let symbol: String

    var body: some View {
        ContentUnavailableView {
            Label(title, systemImage: symbol)
        } description: {
            Text(message)
        }
        .padding()
    }
}

#if DEBUG
#Preview { DesktopPreviewScenes.details[0].makeView() }
#endif
