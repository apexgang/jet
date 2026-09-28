import SwiftUI
import UniformTypeIdentifiers

struct ProjectSetupView: View {
    @Bindable var session: DesktopSession
    #if os(macOS)
    @Environment(\.openSettings) private var openSettings
    #endif

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                Divider()

                switch session.setupState {
                case .idle, .loading:
                    ProgressView("Connecting to Jet on this computer…")
                        .frame(maxWidth: .infinity, minHeight: 280)
                case let .failed(error):
                    ContentUnavailableView {
                        Label("Jet could not connect", systemImage: "network.slash")
                    } description: {
                        VStack(spacing: 6) {
                            Text(error.message)
                            Text(error.code)
                                .font(.caption.monospaced())
                        }
                    } actions: {
                        Button("Try again") {
                            Task { await session.loadSetup() }
                        }
                        JetSettingsRecoveryButton(session: session, error: error)
                    }
                    .frame(maxWidth: .infinity, minHeight: 320)
                case let .ready(snapshot):
                    setupContent(snapshot)
                }
            }
            .frame(maxWidth: 820, alignment: .leading)
            .padding(.horizontal, 28)
            .padding(.bottom, 36)
            .frame(maxWidth: .infinity)
        }
        .navigationTitle("Projects")
        .fileImporter(
            isPresented: $session.isProjectImporterPresented,
            allowedContentTypes: [.folder],
            allowsMultipleSelection: false
        ) { result in
            switch result {
            case let .success(urls):
                if let url = urls.first {
                    Task { await session.previewProject(at: url) }
                }
            case .failure:
                session.setupNotice = "Jet could not read that folder selection."
            }
        }
    }

    private func showSettings(_ pane: JetSettingsPane) {
        session.requestSettings(pane)
        #if os(macOS)
        openSettings()
        #endif
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 20) {
            VStack(alignment: .leading, spacing: 5) {
                Text("Set up Jet")
                    .font(.title2.weight(.semibold))
                Text("Choose a Project and a coding assistant. You can change both later.")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 20)
            Button("Refresh") {
                Task { await session.loadSetup() }
            }
            .disabled(session.setupOperation != nil)
        }
        .padding(.vertical, 22)
    }

    @ViewBuilder
    private func setupContent(_ snapshot: JetSetupSnapshot) -> some View {
        VStack(alignment: .leading, spacing: 32) {
            SetupStep(number: "01", title: "Give your work a home", detail: "Choose a Git project. Jet makes a separate working copy for each task.") {
                if let issue = snapshot.issue(for: .projects) {
                    SetupIssueView(session: session, issue: issue)
                } else {
                    ForEach(snapshot.projects.projects) { project in
                        HStack(spacing: 12) {
                            Button {
                                session.selectedProjectID = project.id
                            } label: {
                                HStack(spacing: 10) {
                                    Image(systemName: session.selectedProjectID == project.id ? "checkmark.circle.fill" : "circle")
                                        .foregroundStyle(session.selectedProjectID == project.id ? Color.accentColor : Color.secondary)
                                    VStack(alignment: .leading, spacing: 3) {
                                        Text(project.name).fontWeight(.medium)
                                        Text(project.root).font(.caption).foregroundStyle(.secondary)
                                            .lineLimit(1).truncationMode(.middle)
                                    }
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityAddTraits(session.selectedProjectID == project.id ? .isSelected : [])
                            Button("Remove…", role: .destructive) {
                                Task { await session.prepareProjectRemoval(project.id) }
                            }
                            .buttonStyle(.borderless)
                            .disabled(session.setupOperation != nil)
                        }
                        .padding(.vertical, 8)
                        Divider()
                    }
                }
                Button("Choose Folder…", systemImage: "folder.badge.plus") {
                    session.isProjectImporterPresented = true
                }
                .disabled(session.setupOperation != nil)
                if let preview = session.projectPreview {
                    ProjectPreviewView(session: session, preview: preview)
                }
            }

            SetupStep(number: "02", title: "Choose who to work with", detail: "Jet uses the assistant installed on this computer and its existing login.") {
                if let issue = snapshot.issue(for: .capabilities) {
                    SetupIssueView(session: session, issue: issue)
                } else if snapshot.capabilities.crafts.isEmpty {
                    Text("Install a supported Harness to start a task.").foregroundStyle(.secondary)
                } else {
                    Picker("Harness", selection: Binding(
                        get: { session.selectedCraftID ?? "" },
                        set: { session.chooseCraft($0) }
                    )) {
                        ForEach(snapshot.capabilities.crafts, id: \.id) { craft in
                            Text(DesktopSession.harnessLabel(craft.harnesses.first ?? craft.id)).tag(craft.id)
                        }
                    }
                    .pickerStyle(.menu)
                    .fixedSize()
                }
                if let issue = snapshot.issue(for: .accounts) {
                    SetupIssueView(session: session, issue: issue)
                }
                HStack {
                    ForEach(snapshot.capabilities.authProviders) { provider in
                        if !snapshot.accounts.bindings.contains(where: { $0.provider == provider.provider }) {
                            Button("Use \(DesktopSession.harnessLabel(provider.harness)) login") {
                                Task { await session.connectHarness(provider) }
                            }
                            .disabled(session.setupOperation != nil)
                        }
                    }
                    Button("Manage Harnesses…") { showSettings(.agents) }
                }
            }

            Divider()
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text(session.canStartTask ? "Ready when you are" : "Finish setup to start your first task")
                        .font(.headline)
                    if session.canStartTask { Text("Describe what you need. You can review every change.").foregroundStyle(.secondary) }
                }
                Spacer()
                Button("New task", action: session.beginNewTask)
                    .buttonStyle(.borderedProminent)
                    .disabled(!session.canStartTask)
            }
            DisclosureGroup("Other computers and connection details") {
                VStack(alignment: .leading, spacing: 12) {
                    LabeledContent("This computer", value: session.planeConnectionLabel)
                    LabeledContent("Service", value: "\(snapshot.capabilities.platform) · \(snapshot.capabilities.coreVersion)")
                    if let issue = snapshot.issue(for: .capabilities) { SetupIssueView(session: session, issue: issue) }
                    ForEach(snapshot.capabilities.degraded, id: \.self) { reason in Text(reason).foregroundStyle(.orange) }
                    if let issue = snapshot.issue(for: .pairing) { SetupIssueView(session: session, issue: issue) }
                    Button("Manage connections…") { showSettings(.connections) }
                }.padding(.top, 12)
            }
            .foregroundStyle(.secondary)
            if let notice = session.setupNotice {
                Text(notice).font(.callout).foregroundStyle(.secondary).accessibilityLabel(notice)
            }
        }
        .padding(.vertical, 28)
    }

}

private struct SetupIssueView: View {
    let session: DesktopSession
    let issue: JetSetupIssue

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            VStack(alignment: .leading, spacing: 2) {
                Text(issue.error.message)
                    .foregroundStyle(.orange)
                Text(issue.error.code)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("\(issue.section.title) unavailable. \(issue.error.message) \(issue.error.code)")
            JetSettingsRecoveryButton(session: session, error: issue.error)
        }
    }
}

private struct SetupStep<Content: View>: View {
    let number: String
    let title: String
    let detail: String
    @ViewBuilder let content: Content
    var body: some View {
        HStack(alignment: .top, spacing: 18) {
            Text(number).font(.caption.monospacedDigit()).foregroundStyle(.tertiary)
                .frame(width: 24).padding(.top, 4).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 14) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(title).font(.title3.weight(.semibold))
                    Text(detail).foregroundStyle(.secondary)
                }
                content
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

#if DEBUG
#Preview {
    DesktopPreviewScenes.view("project-page")
        .frame(width: 1280, height: 800)
}
#endif
