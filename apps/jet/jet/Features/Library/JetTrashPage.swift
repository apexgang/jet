import SwiftUI

/// Jet Trash (design §6.11): tasks moved out of the list, each restorable with one
/// click until it is removed for good.
struct JetTrashPage: View {
    let session: DesktopSession
    @State private var model: JetTrashModel
    @State private var planeRegistryID: UUID
    @State private var sortOrder = [KeyPathComparator(\JetTrashRow.deletedAt, order: .reverse)]
    @State private var selection: Set<UUID> = []

    init(session: DesktopSession, model: JetTrashModel? = nil) {
        self.session = session
        _model = State(initialValue: model ?? JetTrashModel(
            makeAccess: session.libraryAccessProvider,
            memory: session.memory,
            now: { [weak session] in session?.libraryNow ?? .now }
        ))
        _planeRegistryID = State(initialValue: model?.planeRegistryID ?? session.localPlaneRegistryID)
    }

    private var computer: String { session.planeDisplayName(planeRegistryID) }
    private var isLocal: Bool { session.isLocalPlane(planeRegistryID) }

    private var rows: [JetTrashRow] {
        let projects = Dictionary(
            (session.planeSetupSnapshot(for: planeRegistryID)?.projects.projects ?? []).map { ($0.id, $0.name) },
            uniquingKeysWith: { first, _ in first }
        )
        return model.rows(listed: session.conversations, projects: projects)
            .sorted(using: sortOrder)
    }

    var body: some View {
        content
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .safeAreaBar(edge: .bottom) { footer }
            .task(id: ReloadKey(planeRegistryID: planeRegistryID, conversationCount: session.conversations.count)) {
                guard session.canLoadLibrary else { return }
                await model.load(planeRegistryID: planeRegistryID, computer: computer, isLocal: isLocal)
            }
#if !os(macOS)
            .navigationTitle("Jet Trash")
#endif
    }

    private struct ReloadKey: Hashable {
        let planeRegistryID: UUID
        let conversationCount: Int
    }

    // MARK: Content

    @ViewBuilder
    private var content: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            switch model.phase {
            case .loading:
                placeholder
            case .unavailable:
                ContentUnavailableView {
                    Label("Can't Show Jet Trash", systemImage: "trash.slash")
                } description: {
                    Text("Connect to \(computer) to see removed tasks.")
                } actions: {
                    Button("Try Again") { Task { await reload() } }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .loaded:
                if model.entries.isEmpty {
                    ContentUnavailableView(
                        "Jet Trash Is Empty",
                        systemImage: "trash",
                        description: Text("Tasks you move to Jet Trash stay here until they're removed for good.")
                    )
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    table
                }
            }
        }
    }

    /// The computer picker and the page's feedback lines.
    @ViewBuilder
    private var header: some View {
        let lines = headerLines
        if session.showsComputerNames || !lines.isEmpty {
            VStack(alignment: .leading, spacing: JetDesign.smallGap) {
                if session.showsComputerNames {
                    Picker("Computer", selection: $planeRegistryID) {
                        ForEach(session.planes) { plane in
                            Text(plane.name).tag(plane.id)
                        }
                    }
                    .fixedSize()
                }
                ForEach(lines, id: \.text) { line in
                    switch line {
                    case let .notice(notice):
                        InlineNotice(notice: notice, perform: session.perform)
                    case let .issue(issue):
                        LibraryNoticeRow(issue: issue, perform: perform)
                    }
                }
            }
            .padding(.horizontal, JetDesign.gap)
            .padding(.vertical, JetDesign.smallGap)
        }
    }

    private enum HeaderLine {
        case notice(ComposerNotice)
        case issue(LibraryIssue)

        var text: String {
            switch self {
            case let .notice(notice): notice.text
            case let .issue(issue): issue.text
            }
        }
    }

    private var headerLines: [HeaderLine] {
        var lines: [HeaderLine] = []
        if let notice = session.libraryPageNotice { lines.append(.notice(notice)) }
        if let notice = model.notice { lines.append(.issue(notice)) }
        if model.isShowingCache {
            lines.append(.issue(LibraryIssue(
                kind: .info,
                text: String(localized: "Showing Jet Trash from when \(computer) was last connected."),
                action: .tryAgain
            )))
        }
        if model.phase == .loaded, model.isReadOnly, model.issue == nil {
            lines.append(.issue(.paused()))
        }
        if model.phase == .loaded, let issue = model.issue { lines.append(.issue(issue)) }
        return lines
    }

    private var table: some View {
        let rows = rows
        return Table(rows, selection: $selection, sortOrder: $sortOrder) {
            TableColumn("Task", value: \.title) { row in
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.title)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    if let note = row.note {
                        Text(note)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                .padding(.vertical, 2)
            }
            .width(min: 180, ideal: 280)
            TableColumn("Project", value: \.projectSortKey) { row in
                Text(row.projectName ?? "—")
                    .foregroundStyle(row.projectName == nil ? .secondary : .primary)
                    .lineLimit(1)
            }
            .width(min: 80, ideal: 120)
            TableColumn("Deleted", value: \.deletedAt) { row in
                Text(LibraryCopy.shortDate(row.deletedAt, now: model.now()))
                    .foregroundStyle(.secondary)
            }
            .width(min: 64, ideal: 80)
            TableColumn("Removed On", value: \.removedOn) { row in
                Text(row.removedOn <= model.now()
                     ? String(localized: "Soon")
                     : LibraryCopy.shortDate(row.removedOn, now: model.now()))
                    .foregroundStyle(.secondary)
            }
            .width(min: 80, ideal: 96)
            TableColumn("") { row in
                restoreCell(row)
            }
            .width(min: 90, ideal: 150)
        }
        .contextMenu(forSelectionType: UUID.self) { ids in
            if let row = rows.first(where: { ids.contains($0.id) }), row.canRestore {
                Button("Restore") { restore(row) }
                    .disabled(!model.canChange || !isReachable)
            }
        }
        .accessibilityIdentifier("jet-trash-table")
    }

    @ViewBuilder
    private func restoreCell(_ row: JetTrashRow) -> some View {
        HStack {
            Spacer(minLength: 0)
            if model.restoring.contains(row.id) {
                ProgressView()
                    .controlSize(.small)
                    .accessibilityLabel(Text("Restoring \(row.title)"))
            } else if row.canRestore {
                Button("Restore") { restore(row) }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(!model.canChange || !isReachable)
                    .help(model.isReadOnly ? String(localized: "Jet paused changes to protect your data.") : "")
                    .accessibilityLabel(Text("Restore \(row.title)"))
                    .accessibilityIdentifier("trash-restore-\(row.id.uuidString)")
            } else {
                Text("Can't be restored here")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
    }

    /// Four placeholder rows while Jet Trash loads.
    private var placeholder: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(0 ..< 4, id: \.self) { index in
                HStack(spacing: JetDesign.gap) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(index.isMultiple(of: 2) ? "Placeholder task title" : "A longer placeholder task title")
                        Text("Placeholder note").font(.caption)
                    }
                    Spacer()
                    Text("web-app")
                    Text("Sep 20")
                    Text("Oct 20")
                    Text("Restore")
                }
                .padding(.horizontal, JetDesign.gap)
                .padding(.vertical, JetDesign.smallGap)
                Divider()
            }
            Spacer(minLength: 0)
        }
        .redacted(reason: .placeholder)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text("Loading…"))
    }

    // MARK: Footer

    @ViewBuilder
    private var footer: some View {
        if let days = model.graceDays, model.phase == .loaded {
            HStack(spacing: JetDesign.smallGap) {
                Text("Tasks are removed for good after ^[\(Int(days)) day](inflect: true).")
                    .foregroundStyle(.secondary)
                Spacer(minLength: JetDesign.smallGap)
                Button("Change in Settings…") { session.perform(.openSettings(.work)) }
                    .buttonStyle(.borderless)
                    .foregroundStyle(JetDesign.accentText)
            }
            .font(.callout)
            .padding(.horizontal, JetDesign.gap)
            .padding(.vertical, JetDesign.smallGap)
        }
    }

    // MARK: Actions

    /// Restore needs the computer; a saved list can't change anything.
    private var isReachable: Bool {
        !session.isComputerOffline(planeRegistryID)
    }

    private func reload() async {
        await model.reload(computer: computer, isLocal: isLocal)
    }

    private func restore(_ row: JetTrashRow) {
        let ref = ConversationRef(conversationID: row.id, planeRegistryID: planeRegistryID)
        Task {
            if await model.restore(row, computer: computer, isLocal: isLocal) {
                await session.libraryDidRestore(ref)
            }
        }
    }

    private func perform(_ action: LibraryIssue.Action) {
        switch action {
        case .tryAgain, .checkAgain:
            Task { await reload() }
        case .sendSameRequestAgain:
            let ref = model.uncertainRestore.map {
                ConversationRef(conversationID: $0, planeRegistryID: planeRegistryID)
            }
            let rows = rows
            Task {
                if await model.sendSameRequestAgain(rows: rows, computer: computer, isLocal: isLocal),
                   let ref
                {
                    await session.libraryDidRestore(ref)
                }
            }
        case let .review(pane):
            session.perform(.openSettings(pane))
        case let .openTask(conversationID):
            session.openTask(conversationID: conversationID, planeRegistryID: planeRegistryID)
        case .showJetTrash, .showWaitingMessages:
            break
        }
    }
}

#if DEBUG
#Preview("Jet Trash") {
    DesktopPreviewScenes.view("library-trash")
        .frame(width: 1280, height: 800)
}
#endif
