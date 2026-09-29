import SwiftUI

/// Changes in edit mode: one changed file in a plain text editor. Saving is bound
/// to the revision the file was opened at; every exit asks about unsaved edits.
struct DetailsFileEditor: View {
    @Bindable var session: DesktopSession
    @Bindable var model: DetailsPanelModel

    @FocusState private var editorFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, 12)
                .padding(.top, 4)
                .padding(.bottom, 6)
            Text(caption)
                .font(.system(size: JetDesign.TextSize.metadata))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var path: String { session.selectedWorkFilePath ?? session.editableFile?.path ?? "" }
    private var name: String { DesktopSession.detailsFileName(path) }
    private var folder: String { (path as NSString).deletingLastPathComponent }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: session.hasUnsavedFileEdit ? String(localized: "\(name) — Edited") : name)
                    .font(.headline)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if !folder.isEmpty {
                    Text(verbatim: folder)
                        .font(.system(size: JetDesign.TextSize.metadata))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            Spacer(minLength: 8)
            Button("Done") { session.finishEditingFile(reload: model.savedDuringEdit) }
                .accessibilityIdentifier("file-edit-done")
        }
    }

    private var caption: String {
        let project = session.detailsProjectName
        if session.detailsWorksInProjectFolder {
            return project.map { String(localized: "You're editing this file in your \($0) folder.") }
                ?? String(localized: "You're editing this file in your project folder.")
        }
        return project.map {
            String(localized: "You're editing the working copy. Your \($0) folder doesn't change until you keep the changes.")
        } ?? String(localized: "You're editing the working copy. Your project folder doesn't change until you keep the changes.")
    }

    @ViewBuilder
    private var content: some View {
        if let file = session.editableFile, file.content != nil {
            TextEditor(text: $session.fileDraft)
                .font(.system(size: JetDesign.TextSize.control, design: .monospaced))
                .scrollContentBackground(.hidden)
                .autocorrectionDisabled()
                .focused($editorFocused)
                .reportsTextEditing(editorFocused)
                .padding(6)
                .background(.background, in: RoundedRectangle(cornerRadius: JetDesign.fieldRadius))
                .overlay {
                    RoundedRectangle(cornerRadius: JetDesign.fieldRadius)
                        .strokeBorder(.separator, lineWidth: 1)
                }
                .accessibilityLabel(Text("Contents of \(name)"))
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
        } else if session.editableFile == nil {
            ProgressView("Opening \(name)…")
        } else {
            ContentUnavailableView {
                Label("Can't Edit This File", systemImage: "doc.badge.ellipsis")
            } description: {
                Text("Jet can edit text files up to 128 KB.")
            } actions: {
                Button("Done") { session.finishEditingFile(reload: model.savedDuringEdit) }
            }
        }
    }
}

/// Edit mode's footer: Revert…, then Save (⌘S).
struct DetailsEditorFooter: View {
    @Bindable var session: DesktopSession
    @Bindable var model: DetailsPanelModel

    @State private var confirmsRevert = false

    var body: some View {
        HStack(spacing: 8) {
            Button("Revert…") { confirmsRevert = true }
                .disabled(!session.hasUnsavedFileEdit || session.workOperation != nil)
                .accessibilityIdentifier("file-edit-revert")
                .confirmationDialog(
                    Text("Discard your edits to \(name)?"),
                    isPresented: $confirmsRevert,
                    titleVisibility: .visible
                ) {
                    Button("Discard Edits", role: .destructive) { session.revertFileEdits() }
                    Button("Cancel", role: .cancel) {}
                } message: {
                    Text("This can't be undone.")
                }
                .focusedSceneValue(\.hasOpenDialog, confirmsRevert ? true : nil)
            Spacer(minLength: 0)
            Button(session.workOperation == "save" ? String(localized: "Saving…") : String(localized: "Save")) {
                Task {
                    if await session.saveEditedFile() { model.savedDuringEdit = true }
                }
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut("s")
            .disabled(!canSave)
            .help(saveHelp)
            .accessibilityIdentifier("file-edit-save")
        }
        .padding(12)
    }

    private var name: String {
        DesktopSession.detailsFileName(session.editableFile?.path ?? session.selectedWorkFilePath ?? "")
    }

    /// Why Save is unavailable, or what it does.
    private var saveHelp: String {
        if session.detailsIsOffline { return String(localized: "Reconnect to save.") }
        if session.workOperation == "save" { return String(localized: "Saving your edits…") }
        if !session.hasUnsavedFileEdit { return String(localized: "No edits to save yet.") }
        return String(localized: "Save your edits to \(name) (⌘S)")
    }

    private var canSave: Bool {
        session.hasUnsavedFileEdit
            && session.workOperation == nil
            && !session.detailsIsOffline
            && session.editableFile?.content != nil
    }
}

#if DEBUG
#Preview { DesktopPreviewScenes.details[0].makeView() }
#endif
