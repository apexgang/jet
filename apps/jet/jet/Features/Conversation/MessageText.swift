import SwiftUI

/// A small inert document renderer for messages and replies. Model output can't
/// create links, load images or run markup; the transcript keeps its text
/// selectable, including code.
struct MessageText: View {
    let text: String
    @Environment(\.transcriptScale) private var scale

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(MessageBlock.parse(text).enumerated()), id: \.offset) { _, block in
                MessageBlockView(block: block)
            }
        }
        .font(TranscriptFont.content(scale))
        .lineSpacing(3 * scale)
    }
}

private struct MessageBlockView: View {
    let block: MessageBlock
    @Environment(\.transcriptScale) private var scale

    var body: some View {
        switch block.kind {
        case .paragraph:
            inline
        case let .heading(level):
            inline
                .font(TranscriptFont.heading(level: level, scale: scale))
                .accessibilityAddTraits(.isHeader)
                .padding(.top, 4)
        case let .bullet(indent):
            item(indent: indent) {
                Text(verbatim: "•").accessibilityHidden(true)
            }
        case let .numbered(number, indent):
            item(indent: indent) {
                Text(verbatim: "\(number).").monospacedDigit()
            }
        case let .task(checked, indent):
            item(indent: indent) {
                Image(systemName: checked ? "checkmark.square" : "square")
                    .accessibilityLabel(checked ? Text("Done") : Text("Not done"))
            }
        case let .code(language):
            CodeBlockView(language: language, code: block.text)
        case .rule:
            Divider().padding(.vertical, 4)
        }
    }

    private var inline: some View {
        Text(MessageInline.inert(block.text))
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func item<Marker: View>(indent: Int, @ViewBuilder marker: () -> Marker) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            marker()
                .foregroundStyle(.secondary)
                .frame(minWidth: 14 * scale, alignment: .trailing)
            inline
        }
        .padding(.leading, CGFloat(indent) * 18 * scale)
    }
}

/// A fenced code block: its language, a Copy button and unwrapped text that
/// scrolls sideways.
struct CodeBlockView: View {
    let language: String?
    let code: String

    @Environment(\.transcriptScale) private var scale
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language ?? String(localized: "Code"))
                    .font(TranscriptFont.metadata(scale))
                    .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Button(action: copy) {
                    Label(copyTitle, systemImage: copied ? "checkmark" : "document.on.document")
                    .labelStyle(.iconOnly)
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .help(copyTitle)
            }
            .padding(.leading, 12)
            .padding(.trailing, 4)
            ScrollView(.horizontal) {
                Text(verbatim: code)
                    .font(TranscriptFont.code(scale))
                    .fixedSize(horizontal: true, vertical: false)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 10)
            }
        }
        .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: JetDesign.controlRadius))
        .contextMenu {
            Button("Copy Code", action: copy)
        }
        .task(id: copied) {
            guard copied else { return }
            try? await Task.sleep(for: .seconds(1.5))
            copied = false
        }
    }

    private var copyTitle: String {
        copied ? String(localized: "Copied") : String(localized: "Copy Code")
    }

    private func copy() {
        TranscriptPasteboard.copy(code)
        copied = true
    }
}

/// Inline Markdown made inert.
enum MessageInline {
    /// Bold, emphasis and inline code render; links and images don't.
    static func inert(_ text: String) -> AttributedString {
        // ASVS 1.5.2 and 15.3.1: model output is untrusted. Markdown is parsed for
        // inline emphasis only, then every link and image attribute is removed so
        // nothing in a reply can open a URL or load a file.
        var value = (try? AttributedString(
            markdown: text,
            options: .init(
                interpretedSyntax: .inlineOnlyPreservingWhitespace,
                failurePolicy: .returnPartiallyParsedIfPossible
            )
        )) ?? AttributedString(text)
        var ranges: [Range<AttributedString.Index>] = []
        for run in value.runs where run.link != nil || run.imageURL != nil {
            ranges.append(run.range)
        }
        for range in ranges {
            value[range].link = nil
            value[range].imageURL = nil
        }
        return value
    }
}

/// One block of a message: the Markdown subset the transcript renders.
struct MessageBlock: Equatable {
    enum Kind: Equatable {
        case paragraph
        case heading(level: Int)
        case bullet(indent: Int)
        case numbered(Int, indent: Int)
        case task(checked: Bool, indent: Int)
        case code(language: String?)
        case rule
    }

    let kind: Kind
    let text: String

    static func parse(_ text: String) -> [MessageBlock] {
        var result: [MessageBlock] = []
        var paragraph: [String] = []
        var fence: (marker: Character, language: String?, lines: [String])?

        func flush() {
            if !paragraph.isEmpty {
                result.append(MessageBlock(kind: .paragraph, text: paragraph.joined(separator: "\n")))
            }
            paragraph = []
        }

        for line in text.components(separatedBy: "\n") {
            let spaces = line.prefix(while: { $0 == " " || $0 == "\t" })
            let body = line.dropFirst(spaces.count)

            if var open = fence {
                let marker = String(repeating: open.marker, count: 3)
                if body.hasPrefix(marker), body.allSatisfy({ $0 == open.marker || $0.isWhitespace }) {
                    result.append(MessageBlock(kind: .code(language: open.language), text: open.lines.joined(separator: "\n")))
                    fence = nil
                } else {
                    open.lines.append(line)
                    fence = open
                }
                continue
            }
            if body.hasPrefix("```") || body.hasPrefix("~~~"), let marker = body.first {
                flush()
                let info = body.drop(while: { $0 == marker }).split(separator: " ").first.map(String.init)
                fence = (marker, info.flatMap(validLanguage), [])
                continue
            }
            if body.trimmingCharacters(in: .whitespaces).isEmpty {
                flush()
                continue
            }
            if let block = lineBlock(body, indent: indentLevel(spaces)) {
                flush()
                result.append(block)
            } else {
                paragraph.append(line)
            }
        }
        if let open = fence {
            // A fence still streaming keeps everything so far, trailing newline included.
            result.append(MessageBlock(kind: .code(language: open.language), text: open.lines.joined(separator: "\n")))
        }
        flush()
        return result
    }

    /// A heading, rule or list item on one line, or nil for paragraph text.
    private static func lineBlock(_ body: Substring, indent: Int) -> MessageBlock? {
        let hashes = body.prefix(while: { $0 == "#" }).count
        if (1 ... 6).contains(hashes), body.dropFirst(hashes).first == " " {
            return MessageBlock(kind: .heading(level: hashes), text: String(body.dropFirst(hashes + 1)))
        }
        let trimmed = body.trimmingCharacters(in: .whitespaces)
        if trimmed.count >= 3, let first = trimmed.first, "-*_".contains(first),
           trimmed.allSatisfy({ $0 == first })
        {
            return MessageBlock(kind: .rule, text: "")
        }
        if let first = body.first, "-*+".contains(first), body.dropFirst().first == " " {
            let rest = body.dropFirst(2)
            for (box, checked) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] where rest.hasPrefix(box) {
                return MessageBlock(kind: .task(checked: checked, indent: indent), text: String(rest.dropFirst(4)))
            }
            return MessageBlock(kind: .bullet(indent: indent), text: String(rest))
        }
        let digits = body.prefix(while: { $0.isASCII && $0.isNumber })
        if (1 ... 4).contains(digits.count), let number = Int(digits) {
            let rest = body.dropFirst(digits.count)
            if let mark = rest.first, mark == "." || mark == ")", rest.dropFirst().first == " " {
                return MessageBlock(kind: .numbered(number, indent: indent), text: String(rest.dropFirst(2)))
            }
        }
        return nil
    }

    /// Two spaces (or half a tab) per level, at most three levels.
    private static func indentLevel(_ whitespace: Substring) -> Int {
        let width = whitespace.reduce(0) { $0 + ($1 == "\t" ? 4 : 1) }
        return min(width / 2, 3)
    }

    /// A fence's language, kept only when it looks like one.
    private static func validLanguage(_ word: String) -> String? {
        guard (1 ... 20).contains(word.count),
              word.unicodeScalars.allSatisfy({ scalar in
                  scalar.isASCII && (CharacterSet.alphanumerics.contains(scalar) || "+#._-".unicodeScalars.contains(scalar))
              })
        else { return nil }
        return word
    }
}

#if DEBUG
#Preview("Reply with code") {
    MessageText(text: """
    # Changes ready to review

    The navigation keeps **your place** when you return to a task.

    1. Open `Changes` to review the files.
    2. Send a message to ask for another adjustment.

    ```swift
    let message = "A long code line stays readable without widening the whole conversation or hiding the next action."
    ```
    """)
    .padding(24)
    .frame(width: 480)
}
#endif
