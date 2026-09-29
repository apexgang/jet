import SwiftUI

/// The old name of the Computers pane, kept while the main window still shows it.
/// WP11: the lead deletes this typealias in wave 3, once WP5 removes the `.planes`
/// destination from the detail switch.
typealias PlaneManagementView = ComputersSettingsPane

/// Settings › Computers, the only home for computers: This Mac and the Jet apps
/// connected to it, the other computers this Mac uses, and connecting another.
struct ComputersSettingsPane: View {
    @Bindable var session: DesktopSession
    @State private var pairing: ComputerPairingModel
    @State private var showsAllowSheet = false
    @State private var showsConnectSheet = false
    @State private var pendingRevocation: PendingClient?
    @State private var pendingRemoval: JetPlanePresentation?

    init(session: DesktopSession, pairing: ComputerPairingModel) {
        self.session = session
        _pairing = State(initialValue: pairing)
    }

    /// For the main window's legacy destination, which has no Settings models.
    init(session: DesktopSession) {
        self.init(
            session: session,
            pairing: ComputerPairingModel(makeAccess: { try await session.pairingAccess(for: $0) })
        )
    }

    var body: some View {
        SettingsPaneForm {
            if let local = session.localPlane {
                Section("This Mac") {
                    ComputerStatusRow(session: session, plane: local) {
                        Button("Allow Another Mac to Connect…") { showsAllowSheet = true }
                            .accessibilityIdentifier("allow-connection")
                            .disabled(!session.isPlaneConnected(local.id))
                    }
                    .accessibilityIdentifier("plane-\(local.id.uuidString.lowercased())")
                    ConnectionDetails(plane: local, connection: session.computerConnection(local.id))
                }

                Section("Connected Jet Apps") {
                    let clients = pairing.clients(of: local.id, fallback: local.snapshot?.pairing)
                    if clients.isEmpty {
                        Text("No other Jet apps can use This Mac.")
                            .foregroundStyle(.secondary)
                    } else {
                        ForEach(clients) { client in
                            ConnectedAppRow(
                                client: client,
                                isWorking: pairing.clientOperations.contains(client.id),
                                revoke: { pendingRevocation = PendingClient(client: client, planeRegistryID: local.id) },
                                setAccess: { enabled in
                                    Task { await pairing.setAccess(client, enabled: enabled, on: local.id) }
                                }
                            )
                        }
                    }
                }
            }

            ForEach(remotePlanes) { plane in
                Section(plane.name) {
                    ComputerStatusRow(session: session, plane: plane) {
                        Button("Reconnect") {
                            Task { await session.reconnectComputer(plane.id) }
                        }
                        Button("Remove from This Mac…", role: .destructive) { pendingRemoval = plane }
                    }
                    .accessibilityIdentifier("plane-\(plane.id.uuidString.lowercased())")
                    if let endpoint = plane.endpoint {
                        LabeledContent("SSH address") {
                            Text(endpoint)
                                .font(.system(size: JetDesign.TextSize.control, design: .monospaced))
                                .textSelection(.enabled)
                        }
                    }
                    ConnectionDetails(plane: plane, connection: session.computerConnection(plane.id)) {
                        let clients = pairing.clients(of: plane.id, fallback: plane.snapshot?.pairing)
                        Text("Connected Jet Apps")
                            .font(.system(size: JetDesign.TextSize.control, weight: .semibold))
                            .padding(.top, 4)
                        if clients.isEmpty {
                            Text("No other Jet apps can use \(plane.name).")
                                .foregroundStyle(.secondary)
                        }
                        ForEach(clients) { client in
                            ConnectedAppRow(
                                client: client,
                                isWorking: pairing.clientOperations.contains(client.id),
                                revoke: { pendingRevocation = PendingClient(client: client, planeRegistryID: plane.id) },
                                setAccess: { enabled in
                                    Task { await pairing.setAccess(client, enabled: enabled, on: plane.id) }
                                }
                            )
                        }
                    }
                }
            }

            Section {
                HStack(alignment: .firstTextBaseline) {
                    Text("Use a Mac you can reach with SSH. Its tasks appear next to This Mac's.")
                        .settingsCaption()
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 12)
                    Button("Connect Another Computer…") { showsConnectSheet = true }
                        .accessibilityIdentifier("connect-computer")
                }
            }

            SettingsNoticeSection(notice: pairing.notice, identifier: "computers-notice")
        }
        .task(id: connectedKey) {
            guard !session.isPreviewSession else { return }
            await pairing.refresh(session.planes.filter { session.isPlaneConnected($0.id) }.map(\.id))
        }
        .sheet(isPresented: $showsAllowSheet) {
            if let local = session.localPlane {
                AllowConnectionSheet(model: pairing, planeRegistryID: local.id)
            }
        }
        .sheet(isPresented: $showsConnectSheet) {
            ConnectComputerSheet(session: session)
        }
        .confirmationDialog(
            "Revoke this Jet app's access?",
            isPresented: Binding(
                get: { pendingRevocation != nil },
                set: { if !$0 { pendingRevocation = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Revoke", role: .destructive) {
                guard let pending = pendingRevocation else { return }
                pendingRevocation = nil
                Task { await pairing.revoke(pending.client, on: pending.planeRegistryID) }
            }
            Button("Cancel", role: .cancel) { pendingRevocation = nil }
        } message: {
            Text("It's disconnected now and needs a new code to connect again. Tasks already running keep running.")
        }
        .confirmationDialog(
            removalTitle,
            isPresented: Binding(
                get: { pendingRemoval != nil },
                set: { if !$0 { pendingRemoval = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Remove", role: .destructive) {
                guard let plane = pendingRemoval else { return }
                pendingRemoval = nil
                Task {
                    await session.forgetRemotePlane(plane.id)
                    pairing.summaries.removeValue(forKey: plane.id)
                    pairing.notice = String(localized: "Removed “\(plane.name)” from This Mac.")
                }
            }
            Button("Cancel", role: .cancel) { pendingRemoval = nil }
        } message: {
            if let plane = pendingRemoval {
                Text("Its tasks stay on \(plane.name) but won't appear here until you connect again.")
            }
        }
    }

    private var remotePlanes: [JetPlanePresentation] {
        session.planes.filter { !$0.isLocal }
    }

    private var connectedKey: String {
        session.planes
            .map { "\($0.id.uuidString):\(session.isPlaneConnected($0.id))" }
            .joined(separator: ",")
    }

    private var removalTitle: String {
        guard let plane = pendingRemoval else { return String(localized: "Remove this computer?") }
        return String(localized: "Remove “\(plane.name)” from This Mac?")
    }
}

private struct PendingClient {
    let client: JetPairedClientSummary
    let planeRegistryID: UUID
}

/// A computer's connection status with its actions. The section header names it.
private struct ComputerStatusRow<Actions: View>: View {
    let session: DesktopSession
    let plane: JetPlanePresentation
    @ViewBuilder var actions: Actions

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            ComputerConnectionLabel(
                connection: session.computerConnection(plane.id),
                failure: plane.failure,
                isLocal: plane.isLocal
            )
            Spacer(minLength: 8)
            actions
        }
        .accessibilityElement(children: .contain)
    }
}

/// "Connected", "Connecting…", "Reconnecting…", "Starting…", "Offline" or
/// "Can't connect" with Details.
struct ComputerConnectionLabel: View {
    let connection: JetConnectionState
    let failure: JetPresentationError?
    let isLocal: Bool

    var body: some View {
        switch connection {
        case .connected:
            SettingsStatusLabel(text: String(localized: "Connected"), systemImage: "checkmark.circle.fill", tint: .green)
        case .connecting:
            progress(isLocal ? String(localized: "Starting…") : String(localized: "Connecting…"))
        case .reconnecting:
            progress(String(localized: "Reconnecting…"))
        case .disconnected:
            SettingsStatusLabel(text: String(localized: "Offline"), systemImage: "wifi.slash")
                .foregroundStyle(.secondary)
        case let .failed(error):
            VStack(alignment: .leading, spacing: 2) {
                SettingsStatusLabel(text: String(localized: "Can't connect"), systemImage: "exclamationmark.triangle.fill", tint: .orange)
                DetailsDisclosure(error: failure ?? error)
            }
        }
    }

    private func progress(_ text: String) -> some View {
        HStack(spacing: 6) {
            ProgressView().controlSize(.small)
            Text(text).foregroundStyle(.secondary)
        }
    }
}

/// A small Details disclosure with a failure's message and code.
private struct DetailsDisclosure: View {
    let error: JetPresentationError
    @State private var isExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Button(isExpanded ? String(localized: "Hide Details") : String(localized: "Details")) {
                isExpanded.toggle()
            }
            .buttonStyle(.plain)
            .foregroundStyle(JetDesign.accentText)
            if isExpanded {
                Text(error.message)
                Text(error.code)
                    .font(.system(size: JetDesign.TextSize.metadata, design: .monospaced))
            }
        }
        .font(.system(size: JetDesign.TextSize.metadata))
        .textSelection(.enabled)
    }
}

/// Connection Details: identities and versions, where Jet's own terms may appear.
private struct ConnectionDetails<Extra: View>: View {
    let plane: JetPlanePresentation
    let connection: JetConnectionState
    @ViewBuilder var extra: Extra

    var body: some View {
        DisclosureGroup("Connection Details") {
            VStack(alignment: .leading, spacing: 6) {
                Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 4) {
                    GridRow {
                        Text("Plane").foregroundStyle(.secondary)
                        Text(plane.planeID?.uuidString.lowercased() ?? String(localized: "Not seen yet"))
                            .font(.system(size: JetDesign.TextSize.metadata, design: .monospaced))
                            .textSelection(.enabled)
                    }
                    if let capabilities = plane.snapshot?.capabilities {
                        GridRow {
                            Text("Jet service").foregroundStyle(.secondary)
                            Text(capabilities.coreVersion)
                        }
                        GridRow {
                            Text("Platform").foregroundStyle(.secondary)
                            Text(capabilities.platform)
                        }
                        if !capabilities.harnesses.isEmpty {
                            GridRow {
                                Text("Harnesses").foregroundStyle(.secondary)
                                Text(capabilities.harnesses.joined(separator: ", "))
                            }
                        }
                    }
                    if case let .connected(negotiation) = connection {
                        GridRow {
                            Text("Protocol").foregroundStyle(.secondary)
                            Text("\(negotiation.protocolVersion).\(negotiation.minorVersion)")
                        }
                    }
                }
                if let failure = plane.failure {
                    Text("\(failure.message) (\(failure.code))")
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
                extra
            }
            .font(.system(size: JetDesign.TextSize.control))
            .padding(.top, 4)
        }
    }
}

extension ConnectionDetails where Extra == EmptyView {
    init(plane: JetPlanePresentation, connection: JetConnectionState) {
        self.init(plane: plane, connection: connection) { EmptyView() }
    }
}

/// One Jet app that can use a computer: "Jet app", "Paired Aug 12 · Access off".
private struct ConnectedAppRow: View {
    let client: JetPairedClientSummary
    let isWorking: Bool
    let revoke: () -> Void
    let setAccess: (Bool) -> Void

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            Image(systemName: "macwindow")
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text("Jet app")
                Text(subtitle)
                    .settingsCaption()
            }
            Spacer(minLength: 8)
            if isWorking {
                ProgressView().controlSize(.small)
            }
            Button("Revoke…", action: revoke)
                .disabled(isWorking)
        }
        .contentShape(Rectangle())
        .contextMenu {
            if client.access == .enabled {
                Button("Turn Off Access") { setAccess(false) }
            } else {
                Button("Turn On Access") { setAccess(true) }
            }
        }
    }

    private var subtitle: String {
        let date = JetCopy.shortDate(Date(timeIntervalSince1970: TimeInterval(client.pairedAtUnixMilliseconds) / 1_000))
        return client.access == .enabled
            ? String(localized: "Paired \(date)")
            : String(localized: "Paired \(date) · Access off")
    }
}
