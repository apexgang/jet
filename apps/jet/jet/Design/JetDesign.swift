import SwiftUI

/// Shared roles, expressed with native controls and system surfaces on Apple platforms.
enum JetDesign {
    /// Copper for fills: the single prominent action, selection, progress and the
    /// focus ring. Scene roots force it with `.tint(JetDesign.accent)`.
    static let accent = Color("AccentColor")
    /// Copper for text and glyphs on plain surfaces, such as JetMark and the unread
    /// dot. It stays light enough to read on dark surfaces; `accent` is for fills,
    /// selection and the focus ring.
    static let accentText = Color("AccentText")
    static let readingWidth: CGFloat = 760
    static let writingWidth: CGFloat = 640
    static let smallGap: CGFloat = 8
    static let gap: CGFloat = 16
    static let sectionGap: CGFloat = 24
    static let controlRadius: CGFloat = 5
    static let fieldRadius: CGFloat = 8

    /// The type scale in points (design §7). Nothing goes below 10 pt.
    enum TextSize {
        static let metadata: CGFloat = 11
        static let control: CGFloat = 12
        static let navigation: CGFloat = 13
        /// Transcript text before the person's text-size scale.
        static let content: CGFloat = 14
        static let title: CGFloat = 17
        /// "What are you working on?" on New Task.
        static let invitation: CGFloat = 24
    }

    /// AppStorage key for the transcript text-size scale (View › Bigger / Smaller).
    static let transcriptScaleKey = "jet.transcript.text-scale"
    /// The transcript text-size steps, from 85% to 200%. 1.0 is Actual Size.
    static let transcriptScaleSteps: [Double] = [0.85, 1.0, 1.15, 1.3, 1.5, 1.75, 2.0]

    /// Added diff lines. The tint strengthens when Increase Contrast is on; the
    /// +/− gutter still carries the meaning.
    static func diffAddedBackground(contrast: ColorSchemeContrast) -> Color {
        Color.green.opacity(contrast == .increased ? 0.26 : 0.14)
    }

    /// Removed diff lines. See `diffAddedBackground(contrast:)`.
    static func diffRemovedBackground(contrast: ColorSchemeContrast) -> Color {
        Color.red.opacity(contrast == .increased ? 0.26 : 0.14)
    }
}

private struct TranscriptScaleKey: EnvironmentKey {
    static let defaultValue: CGFloat = 1
}

extension EnvironmentValues {
    /// The transcript's text-size multiplier, applied to `JetDesign.TextSize.content`.
    var transcriptScale: CGFloat {
        get { self[TranscriptScaleKey.self] }
        set { self[TranscriptScaleKey.self] = newValue }
    }
}

struct JetMark: View {
    var size: CGFloat = 32
    var body: some View {
        Canvas { context, bounds in
            let scale = bounds.width / 32
            var path = Path()
            for points in [[CGPoint(x: 5, y: 22), CGPoint(x: 14, y: 7), CGPoint(x: 20, y: 7), CGPoint(x: 11, y: 22)],
                           [CGPoint(x: 15, y: 25), CGPoint(x: 24, y: 10), CGPoint(x: 28, y: 10), CGPoint(x: 19, y: 25)]] {
                path.move(to: CGPoint(x: points[0].x * scale, y: points[0].y * scale))
                for point in points.dropFirst() { path.addLine(to: CGPoint(x: point.x * scale, y: point.y * scale)) }
                path.closeSubpath()
            }
            context.fill(path, with: .color(JetDesign.accentText))
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}
