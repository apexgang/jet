import SwiftUI

/// A small inert document renderer. Model output cannot create links, load images,
/// or execute markup. Source text remains selectable, including code fences.
struct MessageText: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(MessageBlock.parse(text).enumerated()), id: \.offset) { _, block in
                switch block.kind {
                case .paragraph:
                    Text(block.text).fixedSize(horizontal: false, vertical: true)
                case .heading:
                    Text(block.text).font(.headline).padding(.top, 4)
                case .item:
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("•").accessibilityHidden(true)
                        Text(block.text).fixedSize(horizontal: false, vertical: true)
                    }
                case .code:
                    ScrollView(.horizontal) {
                        Text(block.text).font(.system(.callout, design: .monospaced)).padding(12)
                    }
                    .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 4))
                }
            }
        }
        .textSelection(.enabled)
    }
}

struct MessageBlock: Equatable {
    enum Kind { case paragraph, heading, item, code }
    let kind: Kind
    let text: String

    static func parse(_ text: String) -> [MessageBlock] {
        var result: [MessageBlock] = []
        var lines: [String] = []
        var fenced = false
        func flush() {
            if !lines.isEmpty { result.append(.init(kind: fenced ? .code : .paragraph, text: lines.joined(separator: "\n"))) }
            lines = []
        }
        for line in text.components(separatedBy: "\n") {
            if line.hasPrefix("```") { flush(); fenced.toggle() }
            else if fenced { lines.append(line) }
            else if line.hasPrefix("- ") || line.hasPrefix("* ") {
                flush(); result.append(.init(kind: .item, text: String(line.dropFirst(2))))
            } else if let space = line.firstIndex(of: " "), (1...6).contains(line.distance(from: line.startIndex, to: space)), line[..<space].allSatisfy({ $0 == "#" }) {
                flush(); result.append(.init(kind: .heading, text: String(line[line.index(after: space)...])))
            } else if line.trimmingCharacters(in: .whitespaces).isEmpty { flush() }
            else { lines.append(line) }
        }
        flush()
        return result
    }
}

#Preview("Transcript with long code") {
    MessageText(text: """
    # Changes ready to review

    The navigation keeps your place when you return to a task.

    - Open Changes to review the files.
    - Send a message to request another adjustment.

    ```swift
    let message = "A long code line remains readable without widening the whole conversation or hiding the next action."
    ```
    """)
    .padding(24).frame(width: 480)
}
