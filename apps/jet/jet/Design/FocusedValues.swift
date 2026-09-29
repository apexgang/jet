import SwiftUI

private struct IsEditingTextKey: FocusedValueKey {
    typealias Value = Bool
}

private struct IsComposerFocusedKey: FocusedValueKey {
    typealias Value = Bool
}

private struct HasOpenDialogKey: FocusedValueKey {
    typealias Value = Bool
}

private struct IsMainWindowKey: FocusedValueKey {
    typealias Value = Bool
}

extension FocusedValues {
    /// True while a text input has keyboard focus. Menu commands that act on the
    /// selection (⌘⌫ Move to Jet Trash…) stay disabled while it is set.
    var isEditingText: Bool? {
        get { self[IsEditingTextKey.self] }
        set { self[IsEditingTextKey.self] = newValue }
    }

    /// True while the composer's field has keyboard focus. Task › Send (⌘↩) stays
    /// available there and is disabled in every other text input.
    var isComposerFocused: Bool? {
        get { self[IsComposerFocusedKey.self] }
        set { self[IsComposerFocusedKey.self] = newValue }
    }

    /// True while the main window shows a dialog or popover that the session doesn't
    /// track, such as Revert… or a comment popover. Set it with
    /// `focusedSceneValue(\.hasOpenDialog, true)`; Interrupt (⌘.) waits until it closes.
    var hasOpenDialog: Bool? {
        get { self[HasOpenDialogKey.self] }
        set { self[HasOpenDialogKey.self] = newValue }
    }

    /// Set by the main window's shell as a scene value. The Task and Changes menus
    /// act only while that window is focused, never from the Settings window.
    var isMainWindow: Bool? {
        get { self[IsMainWindowKey.self] }
        set { self[IsMainWindowKey.self] = newValue }
    }
}

extension View {
    /// Reports that this text input is being edited. Apply it to the text input or
    /// its nearest container, passing the input's focus state.
    func reportsTextEditing(_ isFocused: Bool) -> some View {
        focusedValue(\.isEditingText, isFocused ? true : nil)
    }

    /// The composer's `reportsTextEditing(_:)`: it also reports that the composer is
    /// the field being edited, so Task › Send (⌘↩) stays available while typing there.
    func reportsComposerEditing(_ isFocused: Bool) -> some View {
        focusedValue(\.isEditingText, isFocused ? true : nil)
            .focusedValue(\.isComposerFocused, isFocused ? true : nil)
    }
}
