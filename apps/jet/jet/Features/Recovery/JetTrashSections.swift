import SwiftUI

struct JetTrashSection: View {
    @Bindable var model: JetRecoveryModel
    let planeName: String
    let selectedTitle: String?
    let conversationTitles: [UUID: String]
    @State private var pendingAction: JetRetentionAction?
    @State private var pendingRestore: JetTrashEntry?

    // ASVS 2.3.1: do not offer a mutation while the Plane is in Recovery mode.
    private var isReadOnly: Bool { model.health?.recoveryState == "read_only" }

    @ViewBuilder
    var body: some View {
        selectedTask
        Section("Jet Trash") {
            Text("Entries are held on \(planeName) until their shown expiry. Restore before expiry to keep a task.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if model.isLoading && model.trash == nil { ProgressView("Loading Jet Trash") }
            if model.trash != nil && model.issues[.trash] != nil {
                Text("Showing the last observed Trash state.").font(.caption).foregroundStyle(.orange)
            }
            if let entries = model.trash?.entries {
                if entries.isEmpty { Text("Jet Trash is empty.").foregroundStyle(.secondary) }
                ForEach(entries) { entry in
                    VStack(alignment: .leading, spacing: 5) {
                        HStack {
                            Text(title(for: entry.conversationID)).font(.headline)
                            Spacer()
                            if entry.canRestore {
                                Button("Restore") { pendingRestore = entry }
                                    .disabled(isReadOnly || model.operation != nil || model.issues[.trash] != nil)
                                    .accessibilityIdentifier("trash-restore-\(entry.id.uuidString)")
                            }
                        }
                        Text("Reason: \(entry.reason.replacingOccurrences(of: "_", with: " "))")
                        Text("Expires \(entry.expiresAt.formatted(date: .abbreviated, time: .shortened))")
                        if !entry.canRestore { Text("This transfer entry cannot be restored here.") }
                    }
                    .font(.caption)
                    .padding(.vertical, 3)
                }
            }
            RecoveryIssue(error: model.issues[.trash])
        }
        .confirmationDialog(
            "Restore this task?",
            isPresented: Binding(get: { pendingRestore != nil }, set: { if !$0 { pendingRestore = nil } }),
            titleVisibility: .visible
        ) {
            if let entry = pendingRestore {
                Button("Restore on \(planeName)") {
                    pendingRestore = nil
                    Task { await model.restore(entry.conversationID) }
                }
            }
            Button("Cancel", role: .cancel) { pendingRestore = nil }
        } message: {
            Text("Jet will remove this task from Trash on \(planeName).")
        }
    }

    var selectedTask: some View {
        Section("Remove current task") {
            if isReadOnly {
                Text("This Plane is read-only. Restore a verified snapshot in Safety & System before changing tasks.")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
            if let conversationID = model.selectedConversationID {
                Text(selectedTitle ?? conversationID.uuidString).font(.headline)
                if let preview = model.preview, preview.conversationID == conversationID {
                    if let trash = preview.trash {
                        Text("Already in Jet Trash until \(trash.expiresAt.formatted(date: .abbreviated, time: .shortened)).")
                    } else {
                        if !preview.protections.isEmpty {
                            Text("Current protections: \(preview.protections.map { $0.replacingOccurrences(of: "_", with: " ") }.joined(separator: ", ")).")
                                .foregroundStyle(.secondary)
                        }
                        Text("\(preview.auditRecords.formatted()) content-free audit records will remain until audit retention ends.")
                            .font(.caption)
                        HStack {
                            Button("Forget on this Plane") { pendingAction = .forget }
                                .disabled(isReadOnly || model.operation != nil || model.issues[.preview] != nil || preview.blocksForget)
                                .accessibilityIdentifier("retention-forget")
                            Button("Delete everywhere", role: .destructive) { pendingAction = .deleteEverywhere }
                                .disabled(isReadOnly || model.operation != nil || model.issues[.preview] != nil)
                                .accessibilityIdentifier("retention-delete-everywhere")
                        }
                        if preview.blocksForget {
                            Text("Finish the active Run and queued turns before forgetting. Delete everywhere can stop and cancel them.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                } else if model.isLoading { ProgressView("Checking task protections") }
                RecoveryIssue(error: model.issues[.preview])
            } else {
                Text("Open a task in the main window to review its removal.")
                    .foregroundStyle(.secondary)
            }
        }
        // ASVS 2.3.1: the owning view keeps this confirmation in the rendered workflow.
        .confirmationDialog(
            pendingAction == .forget ? "Forget this task?" : "Delete this task everywhere?",
            isPresented: Binding(get: { pendingAction != nil }, set: { if !$0 { pendingAction = nil } }),
            titleVisibility: .visible
        ) {
            if let action = pendingAction {
                Button(action == .forget ? "Move to Jet Trash" : "Stage deletion everywhere", role: .destructive) {
                    pendingAction = nil
                    Task { await model.stage(action) }
                }
            }
            Button("Cancel", role: .cancel) { pendingAction = nil }
        } message: {
            Text(pendingAction == .forget
                 ? "Jet will stage this task on \(planeName). At expiry, its Jet history, schedules, and managed Workspace are deleted unless you restore it. Its Harness history remains."
                 : "Jet will stage this task on \(planeName), stop its active Run, and cancel queued turns. At expiry, its Jet history and managed Workspace are deleted unless restored. This release cannot delete the Harness's native history.")
        }
    }

    private func title(for id: UUID) -> String {
        if model.selectedConversationID == id { return selectedTitle ?? id.uuidString }
        if let title = conversationTitles[id] { return title }
        return "Task \(id.uuidString.suffix(8))"
    }
}
