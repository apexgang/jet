import SwiftUI

struct RecoveryIssue: View {
    let error: JetPresentationError?

    var body: some View {
        if let error {
            VStack(alignment: .leading, spacing: 3) {
                Text(error.message)
                Text(error.code).font(.caption.monospaced())
            }
            .foregroundStyle(.orange)
        }
    }
}
