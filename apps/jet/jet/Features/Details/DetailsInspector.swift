import SwiftUI

struct WorkPanelView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Work details").font(.headline)
                Spacer()
                Button("Close") { session.isWorkPanelPresented = false }
                    .buttonStyle(.borderless).font(.caption)
            }.padding(.horizontal, 16).padding(.top, 16)
            Picker("Work panel", selection: $session.selectedWorkPanel) {
                ForEach(WorkPanelTab.allCases, id: \.self) { tab in
                    Text(tab.title).tag(tab)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(12)

            Divider()

            Group {
                if let error = session.workError, session.selectedWorkPanel != .run {
                    ContentUnavailableView {
                        Label("Work details unavailable", systemImage: "exclamationmark.triangle")
                    } description: {
                        Text(error.message)
                    } actions: {
                        ForEach(error.recoveryActions) { action in
                            Button(action.label) {
                                Task { await session.applyWorkRecovery(action) }
                            }
                        }
                        if error.recoveryActions.isEmpty, error.retryable {
                            Button("Try Again") { Task { await session.loadWorkPanel() } }
                        }
                        JetSettingsRecoveryButton(session: session, error: error)
                    }
                    .padding()
                } else if session.workOperation == "refresh",
                          session.workDiff == nil,
                          session.selectedWorkPanel != .run
                {
                    ProgressView("Loading work details")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    switch session.selectedWorkPanel {
                    case .changes:
                        ChangesWorkView(session: session)
                    case .files:
                        FilesWorkView(session: session)
                    case .terminal:
                        TerminalWorkView(session: session)
                    case .run:
                        RunSummaryView(session: session)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            if let notice = session.workNotice {
                Divider()
                VStack(alignment: .leading, spacing: 8) {
                    Text(notice)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityLabel(notice)
                    if let error = session.workNoticeError {
                        HStack(spacing: 8) {
                            ForEach(error.recoveryActions) { action in
                                Button(action.label) {
                                    Task { await session.applyWorkRecovery(action) }
                                }
                                .controlSize(.small)
                            }
                            if let conflict = error.revisionConflict {
                                Text("Current revision \(conflict.currentRevision)")
                                    .font(.caption2)
                                    .foregroundStyle(.tertiary)
                            }
                            JetSettingsRecoveryButton(session: session, error: error)
                                .controlSize(.small)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
                .modifier(LegibleBarBackground())
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("work-panel")
    }
}

struct WorkSectionHeader<Actions: View>: View {
    let title: String
    let detail: String
    let actions: Actions

    init(
        title: String,
        detail: String,
        @ViewBuilder actions: () -> Actions
    ) {
        self.title = title
        self.detail = detail
        self.actions = actions()
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.subheadline.weight(.semibold))
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 6)
            actions
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }
}

extension WorkSectionHeader where Actions == EmptyView {
    init(title: String, detail: String) {
        self.init(title: title, detail: detail) { EmptyView() }
    }
}

struct WorkPanelEmptyState: View {
    let title: String
    let message: String
    let symbol: String

    var body: some View {
        ContentUnavailableView {
            Label(title, systemImage: symbol)
        } description: {
            Text(message)
        }
        .padding()
    }
}
