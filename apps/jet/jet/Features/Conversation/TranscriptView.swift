import SwiftUI

/// The open task's messages and replies (design §6.5). It follows new output only
/// while it is at the bottom; otherwise Jump to Latest appears. Streaming never
/// moves focus.
struct TranscriptView: View {
    let session: DesktopSession
    var startsAtTop = false

    @AppStorage(JetDesign.transcriptScaleKey) private var storedScale = 1.0
    @AppStorage("jet.transcript.show-technical") private var showsTechnical = false
    @ScaledMetric(relativeTo: .body) private var bodySize: CGFloat = 14
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var position: ScrollPosition
    @State private var follow: TranscriptFollow
    @State private var lastSample: ScrollSample?
    @State private var phase: ScrollPhase = .idle
    @State private var expandedSteps: Set<String> = []

    init(session: DesktopSession, startsAtTop: Bool = false) {
        self.session = session
        self.startsAtTop = startsAtTop
        _position = State(initialValue: ScrollPosition(edge: startsAtTop ? .top : .bottom))
        _follow = State(initialValue: TranscriptFollow(followsLatest: !startsAtTop))
    }

    var body: some View {
        let rows = session.transcriptRows(showsTechnical: showsTechnical)
        let top = session.transcriptTop(rowsAreEmpty: rows.isEmpty)
        let notice = session.historyNotice
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 12) {
                TranscriptTopView(top: top, session: session)
                if let notice {
                    TranscriptHistoryRow(notice: notice, session: session)
                }
                ForEach(rows) { row in
                    TranscriptRowView(row: row, session: session, expandedSteps: $expandedSteps)
                        .padding(.top, Self.isYou(row) && row.id != rows.first?.id ? 12 : 0)
                        .id(row.id)
                }
                if let ref = session.transcriptConversationRef {
                    KeepChangesStatusLine(session: session, ref: ref)
                }
            }
            .frame(maxWidth: JetDesign.readingWidth, alignment: .leading)
            .padding(.horizontal, 24)
            .padding(.top, 20)
            .padding(.bottom, 16)
            .frame(maxWidth: .infinity)
            .textSelection(.enabled)
        }
        .scrollPosition($position)
        .defaultScrollAnchor(startsAtTop ? .top : .bottom, for: .initialOffset)
        // Earlier messages arriving above keep the visible ones in place.
        .defaultScrollAnchor(startsAtTop ? nil : .bottom, for: .sizeChanges)
        .onScrollPhaseChange { _, newPhase in
            phase = newPhase
        }
        .onScrollGeometryChange(for: ScrollSample.self) { geometry in
            ScrollSample(
                offsetY: geometry.contentOffset.y,
                contentHeight: geometry.contentSize.height,
                containerHeight: geometry.containerSize.height,
                topInset: geometry.contentInsets.top
            )
        } action: { _, sample in
            follow.observe(previous: lastSample, current: sample, userIsScrolling: isUserScrolling)
            lastSample = sample
        }
        .onChange(of: followToken) { _, _ in
            if follow.followsLatest { position.scrollTo(edge: .bottom) }
        }
        .overlay(alignment: .bottom) {
            if !follow.followsLatest, !rows.isEmpty {
                Button("Jump to Latest", systemImage: "arrow.down", action: jumpToLatest)
                    .buttonStyle(.glass)
                    .help("Scroll to the latest message")
                    .accessibilityIdentifier("jump-to-latest")
                    .padding(.bottom, 12)
            }
        }
        .environment(\.transcriptScale, CGFloat(min(max(storedScale, 0.85), 2.0)) * bodySize / 14)
        .accessibilityRotor(
            "Messages",
            entries: Self.rotorEntries(rows, assistant: session.selectedAssistantName),
            entryID: \.id,
            entryLabel: \.label
        )
        .accessibilityIdentifier("transcript")
    }


    private var isUserScrolling: Bool {
        switch phase {
        case .tracking, .interacting, .decelerating: true
        case .idle, .animating: false
        @unknown default: false
        }
    }

    /// Changes whenever new output arrives, including text streaming into the last entry.
    private var followToken: FollowToken {
        let entries = session.transcriptSourceEntries
        return FollowToken(
            count: entries.count,
            lastID: entries.last?.id,
            lastLength: entries.last?.text.utf8.count ?? 0
        )
    }

    private func jumpToLatest() {
        if reduceMotion {
            position.scrollTo(edge: .bottom)
        } else {
            withAnimation { position.scrollTo(edge: .bottom) }
        }
        follow.jumpToLatest()
    }

    private static func isYou(_ row: TranscriptRow) -> Bool {
        if case .you = row { return true }
        return false
    }

    private static func rotorEntries(_ rows: [TranscriptRow], assistant: String?) -> [RotorEntry] {
        let name = assistant ?? String(localized: "Assistant")
        return rows.compactMap { row in
            switch row {
            case let .you(you):
                RotorEntry(id: you.id, label: String(localized: "You: \(snippet(you.text))"))
            case let .assistant(reply):
                RotorEntry(id: reply.id, label: String(localized: "\(name): \(snippet(reply.text))"))
            default:
                nil
            }
        }
    }

    private static func snippet(_ text: String) -> String {
        let flat = text.split(whereSeparator: \.isNewline).joined(separator: " ")
        return flat.count > 80 ? String(flat.prefix(80)) + "…" : flat
    }
}

private struct FollowToken: Equatable {
    let count: Int
    let lastID: String?
    let lastLength: Int
}

private struct RotorEntry: Identifiable {
    let id: String
    let label: String
}

/// The task itself is loading or couldn't be read.
private struct TranscriptTopView: View {
    let top: TranscriptTop
    let session: DesktopSession

    var body: some View {
        switch top {
        case .none:
            EmptyView()
        case .loadingTask:
            ProgressView()
                .controlSize(.small)
                .accessibilityLabel("Loading task")
                .frame(maxWidth: .infinity)
                .padding(.top, 48)
        case .couldNotLoad:
            ContentUnavailableView {
                Label("Couldn't Load This Task", systemImage: "exclamationmark.triangle")
            } description: {
                Text("Jet couldn't read this task from \(session.selectedPlaneName).")
            } actions: {
                Button("Try Again") {
                    Task { await session.reloadSelectedTask() }
                }
            }
            .frame(maxWidth: .infinity, minHeight: 320)
        }
    }
}

/// Earlier history: loading, not available on this Mac, or no messages at all.
private struct TranscriptHistoryRow: View {
    let notice: TranscriptHistoryNotice
    let session: DesktopSession

    @Environment(\.transcriptScale) private var scale
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        switch notice {
        case let .loading(progress):
            HStack(spacing: 10) {
                Group {
                    if let progress {
                        ProgressView(value: progress)
                    } else {
                        ProgressView().progressViewStyle(.linear)
                    }
                }
                .frame(width: 120)
                Text(notice.title)
                    .font(TranscriptFont.metadata(scale))
                    .foregroundStyle(contrast == .increased ? .primary : .secondary)
            }
            .frame(maxWidth: .infinity)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("transcript-history-loading")
        case let .unavailable(summary, canRetry):
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Image(systemName: "clock.arrow.circlepath")
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(.secondary)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(notice.title)
                        if let summary {
                            Text(summary)
                                .font(TranscriptFont.metadata(scale))
                                .foregroundStyle(contrast == .increased ? .primary : .secondary)
                        }
                    }
                    HStack(spacing: 8) {
                        if canRetry {
                            Button("Try Again", action: session.retryEarlierMessages)
                                .buttonStyle(.bordered)
                                .accessibilityIdentifier("transcript-history-retry")
                        }
                        Button("Review Changes") { session.showChanges(.all) }
                            .buttonStyle(.bordered)
                            .accessibilityIdentifier("transcript-history-review-changes")
                    }
                    .controlSize(TranscriptFont.controlSize(scale))
                }
            }
            .font(TranscriptFont.content(scale))
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay {
                RoundedRectangle(cornerRadius: JetDesign.fieldRadius)
                    .stroke(.separator)
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("transcript-history-card")
        case .noMessages:
            ContentUnavailableView {
                Label("No Messages Yet", systemImage: "text.bubble")
            } description: {
                Text("Your messages and replies appear here.")
            }
            .frame(maxWidth: .infinity, minHeight: 320)
            .accessibilityIdentifier("transcript-no-messages")
        }
    }
}
