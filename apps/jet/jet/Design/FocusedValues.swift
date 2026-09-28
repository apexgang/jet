import SwiftUI

private struct IsEditingTextKey: FocusedValueKey {
    typealias Value = Bool
}

extension FocusedValues {
    /// True while a text input has keyboard focus. Menu commands that act on the
    /// selection (⌘⌫ Move to Jet Trash…) stay disabled while it is set.
    var isEditingText: Bool? {
        get { self[IsEditingTextKey.self] }
        set { self[IsEditingTextKey.self] = newValue }
    }
}

extension View {
    /// Reports that this text input is being edited. Apply it to the text input or
    /// its nearest container, passing the input's focus state.
    func reportsTextEditing(_ isFocused: Bool) -> some View {
        focusedValue(\.isEditingText, isFocused ? true : nil)
    }
}
