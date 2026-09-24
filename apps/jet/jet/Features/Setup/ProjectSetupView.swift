import SwiftUI
import UniformTypeIdentifiers

struct ProjectSetupView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                Divider()

                switch session.setupState {
                case .idle, .loading:
                    ProgressView("Connecting to the local Plane")
                        .frame(maxWidth: .infinity, minHeight: 280)
                case let .failed(error):
                    ContentUnavailableView {
                        Label("Local Plane unavailable", systemImage: "network.slash")
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
        .sheet(item: $session.removalPreview) { preview in
            ProjectRemovalSheet(session: session, preview: preview)
        }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 20) {
            VStack(alignment: .leading, spacing: 5) {
                Text("Set up this workspace")
                    .font(.title2.weight(.semibold))
                Text("Connect the local Plane, choose a Project, and use a Harness login already available here.")
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
        let capabilitiesIssue = snapshot.issue(for: .capabilities)
        let needsAttention = capabilitiesIssue != nil || !snapshot.capabilities.degraded.isEmpty

        SetupRow(
            symbol: "circle.fill",
            symbolColor: session.planeIsConnected ? (needsAttention ? .orange : .green) : .orange,
            title: "Local Plane"
        ) {
            VStack(alignment: .leading, spacing: 4) {
                Text("\(snapshot.capabilities.platform) · Core \(snapshot.capabilities.coreVersion)")
                if !snapshot.capabilities.degraded.isEmpty {
                    Text(snapshot.capabilities.degraded.joined(separator: " · "))
                        .foregroundStyle(.orange)
                }
                if let capabilitiesIssue {
                    SetupIssueView(session: session, issue: capabilitiesIssue)
                }
            }
        } trailing: {
            let label = session.planeIsConnected
                ? (needsAttention ? "Needs attention" : "Connected")
                : session.planeConnectionLabel
            Text(label)
                .foregroundStyle(session.planeIsConnected && !needsAttention ? Color.green : Color.orange)
        }

        Divider()

        SetupRow(symbol: "folder", symbolColor: .accentColor, title: "Projects") {
            VStack(alignment: .leading, spacing: 14) {
                Text("Jet registers the Git working tree only after you review its resolved path.")
                    .foregroundStyle(.secondary)

                if let issue = snapshot.issue(for: .projects) {
                    SetupIssueView(session: session, issue: issue)
                } else if snapshot.projects.projects.isEmpty {
                    Text("No Projects yet. Add the root folder of a Git working tree.")
                        .foregroundStyle(.secondary)
                } else {
                    VStack(spacing: 2) {
                        ForEach(snapshot.projects.projects) { project in
                            HStack(spacing: 12) {
                                Button {
                                    session.selectProject(project.id)
                                } label: {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(project.name)
                                            .foregroundStyle(.primary)
                                        Text(project.root)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                            .lineLimit(1)
                                            .truncationMode(.middle)
                                    }
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)

                                if session.selectedProjectID == project.id {
                                    Text("Selected")
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }

                                Button("Remove", role: .destructive) {
                                    Task { await session.prepareProjectRemoval(project.id) }
                                }
                                .buttonStyle(.borderless)
                                .disabled(session.setupOperation != nil)
                            }
                            .padding(.horizontal, 10)
                            .padding(.vertical, 8)
                            .background(
                                session.selectedProjectID == project.id
                                    ? Color.accentColor.opacity(0.09)
                                    : Color.clear,
                                in: RoundedRectangle(cornerRadius: 9)
                            )
                        }
                    }
                }

                Button {
                    session.isProjectImporterPresented = true
                } label: {
                    Label("Add Project…", systemImage: "plus")
                }
                .disabled(session.setupOperation != nil)

                if let preview = session.projectPreview {
                    ProjectPreviewView(session: session, preview: preview)
                }
            }
        } trailing: {
            Text(
                snapshot.issue(for: .projects) == nil
                    ? "\(snapshot.projects.projects.count) registered"
                    : "Unavailable"
            )
                .foregroundStyle(.secondary)
        }

        Divider()

        SetupRow(symbol: "person.crop.circle", symbolColor: .secondary, title: "Harness access") {
            VStack(alignment: .leading, spacing: 12) {
                Text("Jet records a non-secret binding. Sign-in stays with the Harness when work starts.")
                    .foregroundStyle(.secondary)

                if let issue = snapshot.issue(for: .accounts) {
                    SetupIssueView(session: session, issue: issue)
                } else {
                    ForEach(snapshot.accounts.bindings) { binding in
                        LabeledContent(binding.label) {
                            Text(binding.stateLabel)
                                .foregroundStyle(.secondary)
                        }
                    }
                }

                if snapshot.issue(for: .capabilities) != nil {
                    Text("Harness choices will return when Plane capabilities are available.")
                        .foregroundStyle(.secondary)
                } else if snapshot.capabilities.authProviders.isEmpty {
                    Text("Install a supported Craft before connecting a Harness.")
                        .foregroundStyle(.secondary)
                } else {
                    HStack(spacing: 8) {
                        ForEach(snapshot.capabilities.authProviders) { provider in
                            Button("Use \(provider.harness) login") {
                                Task { await session.connectHarness(provider) }
                            }
                            .disabled(session.setupOperation != nil)
                        }
                    }
                }
            }
        } trailing: {
            Text(snapshot.capabilities.credentialStore.label)
                .foregroundStyle(
                    snapshot.capabilities.credentialStore == .available
                        ? Color.secondary
                        : Color.orange
                )
        }

        Divider()

        SetupRow(symbol: "desktopcomputer", symbolColor: .secondary, title: "Remote Plane") {
            if let issue = snapshot.issue(for: .pairing) {
                SetupIssueView(session: session, issue: issue)
            } else if snapshot.pairing.pairedClients > 0 {
                Text("\(snapshot.pairing.pairedClients) paired client\(snapshot.pairing.pairedClients == 1 ? "" : "s")")
                    .foregroundStyle(.secondary)
            } else {
                Text("Pair another computer later from Connections.")
                    .foregroundStyle(.secondary)
            }
        } trailing: {
            if snapshot.issue(for: .pairing) != nil {
                Text("Unavailable")
                    .foregroundStyle(.secondary)
            } else if session.remotePairingSkipped {
                Text("Skipped")
                    .foregroundStyle(.secondary)
            } else {
                Button("Skip for now", action: session.skipRemotePairing)
                    .buttonStyle(.borderless)
            }
        }

        if let notice = session.setupNotice {
            Text(notice)
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.top, 14)
                .padding(.leading, 38)
                .accessibilityLabel(notice)
        }
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

private struct SetupRow<Content: View, Trailing: View>: View {
    let symbol: String
    let symbolColor: Color
    let title: String
    @ViewBuilder let content: Content
    @ViewBuilder let trailing: Trailing

    var body: some View {
        Grid(alignment: .topLeading, horizontalSpacing: 14, verticalSpacing: 6) {
            GridRow {
                Image(systemName: symbol)
                    .foregroundStyle(symbolColor)
                    .frame(width: 24)
                    .accessibilityHidden(true)
                Text(title)
                    .font(.headline)
                trailing
                    .font(.caption)
                    .gridColumnAlignment(.trailing)
            }
            GridRow {
                Color.clear.frame(width: 24, height: 0)
                content
                    .font(.subheadline)
                Color.clear.frame(width: 0, height: 0)
            }
        }
        .padding(.vertical, 18)
    }
}

private struct ProjectPreviewView: View {
    let session: DesktopSession
    let preview: JetProjectPreview

    var body: some View {
        HStack(alignment: .center, spacing: 18) {
            VStack(alignment: .leading, spacing: 3) {
                Text(preview.canRegister ? "Ready to add" : "Choose another folder")
                    .font(.subheadline.weight(.semibold))
                Text(preview.root)
                    .font(.caption)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 10)
            if preview.canRegister {
                Button("Add Project") {
                    Task { await session.registerPreviewedProject() }
                }
                .buttonStyle(.borderedProminent)
                .disabled(session.setupOperation != nil)
            }
        }
        .padding(12)
        .background(Color.accentColor.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
    }

    private var detail: String {
        switch preview.registrability {
        case let .registrable(detail): detail
        case let .unavailable(_, detail): detail
        }
    }
}

private struct ProjectRemovalSheet: View {
    let session: DesktopSession
    let preview: JetProjectRemovalPreview

    @Environment(\.dismiss) private var dismiss
    @State private var typedName = ""
    @State private var permanently = false

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 5) {
                Text("Remove \(preview.name)?")
                    .font(.title2.weight(.semibold))
                Text("This removes Jet's registration and the Project folder.")
                    .foregroundStyle(.secondary)
            }

            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 8) {
                fact("Folder", preview.root)
                fact("On disk", ByteCountFormatter.string(fromByteCount: Int64(preview.diskUseBytes), countStyle: .file))
                fact("Changed files", preview.dirtyFiles.formatted())
                fact("Unpushed commits", preview.unpushedCommits.formatted())
                fact("Workspaces", preview.workspaceCount.formatted())
            }

            if preview.obstacles.isEmpty {
                TextField("Type \(preview.name) to confirm", text: $typedName)
                    .textFieldStyle(.roundedBorder)

                if session.permanentRemovalAllowed {
                    Toggle(isOn: $permanently) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Delete permanently")
                            Text("Trash is unavailable. \(preview.permanentWarning)")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            } else {
                VStack(alignment: .leading, spacing: 7) {
                    Text("This Project cannot be removed yet.")
                        .font(.subheadline.weight(.semibold))
                    ForEach(preview.obstacles, id: \.self) { obstacle in
                        Label(obstacle, systemImage: "exclamationmark.circle")
                            .foregroundStyle(.orange)
                    }
                }
                .padding(12)
                .background(Color.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
            }

            HStack {
                Spacer()
                Button("Cancel") {
                    session.cancelProjectRemoval()
                    dismiss()
                }
                Button(permanently ? "Delete permanently" : "Move to Trash", role: .destructive) {
                    Task {
                        await session.confirmProjectRemoval(
                            typedName: typedName,
                            permanently: permanently
                        )
                        if session.removalPreview == nil { dismiss() }
                    }
                }
                .disabled(
                    typedName != preview.name
                        || !preview.obstacles.isEmpty
                        || session.setupOperation != nil
                )
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(width: 520)
        .interactiveDismissDisabled(session.setupOperation != nil)
    }

    private func fact(_ label: String, _ value: String) -> some View {
        GridRow {
            Text(label)
                .foregroundStyle(.secondary)
            Text(value)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .font(.subheadline)
    }
}

#Preview {
    ProjectSetupView(session: DesktopSession())
        .frame(width: 900, height: 720)
}
