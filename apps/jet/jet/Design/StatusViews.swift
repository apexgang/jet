import SwiftUI

/// A task's status as a small spinner or hierarchical symbol plus text, for the
/// toolbar and Activity. `.unknown` shows nothing.
struct TaskStatusLabel: View {
    let status: TaskStatus

    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.backgroundProminence) private var prominence
    @ScaledMetric(relativeTo: .callout) private var textSize = JetDesign.TextSize.control

    var body: some View {
        if status != .unknown {
            HStack(spacing: 6) {
                TaskStatusIcon(status: status, style: iconStyle)
                    .frame(width: 16, height: 16)
                Text(status.title)
                    .foregroundStyle(textStyle)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .font(.system(size: textSize, weight: .medium))
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Text("Status"))
            .accessibilityValue(Text(status.accessibilityDescription))
        }
    }

    private var iconStyle: AnyShapeStyle {
        prominence == .increased ? AnyShapeStyle(.primary) : AnyShapeStyle(status.tint)
    }

    /// The symbol carries the colour; the text stays readable. Statuses that need
    /// attention or are in progress read in the primary style, calm ones in secondary.
    private var textStyle: AnyShapeStyle {
        if contrast == .increased || prominence == .increased { return AnyShapeStyle(.primary) }
        let emphasized = status.needsYou || status == .failed || status.showsSpinner
        return emphasized ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary)
    }
}

/// The 16-point status column of a task row: a spinner, an alert symbol, the
/// copper unread dot, or nothing (design §8 "Row glyph").
struct TaskStatusGlyph: View {
    let status: TaskStatus
    let isUnread: Bool

    @Environment(\.backgroundProminence) private var prominence

    var body: some View {
        let glyph = status.rowGlyph(isUnread: isUnread)
        ZStack {
            switch glyph {
            case .none:
                Color.clear
            case .spinner:
                TaskStatusIcon(status: status, style: style(status.tint))
            case .symbol:
                TaskStatusIcon(status: status, style: style(status.tint))
            case .unreadDot:
                Image(systemName: "circle.fill")
                    .font(.system(size: 7))
                    .foregroundStyle(style(JetDesign.accentText))
            }
        }
        .imageScale(.medium)
        .frame(width: 16, height: 16)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(accessibilityText(glyph)))
        .accessibilityHidden(glyph == .none)
    }

    /// On a prominent (selected) row, coloured glyphs would vanish into the copper
    /// highlight; the primary style adapts to it. The row's text keeps the meaning.
    private func style(_ color: Color) -> AnyShapeStyle {
        prominence == .increased ? AnyShapeStyle(.primary) : AnyShapeStyle(color)
    }

    private func accessibilityText(_ glyph: TaskStatus.RowGlyph) -> String {
        let description = glyph == .unreadDot ? "" : status.accessibilityDescription
        switch (description.isEmpty, isUnread) {
        case (false, true): return String(localized: "\(description), new reply")
        case (false, false): return description
        case (true, true): return String(localized: "New reply")
        case (true, false): return ""
        }
    }
}

/// The spinner or symbol shared by the label and the row glyph.
private struct TaskStatusIcon: View {
    let status: TaskStatus
    let style: AnyShapeStyle

    var body: some View {
        if status.showsSpinner {
            ProgressView()
                .controlSize(.mini)
                .tint(status.tint)
        } else if let symbol = status.systemImage {
            Image(systemName: symbol)
                .symbolRenderingMode(.hierarchical)
                .font(.system(size: 12))
                .imageScale(.medium)
                .foregroundStyle(style)
                .accessibilityHidden(true)
        }
    }
}
