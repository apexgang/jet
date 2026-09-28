import SwiftUI

struct FilesWorkView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(spacing: 0) {
            WorkSectionHeader(
                title: "Workspace file",
                detail: session.editableFile?.path ?? "Choose a changed file"
            ) {
                if !session.workFiles.isEmpty {
                    Menu("Choose") {
                        ForEach(session.workFiles) { file in
                            Button(file.path) { Task { await session.selectWorkFile(file.path) } }
                        }
                    }
                }
            }
            Divider()

            if session.selectedWorkFilePath == nil {
                WorkPanelEmptyState(
                    title: "No file selected",
                    message: "Choose a file in Changes. Only files from this Run can be opened here.",
                    symbol: "doc"
                )
            } else if session.workOperation == "file", session.editableFile == nil {
                ProgressView("Loading file")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let file = session.editableFile, file.content != nil {
                VStack(spacing: 0) {
                    HStack {
                        Text("\(file.content?.utf8.count ?? 0) bytes")
                        Spacer()
                        Text("Revision bound")
                            .help(file.revision.label)
                    }
                    .font(.caption2.monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)

                    TextEditor(text: $session.fileDraft)
                        .font(.system(.caption, design: .monospaced))
                        .scrollContentBackground(.hidden)
                        .padding(8)
                        .background(Color.primary.opacity(0.035))
                        .accessibilityLabel("Edit \(file.path)")

                    HStack {
                        Button("Reload") { Task { await session.selectWorkFile(file.path) } }
                        Spacer()
                        Button("Save Edit") { Task { await session.saveSelectedWorkFile() } }
                            .buttonStyle(.borderedProminent)
                            .disabled(session.workOperation != nil || session.fileDraft == file.content)
                    }
                    .padding(12)

                    Divider()
                    VStack(alignment: .leading, spacing: 9) {
                        Text("Review comment").font(.subheadline.weight(.semibold))
                        TextField("Line", value: $session.reviewLine, format: .number)
                            .textFieldStyle(.roundedBorder)
                        TextField(
                            "Describe the issue or requested change",
                            text: $session.reviewComment,
                            axis: .vertical
                        )
                        .lineLimit(3 ... 6)
                        .textFieldStyle(.roundedBorder)
                        HStack {
                            Spacer()
                            Button("Add Review Comment") {
                                Task { await session.submitSelectedReview() }
                            }
                            .disabled(
                                session.workOperation != nil
                                    || session.reviewLine == 0
                                    || session.reviewComment.trimmingCharacters(
                                        in: .whitespacesAndNewlines
                                    ).isEmpty
                            )
                        }
                    }
                    .padding(14)
                }
            } else {
                WorkPanelEmptyState(
                    title: "Content unavailable",
                    message: "The file may be binary, oversized, deleted, or no longer available as bounded UTF-8 text.",
                    symbol: "doc.badge.ellipsis"
                )
            }
        }
    }
}
