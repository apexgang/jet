import Testing
@testable import jet

struct MessageBlockTests {
    @Test func streamedUnclosedFencePreservesNewlinesAndUntrustedText() {
        #expect(MessageBlock.parse("# Result\n\n```html\n<script>\n  example\n") == [
            .init(kind: .heading, text: "Result"),
            .init(kind: .code, text: "<script>\n  example\n"),
        ])
    }
    @Test func keepsSourceLinksInertAndGroupsParagraphs() {
        #expect(MessageBlock.parse("[open](file:///tmp/a)\nnext line\n\n- one") == [
            .init(kind: .paragraph, text: "[open](file:///tmp/a)\nnext line"),
            .init(kind: .item, text: "one"),
        ])
    }
}
