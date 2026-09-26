import SwiftUI

/// Shared roles, expressed with native controls and system surfaces on Apple platforms.
enum JetDesign {
    static let accent = Color("AccentColor")
    static let readingWidth: CGFloat = 760
    static let writingWidth: CGFloat = 640
    static let smallGap: CGFloat = 8
    static let gap: CGFloat = 16
    static let sectionGap: CGFloat = 24
    static let controlRadius: CGFloat = 5
    static let fieldRadius: CGFloat = 8
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
            context.fill(path, with: .color(JetDesign.accent))
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}
