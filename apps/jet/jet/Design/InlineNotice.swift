import SwiftUI

/// One line of feedback: a symbol, its colour, the text and at most one bordered
/// action (design §6.6 "Notice line"). Colour always comes with a symbol and text.
struct InlineNotice: View {
    let notice: ComposerNotice
    let perform: @MainActor (ComposerNotice.Action) -> Void

    @ScaledMetric(relativeTo: .callout) private var textSize = JetDesign.TextSize.control

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Image(systemName: Self.systemImage(for: notice.kind))
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(Self.tint(for: notice.kind))
                Text(notice.text)
                    .foregroundStyle(notice.kind == .warning || notice.kind == .error ? .primary : .secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Text(accessibilityText))

            Spacer(minLength: 8)

            if let action = notice.action {
                Button(action.title) { perform(action) }
                    .buttonStyle(.bordered)
                    .controlSize(.regular)
            }
        }
        .font(.system(size: textSize))
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    static func systemImage(for kind: ComposerNotice.Kind) -> String {
        switch kind {
        case .info: "info.circle"
        case .confirmation: "checkmark.circle"
        case .warning: "exclamationmark.triangle.fill"
        case .error: "xmark.octagon.fill"
        }
    }

    static func tint(for kind: ComposerNotice.Kind) -> Color {
        switch kind {
        case .info, .confirmation: .secondary
        case .warning: .orange
        case .error: .red
        }
    }

    private var accessibilityText: String {
        switch notice.kind {
        case .info, .confirmation: notice.text
        case .warning: String(localized: "Warning: \(notice.text)")
        case .error: String(localized: "Error: \(notice.text)")
        }
    }
}
