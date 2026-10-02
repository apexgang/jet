import SwiftUI

// MARK: - Model

/// One line of a hunk. Numbers are 1-based; each side has one only where the line
/// exists on that side.
struct DiffLine: Equatable, Sendable {
    enum Kind: Equatable, Sendable {
        case context
        case added
        case removed
        /// "\ No newline at end of file".
        case noNewlineMarker
    }

    let kind: Kind
    let text: String
    let oldNumber: Int?
    let newNumber: Int?
}

/// The parsed numbers of an `@@ -a,b +c,d @@ heading` line.
struct DiffHunkHeader: Equatable, Sendable {
    let oldStart: Int
    let oldCount: Int
    let newStart: Int
    let newCount: Int
    let heading: String?
}

struct DiffHunk: Equatable, Sendable {
    let oldStart: Int
    let oldCount: Int
    let newStart: Int
    let newCount: Int
    let heading: String?
    var lines: [DiffLine]

    /// "Lines 10–18 · func", "Line 10" or "Removed at line 10".
    var title: String {
        let range: String
        if newCount == 0 {
            range = String(localized: "Removed at line \(oldStart)")
        } else if newCount == 1 {
            range = String(localized: "Line \(newStart)")
        } else {
            range = String(localized: "Lines \(newStart)–\(newStart + newCount - 1)")
        }
        guard let heading, !heading.isEmpty else { return range }
        return "\(range) · \(heading)"
    }
}

/// One file's section of the change patch.
struct DiffFilePatch: Equatable, Sendable {
    let path: String
    var isBinary: Bool
    /// False for the last section of a patch that was cut off.
    var isComplete: Bool
    var hunks: [DiffHunk]
    var additions: Int
    var deletions: Int

    var hasTextChanges: Bool { additions + deletions > 0 }
}

/// A patch split at `diff --git`, in patch order.
struct DiffSplitResult: Equatable, Sendable {
    let order: [String]
    let files: [String: DiffFilePatch]
    let isComplete: Bool
    /// Totals over text sections; binary sections don't count.
    let additions: Int
    let deletions: Int

    static let empty = DiffSplitResult(order: [], files: [:], isComplete: true, additions: 0, deletions: 0)
}

/// What Details can show for one changed file.
enum DiffFileAvailability: Equatable, Sendable {
    case lines(DiffFilePatch)
    case binary
    /// The patch ends inside or before this file. `canLoadMore` when the rest is
    /// stored; otherwise `reason` says why it isn't.
    case cutOff(partial: DiffFilePatch?, canLoadMore: Bool, reason: String?)
    case tooLarge
    case noTextChanges

    /// Lines or a cut-off notice show under the file's header row; the other
    /// states are a sentence in that row.
    var showsLines: Bool {
        switch self {
        case .lines, .cutOff: true
        case .binary, .tooLarge, .noTextChanges: false
        }
    }
}

// MARK: - Parser

/// Splits a `git diff --binary --full-index --no-renames` patch into per-file
/// sections. Pure and synchronous; the Details model memoises it.
enum DiffSplit {
    static func split(_ patch: String, patchIsComplete: Bool) -> DiffSplitResult {
        var lines = patch.split(separator: "\n", omittingEmptySubsequences: false)
        if patch.hasSuffix("\n") { lines.removeLast() }

        var order: [String] = []
        var files: [String: DiffFilePatch] = [:]
        var section: SectionBuilder?

        func finish(_ builder: SectionBuilder?, isLast: Bool) {
            guard let builder else { return }
            var file = builder.build()
            file.isComplete = !(isLast && !patchIsComplete)
            if files[file.path] == nil { order.append(file.path) }
            files[file.path] = file
        }

        for line in lines {
            if line.hasPrefix("diff --git ") {
                finish(section, isLast: false)
                section = SectionBuilder(header: String(line.dropFirst("diff --git ".count)))
            } else {
                section?.consume(line)
            }
        }
        finish(section, isLast: true)

        let text = order.compactMap { files[$0] }.filter { !$0.isBinary }
        return DiffSplitResult(
            order: order,
            files: files,
            isComplete: patchIsComplete,
            additions: text.reduce(0) { $0 + $1.additions },
            deletions: text.reduce(0) { $0 + $1.deletions }
        )
    }

    /// Parses `@@ -a,b +c,d @@ heading`, with or without the `@@` markers. A
    /// missing count is 1.
    static func parseHunkHeader(_ line: some StringProtocol) -> DiffHunkHeader? {
        var rest = Substring(line)
        if rest.hasPrefix("@@") { rest = rest.dropFirst(2) }
        rest = rest.drop { $0 == " " }
        guard rest.first == "-" else { return nil }
        rest = rest.dropFirst()
        guard let (oldStart, oldCount, afterOld) = range(rest) else { return nil }
        rest = afterOld.drop { $0 == " " }
        guard rest.first == "+" else { return nil }
        rest = rest.dropFirst()
        guard let (newStart, newCount, afterNew) = range(rest) else { return nil }
        rest = afterNew.drop { $0 == " " }
        var heading: String?
        if rest.hasPrefix("@@") {
            let text = rest.dropFirst(2).trimmingCharacters(in: .whitespaces)
            heading = text.isEmpty ? nil : text
        }
        return DiffHunkHeader(
            oldStart: oldStart, oldCount: oldCount, newStart: newStart, newCount: newCount, heading: heading
        )
    }

    private static func range(_ text: Substring) -> (Int, Int, Substring)? {
        let digits = text.prefix { $0.isASCII && $0.isNumber }
        guard let start = Int(digits) else { return nil }
        var rest = text.dropFirst(digits.count)
        var count = 1
        if rest.first == "," {
            rest = rest.dropFirst()
            let countDigits = rest.prefix { $0.isASCII && $0.isNumber }
            guard let value = Int(countDigits) else { return nil }
            count = value
            rest = rest.dropFirst(countDigits.count)
        }
        return (start, count, rest)
    }

    /// Git's C-quoted path (`"caf\303\251.txt"`) as text. Unquoted input is
    /// returned unchanged.
    static func unquote(_ value: some StringProtocol) -> String {
        guard value.count >= 2, value.hasPrefix("\""), value.hasSuffix("\"") else { return String(value) }
        let inner = Array(value.utf8.dropFirst().dropLast())
        var bytes: [UInt8] = []
        var index = 0
        while index < inner.count {
            let byte = inner[index]
            guard byte == UInt8(ascii: "\\"), index + 1 < inner.count else {
                bytes.append(byte)
                index += 1
                continue
            }
            let next = inner[index + 1]
            if (UInt8(ascii: "0") ... UInt8(ascii: "7")).contains(next) {
                var value = 0
                var length = 0
                while length < 3, index + 1 + length < inner.count,
                      (UInt8(ascii: "0") ... UInt8(ascii: "7")).contains(inner[index + 1 + length])
                {
                    value = value * 8 + Int(inner[index + 1 + length] - UInt8(ascii: "0"))
                    length += 1
                }
                bytes.append(UInt8(truncatingIfNeeded: value))
                index += 1 + length
                continue
            }
            let escaped: UInt8 = switch next {
            case UInt8(ascii: "n"): 0x0A
            case UInt8(ascii: "t"): 0x09
            case UInt8(ascii: "r"): 0x0D
            case UInt8(ascii: "a"): 0x07
            case UInt8(ascii: "b"): 0x08
            case UInt8(ascii: "f"): 0x0C
            case UInt8(ascii: "v"): 0x0B
            default: next
            }
            bytes.append(escaped)
            index += 2
        }
        return String(decoding: bytes, as: UTF8.self)
    }

    /// What Details can show for `file` given the loaded part of the patch.
    static func availability(
        for file: JetChangedFile,
        in result: DiffSplitResult,
        artifact: JetChangeArtifact
    ) -> DiffFileAvailability {
        let canLoadMore = artifact.availability == .stored
        if let section = result.files[file.path] {
            if section.isBinary { return .binary }
            if !section.isComplete {
                return .cutOff(partial: section, canLoadMore: canLoadMore, reason: cutOffReason(artifact.availability))
            }
            return section.hasTextChanges ? .lines(section) : .noTextChanges
        }
        if !result.isComplete {
            return .cutOff(partial: nil, canLoadMore: canLoadMore, reason: cutOffReason(artifact.availability))
        }
        return file.contentAvailable ? .noTextChanges : .tooLarge
    }

    /// Why the rest of a cut-off patch can't be loaded, or nil when it can.
    static func cutOffReason(_ availability: JetArtifactAvailability) -> String? {
        switch availability {
        case .stored: nil
        case .diskPressure: String(localized: "The rest wasn't saved because the disk was almost full.")
        case .runBudgetExceeded: String(localized: "The rest wasn't saved because this task reached its storage limit.")
        case .artifactSizeExceeded: String(localized: "The full changes are too large to save.")
        }
    }

    /// The path in `a/P b/P`, `"a/P" "b/P"` or a `---`/`+++` line, without its
    /// side prefix. Nil for /dev/null.
    fileprivate static func sidePath(_ token: Substring, prefix: String) -> String? {
        var token = token
        // Git ends ---/+++ names that contain a space with a tab.
        if token.hasSuffix("\t") { token = token.dropLast() }
        let path = token.hasPrefix("\"") ? unquote(token) : String(token)
        if path == "/dev/null" { return nil }
        return path.hasPrefix(prefix) ? String(path.dropFirst(prefix.count)) : path
    }

    /// The path of a `diff --git` header whose sides are the same file.
    fileprivate static func headerPath(_ header: String) -> String? {
        if header.hasPrefix("\"") {
            // "a/P" "b/P": the first quoted token, up to an unescaped quote.
            var escaped = false
            var end: String.Index?
            var index = header.index(after: header.startIndex)
            while index < header.endIndex {
                let character = header[index]
                if escaped {
                    escaped = false
                } else if character == "\\" {
                    escaped = true
                } else if character == "\"" {
                    end = index
                    break
                }
                index = header.index(after: index)
            }
            guard let end else { return nil }
            return sidePath(header[header.startIndex ... end], prefix: "a/")
        }
        guard header.hasPrefix("a/") else { return nil }
        // a/P b/P has length 2p + 5; the sides match because renames are off.
        let characters = Array(header)
        if characters.count >= 5, (characters.count - 5) % 2 == 0 {
            let length = (characters.count - 5) / 2
            let first = String(characters[2 ..< 2 + length])
            let separator = String(characters[(2 + length) ..< (2 + length + 3)])
            let second = String(characters[(2 + length + 3)...])
            if separator == " b/", first == second { return first }
        }
        guard let separator = header.range(of: " b/") else { return String(header.dropFirst(2)) }
        return String(header[header.index(header.startIndex, offsetBy: 2) ..< separator.lowerBound])
    }
}

/// Accumulates one `diff --git` section.
private struct SectionBuilder {
    let header: String
    var oldPath: String?
    var newPath: String?
    var sawOldLine = false
    var sawNewLine = false
    var isBinary = false
    var hunks: [DiffHunk] = []
    var oldLine = 0
    var newLine = 0
    var additions = 0
    var deletions = 0

    init(header: String) {
        self.header = header
    }

    mutating func consume(_ line: Substring) {
        if isBinary { return }
        if hunks.isEmpty {
            consumeHeader(line)
            return
        }
        if line.hasPrefix("@@") {
            startHunk(line)
            return
        }
        let text = String(line.dropFirst())
        switch line.first {
        case "+":
            hunks[hunks.count - 1].lines.append(DiffLine(kind: .added, text: text, oldNumber: nil, newNumber: newLine))
            newLine += 1
            additions += 1
        case "-":
            hunks[hunks.count - 1].lines.append(DiffLine(kind: .removed, text: text, oldNumber: oldLine, newNumber: nil))
            oldLine += 1
            deletions += 1
        case " ", nil:
            hunks[hunks.count - 1].lines.append(DiffLine(kind: .context, text: text, oldNumber: oldLine, newNumber: newLine))
            oldLine += 1
            newLine += 1
        case "\\":
            hunks[hunks.count - 1].lines.append(
                DiffLine(kind: .noNewlineMarker, text: text.trimmingCharacters(in: .whitespaces), oldNumber: nil, newNumber: nil)
            )
        default:
            break
        }
    }

    private mutating func consumeHeader(_ line: Substring) {
        if line.hasPrefix("@@") {
            startHunk(line)
        } else if line.hasPrefix("--- ") {
            sawOldLine = true
            oldPath = DiffSplit.sidePath(line.dropFirst(4), prefix: "a/")
        } else if line.hasPrefix("+++ ") {
            sawNewLine = true
            newPath = DiffSplit.sidePath(line.dropFirst(4), prefix: "b/")
        } else if line == "GIT binary patch" || (line.hasPrefix("Binary files ") && line.hasSuffix(" differ")) {
            isBinary = true
        }
    }

    private mutating func startHunk(_ line: Substring) {
        guard let header = DiffSplit.parseHunkHeader(line) else { return }
        hunks.append(DiffHunk(
            oldStart: header.oldStart,
            oldCount: header.oldCount,
            newStart: header.newStart,
            newCount: header.newCount,
            heading: header.heading,
            lines: []
        ))
        oldLine = header.oldStart
        newLine = header.newStart
    }

    func build() -> DiffFilePatch {
        let path = (sawNewLine ? newPath : nil)
            ?? (sawOldLine ? oldPath : nil)
            ?? DiffSplit.headerPath(header)
            ?? header
        return DiffFilePatch(
            path: path,
            isBinary: isBinary,
            isComplete: true,
            hunks: isBinary ? [] : hunks,
            additions: isBinary ? 0 : additions,
            deletions: isBinary ? 0 : deletions
        )
    }
}

// MARK: - Views

/// One file's expanded changes in Details.
struct FileDiffView: View {
    let file: JetChangedFile
    let availability: DiffFileAvailability
    @Bindable var session: DesktopSession
    @Bindable var model: DetailsPanelModel
    /// Opens Comment on Line… at a new-side line.
    let onComment: (UInt32) -> Void

    static let lineLimit = 1_000

    var body: some View {
        switch availability {
        case let .lines(patch):
            DiffLinesView(patch: patch, path: file.path, model: model, onComment: onComment)
        case let .cutOff(partial, canLoadMore, reason):
            VStack(alignment: .leading, spacing: 0) {
                if let partial, !partial.hunks.isEmpty {
                    DiffLinesView(patch: partial, path: file.path, model: model, onComment: onComment)
                }
                VStack(alignment: .leading, spacing: 8) {
                    Text("This file's changes are cut off.")
                        .foregroundStyle(.secondary)
                    if canLoadMore {
                        HStack(spacing: 8) {
                            Button("Load More") { Task { await session.loadMorePatch() } }
                                .disabled(session.workOperation != nil || session.detailsIsOffline)
                                .accessibilityIdentifier("diff-load-more")
                            if session.workOperation == "patch" {
                                ProgressView().controlSize(.small)
                            }
                        }
                    } else if let reason {
                        Text(reason)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .font(.system(size: JetDesign.TextSize.control))
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
        case .binary, .tooLarge, .noTextChanges:
            // The header row says why no lines show.
            EmptyView()
        }
    }
}

/// A row of the rendered diff: a hunk title or a line.
private struct DiffRow: Identifiable {
    enum Content {
        case hunkTitle(String)
        case line(DiffLine)
    }

    let id: Int
    let content: Content
}

/// The hunks of one file, scrolling horizontally only.
private struct DiffLinesView: View {
    let patch: DiffFilePatch
    let path: String
    @Bindable var model: DetailsPanelModel
    let onComment: (UInt32) -> Void

    @Environment(\.colorSchemeContrast) private var contrast
    @State private var viewportWidth: CGFloat = 0

    var body: some View {
        let (rows, hiddenLines) = visibleRows
        VStack(alignment: .leading, spacing: 0) {
            ScrollView(.horizontal) {
                DiffRowsLayout(minWidth: viewportWidth) {
                    ForEach(rows) { row in
                        rowView(row)
                    }
                }
            }
            .scrollIndicators(.automatic)
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { viewportWidth = $0 }

            if hiddenLines > 0 {
                Button("Show \(hiddenLines) More Lines") {
                    model.lineLimitOverrides.insert(path)
                }
                .font(.system(size: JetDesign.TextSize.control))
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
        }
    }

    /// Hunk titles and lines, stopping after the line limit unless it was lifted.
    private var visibleRows: ([DiffRow], Int) {
        let limit = model.lineLimitOverrides.contains(path) ? Int.max : FileDiffView.lineLimit
        var rows: [DiffRow] = []
        var shownLines = 0
        var hiddenLines = 0
        for hunk in patch.hunks {
            if shownLines >= limit {
                hiddenLines += hunk.lines.count
                continue
            }
            rows.append(DiffRow(id: rows.count, content: .hunkTitle(hunk.title)))
            for line in hunk.lines {
                if shownLines < limit {
                    rows.append(DiffRow(id: rows.count, content: .line(line)))
                    shownLines += 1
                } else {
                    hiddenLines += 1
                }
            }
        }
        return (rows, hiddenLines)
    }

    @ViewBuilder
    private func rowView(_ row: DiffRow) -> some View {
        switch row.content {
        case let .hunkTitle(title):
            Text(verbatim: title)
                .font(.system(size: JetDesign.TextSize.metadata))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .fixedSize()
                .padding(.horizontal, 12)
                .padding(.vertical, 3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.secondary.opacity(0.08))
                .accessibilityAddTraits(.isHeader)
        case let .line(line):
            DiffLineRow(line: line, contrast: contrast)
                .contextMenu {
                    if let number = line.newNumber, number > 0 {
                        Button("Comment on Line \(UInt32(number))…") { onComment(UInt32(number)) }
                    }
                }
        }
    }
}

/// One diff line: the new-side number, the +/− marker and the text.
private struct DiffLineRow: View {
    let line: DiffLine
    let contrast: ColorSchemeContrast

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 0) {
            Text(verbatim: line.newNumber.map(String.init) ?? "")
                .font(.system(size: JetDesign.TextSize.metadata).monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 36, alignment: .trailing)
            Text(verbatim: marker)
                .font(.system(size: JetDesign.TextSize.control, design: .monospaced))
                .foregroundStyle(markerStyle)
                .frame(width: 18)
            Text(verbatim: line.text.isEmpty ? " " : line.text)
                .font(.system(size: line.kind == .noNewlineMarker ? JetDesign.TextSize.metadata : JetDesign.TextSize.control,
                              design: .monospaced))
                .foregroundStyle(line.kind == .noNewlineMarker ? .secondary : .primary)
                .lineLimit(1)
                .fixedSize()
                .textSelection(.enabled)
                .padding(.trailing, 12)
        }
        .padding(.vertical, 1)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(background)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(accessibilityText))
    }

    private var marker: String {
        switch line.kind {
        case .added: "+"
        case .removed: "\u{2212}"
        case .context, .noNewlineMarker: ""
        }
    }

    private var markerStyle: AnyShapeStyle {
        switch line.kind {
        case .added: AnyShapeStyle(Color.green)
        case .removed: AnyShapeStyle(Color.red)
        case .context, .noNewlineMarker: AnyShapeStyle(.secondary)
        }
    }

    private var background: Color {
        switch line.kind {
        case .added: JetDesign.diffAddedBackground(contrast: contrast)
        case .removed: JetDesign.diffRemovedBackground(contrast: contrast)
        case .context, .noNewlineMarker: .clear
        }
    }

    private var accessibilityText: String {
        switch line.kind {
        case .added:
            String(localized: "Added line \(line.newNumber ?? 0): \(line.text)")
        case .removed:
            String(localized: "Removed line \(line.oldNumber ?? 0): \(line.text)")
        case .context:
            String(localized: "Line \(line.newNumber ?? 0): \(line.text)")
        case .noNewlineMarker:
            line.text
        }
    }
}

/// Stacks rows at the width of the widest one, at least the viewport width, so
/// line tints span the whole row while the diff scrolls horizontally.
struct DiffRowsLayout: Layout {
    var minWidth: CGFloat

    struct Cache {
        var minWidth: CGFloat = -1
        var width: CGFloat = 0
        var heights: [CGFloat] = []
    }

    func makeCache(subviews: Subviews) -> Cache {
        Cache()
    }

    func updateCache(_ cache: inout Cache, subviews: Subviews) {
        cache = Cache()
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout Cache) -> CGSize {
        let width = measure(subviews, cache: &cache)
        return CGSize(width: width, height: cache.heights.reduce(0, +))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout Cache) {
        _ = measure(subviews, cache: &cache)
        var y = bounds.minY
        for (index, subview) in subviews.enumerated() {
            let height = cache.heights[index]
            subview.place(
                at: CGPoint(x: bounds.minX, y: y),
                proposal: ProposedViewSize(width: bounds.width, height: height)
            )
            y += height
        }
    }

    private func measure(_ subviews: Subviews, cache: inout Cache) -> CGFloat {
        if cache.heights.count == subviews.count, cache.minWidth == minWidth, !subviews.isEmpty {
            return cache.width
        }
        let widest = subviews.map { $0.sizeThatFits(.unspecified).width }.max() ?? 0
        let width = max(minWidth, widest)
        cache.minWidth = minWidth
        cache.width = width
        cache.heights = subviews.map { $0.sizeThatFits(ProposedViewSize(width: width, height: nil)).height }
        return width
    }
}
