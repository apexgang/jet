import Testing
@testable import jet

@MainActor
struct MessageBlockTests {
    @Test func streamedUnclosedFencePreservesNewlinesAndUntrustedText() {
        #expect(MessageBlock.parse("# Result\n\n```html\n<script>\n  example\n") == [
            .init(kind: .heading(level: 1), text: "Result"),
            .init(kind: .code(language: "html"), text: "<script>\n  example\n"),
        ])
    }

    @Test func keepsSourceLinksInertAndGroupsParagraphs() {
        #expect(MessageBlock.parse("[open](file:///tmp/a)\nnext line\n\n- one") == [
            .init(kind: .paragraph, text: "[open](file:///tmp/a)\nnext line"),
            .init(kind: .bullet(indent: 0), text: "one"),
        ])
    }

    @Test func numberedItemsKeepTheirNumbers() {
        #expect(MessageBlock.parse("1. First\n2) Second\n12345. Not a list") == [
            .init(kind: .numbered(1, indent: 0), text: "First"),
            .init(kind: .numbered(2, indent: 0), text: "Second"),
            .init(kind: .paragraph, text: "12345. Not a list"),
        ])
    }

    @Test func tasksAreCheckedOrNot() {
        #expect(MessageBlock.parse("- [x] Done\n* [ ] Open\n+ [X] Also done\n- [] Plain") == [
            .init(kind: .task(checked: true, indent: 0), text: "Done"),
            .init(kind: .task(checked: false, indent: 0), text: "Open"),
            .init(kind: .task(checked: true, indent: 0), text: "Also done"),
            .init(kind: .bullet(indent: 0), text: "[] Plain"),
        ])
    }

    @Test func indentIsTwoSpacesPerLevelUpToThree() {
        #expect(MessageBlock.parse("- a\n  - b\n    1. c\n          - d") == [
            .init(kind: .bullet(indent: 0), text: "a"),
            .init(kind: .bullet(indent: 1), text: "b"),
            .init(kind: .numbered(1, indent: 2), text: "c"),
            .init(kind: .bullet(indent: 3), text: "d"),
        ])
    }

    @Test func fenceLanguagesMustLookLikeLanguages() {
        #expect(MessageBlock.parse("```ts extra words\nlet a = 1\n```") == [
            .init(kind: .code(language: "ts"), text: "let a = 1"),
        ])
        #expect(MessageBlock.parse("```c++\nx\n```\n```objective-c\ny\n```") == [
            .init(kind: .code(language: "c++"), text: "x"),
            .init(kind: .code(language: "objective-c"), text: "y"),
        ])
        #expect(MessageBlock.parse("```<img src=x>\nz\n```\n```\nw\n```") == [
            .init(kind: .code(language: nil), text: "z"),
            .init(kind: .code(language: nil), text: "w"),
        ])
        #expect(MessageBlock.parse("```averyveryverylonglanguagename\nq\n```") == [
            .init(kind: .code(language: nil), text: "q"),
        ])
    }

    @Test func tildeFencesCloseOnlyWithTildes() {
        #expect(MessageBlock.parse("~~~python\nprint(1)\n```\n~~~\nafter") == [
            .init(kind: .code(language: "python"), text: "print(1)\n```"),
            .init(kind: .paragraph, text: "after"),
        ])
    }

    @Test func rulesAndHeadingLevels() {
        #expect(MessageBlock.parse("## Plan\n---\n***\n___\n####### Seven\n#NoSpace") == [
            .init(kind: .heading(level: 2), text: "Plan"),
            .init(kind: .rule, text: ""),
            .init(kind: .rule, text: ""),
            .init(kind: .rule, text: ""),
            .init(kind: .paragraph, text: "####### Seven\n#NoSpace"),
        ])
    }
}
