import SwiftUI

struct ChangesWorkView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                WorkSectionHeader(
                    title: "Changed files",
                    detail: "\(session.workDiff?.scope.label ?? "Current") checkpoint · \(session.workDiff?.totalFiles ?? 0) files"
                ) {
                    Button("Refresh") { Task { await session.loadWorkPanel() } }
                        .disabled(session.workOperation != nil)
                }
                .id("changes-heading")

                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        Picker("Checkpoint", selection: $session.checkpointKind) {
                            ForEach(WorkCheckpointKind.allCases, id: \.self) { kind in
                                Text(kind.title)
                                    .tag(kind)
                                    .disabled(kind == .final && session.selectedRun?.lifecycle.isLive == true)
                            }
                        }
                        .pickerStyle(.menu)
                        Spacer(minLength: 4)
                        Button("Apply") {
                            Task { await session.applyWorkCheckpoint() }
                        }
                        .disabled(!session.canApplyWorkCheckpoint)
                    }
                    switch session.checkpointKind {
                    case .turn:
                        TextField("Turn", value: $session.checkpointTurn, format: .number)
                            .textFieldStyle(.roundedBorder)
                    case .historical:
                        HStack(spacing: 8) {
                            TextField("From", value: $session.checkpointFromTurn, format: .number)
                            TextField("To", value: $session.checkpointToTurn, format: .number)
                        }
                        .textFieldStyle(.roundedBorder)
                    case .current, .final:
                        EmptyView()
                    }
                }
                .controlSize(.small)
                .padding(.horizontal, 14)
                .padding(.bottom, 10)

                if session.workFiles.isEmpty {
                    WorkPanelEmptyState(
                        title: "No changes recorded",
                        message: "The selected Run has not produced a file change at this checkpoint.",
                        symbol: "checkmark.circle"
                    )
                    .frame(minHeight: 180)
                } else {
                    ForEach(session.workFiles) { file in
                        Button {
                            Task { await session.selectWorkFile(file.path) }
                        } label: {
                            HStack(spacing: 8) {
                                Text(file.status.prefix(1).uppercased())
                                    .font(.caption2.bold().monospaced())
                                    .foregroundStyle(statusColor(file.status))
                                    .frame(width: 16)
                                Text(file.path)
                                    .font(.caption)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                Spacer(minLength: 4)
                                Text(file.origin)
                                    .font(.caption2)
                                    .foregroundStyle(.tertiary)
                            }
                            .contentShape(Rectangle())
                            .padding(.horizontal, 14)
                            .padding(.vertical, 7)
                            .background(
                                session.selectedWorkFilePath == file.path
                                    ? Color.accentColor.opacity(0.12)
                                    : Color.clear
                            )
                        }
                        .buttonStyle(.plain)
                        .id("file-\(file.path)")
                    }
                    if session.workNextPage != nil {
                        Button("Load more files") {
                            Task { await session.loadMoreWorkFiles() }
                        }
                        .disabled(session.workOperation != nil)
                        .frame(maxWidth: .infinity)
                        .padding(12)
                    }
                }

                Divider().padding(.top, 6)
                WorkSectionHeader(
                    title: "Patch",
                    detail: ByteCountFormatter.string(
                        fromByteCount: Int64(session.workDiff?.artifact.size ?? 0),
                        countStyle: .file
                    ) + " retained artifact"
                )
                .id("patch-heading")

                if patchIsBinary {
                    WorkPanelEmptyState(
                        title: "Binary change",
                        message: "Jet records the file change, but this patch is not readable as text.",
                        symbol: "doc.badge.ellipsis"
                    )
                    .frame(minHeight: 160)
                } else if session.workPatch.isEmpty {
                    Text("No text patch is available.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(16)
                } else {
                    ScrollView(.horizontal) {
                        Text(session.workPatch)
                            .font(.system(.caption2, design: .monospaced))
                            .textSelection(.enabled)
                            .fixedSize(horizontal: true, vertical: false)
                            .padding(14)
                    }
                    .frame(maxHeight: 420)
                    .background(Color.primary.opacity(0.035))
                }

                if let diff = session.workDiff,
                   diff.patchTruncated,
                   session.workPatchBytesLoaded < diff.artifact.size
                {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(artifactMessage(diff.artifact.availability))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        if diff.artifact.availability == .stored {
                            Button("Load next chunk") {
                                Task { await session.loadMorePatch() }
                            }
                            .disabled(session.workOperation != nil)
                        }
                    }
                    .padding(14)
                    .id("artifact-state")
                }

                Divider().padding(.top, 6)
                DeliveryWorkView(session: session)
                    .id("delivery-heading")
            }
            .scrollTargetLayout()
        }
        .scrollPosition(id: scrollAnchor)
    }

    private var scrollAnchor: Binding<String?> {
        Binding(
            get: { session.workScrollAnchors[.changes] },
            set: { session.workScrollAnchors[.changes] = $0 }
        )
    }

    private var patchIsBinary: Bool {
        session.workPatch.contains("GIT binary patch")
            || session.workPatch.contains("Binary files")
    }

    private func statusColor(_ status: String) -> Color {
        switch status {
        case "added": .green
        case "deleted": .red
        default: .orange
        }
    }

    private func artifactMessage(_ availability: JetArtifactAvailability) -> String {
        switch availability {
        case .stored: "The complete patch can be loaded in verified chunks."
        case .diskPressure: "The full patch was not retained because storage is constrained."
        case .runBudgetExceeded: "The Run reached its retained-artifact budget."
        case .artifactSizeExceeded: "The complete patch exceeds the artifact size limit."
        }
    }
}
