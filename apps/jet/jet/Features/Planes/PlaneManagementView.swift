import SwiftUI

struct PlaneManagementView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                HStack(alignment: .firstTextBaseline) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Planes")
                            .font(.title2.weight(.semibold))
                        Text("Each Plane keeps its own state, cursor, capabilities, and connection health.")
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Refresh") {
                        Task { await session.refreshPlanes() }
                    }
                    .disabled(session.remotePairingOperation != nil)
                }

                ForEach(session.planes) { plane in
                    PlaneCard(session: session, plane: plane)
                }

                if !session.remoteProfiles.isEmpty {
                    Divider()
                }
                AddRemotePlaneCard(session: session)

                if let notice = session.remotePairingNotice {
                    Text(notice)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .accessibilityIdentifier("remote-pairing-notice")
                }
            }
            .frame(maxWidth: 760, alignment: .leading)
            .padding(24)
            .frame(maxWidth: .infinity)
        }
        .navigationTitle("Planes")
        .confirmationDialog(
            "Revoke this paired client?",
            isPresented: Binding(
                get: { session.pairedClientPendingRevocation != nil },
                set: { if !$0 { session.cancelPairedClientRevocation() } }
            ),
            titleVisibility: .visible
        ) {
            Button("Revoke Client", role: .destructive) {
                Task { await session.confirmPairedClientRevocation() }
            }
            Button("Cancel", role: .cancel, action: session.cancelPairedClientRevocation)
        } message: {
            Text("Revocation deletes this client's public key, closes its connections, and requires pairing again. Visa Runs already hosted by this Plane keep running.")
        }
    }
}

private struct PlaneCard: View {
    @Bindable var session: DesktopSession
    let plane: JetPlanePresentation

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 14) {
                HStack(alignment: .top, spacing: 12) {
                    Image(systemName: plane.isLocal ? "macbook" : "desktopcomputer")
                        .font(.title3)
                        .foregroundStyle(connectionColor)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(plane.name)
                            .font(.headline)
                        if let endpoint = plane.endpoint {
                            Text(endpoint)
                                .font(.caption.monospaced())
                                .foregroundStyle(.secondary)
                                .textSelection(.enabled)
                        }
                    }
                    Spacer()
                    Label(connectionLabel, systemImage: connectionSymbol)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(connectionColor)
                }

                Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 7) {
                    GridRow {
                        Text("Plane identity").foregroundStyle(.secondary)
                        Text(plane.planeID?.uuidString.lowercased() ?? "Not observed")
                            .font(.caption.monospaced())
                            .textSelection(.enabled)
                    }
                    if let snapshot = plane.snapshot {
                        GridRow {
                            Text("Core").foregroundStyle(.secondary)
                            Text(snapshot.capabilities.coreVersion)
                        }
                        GridRow {
                            Text("Platform").foregroundStyle(.secondary)
                            Text(snapshot.capabilities.platform)
                        }
                        GridRow {
                            Text("Cursor").foregroundStyle(.secondary)
                            Text((plane.conversationCursor ?? snapshot.status.cursor ?? 0).formatted())
                                .monospacedDigit()
                        }
                    }
                }
                .font(.caption)

                if let failure = plane.failure {
                    Label {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(failure.message)
                            Text(failure.code)
                                .font(.caption2.monospaced())
                        }
                    } icon: {
                        Image(systemName: "exclamationmark.triangle")
                    }
                    .foregroundStyle(.orange)
                    .accessibilityElement(children: .combine)
                }

                capabilitySection
                pairingSection

                if !plane.isLocal {
                    Divider()
                    HStack {
                        Text("Remote traffic uses the system SSH client and `jetd connect --stdio`. Jet never opens a plaintext fallback.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button("Forget on This Mac", role: .destructive) {
                            Task { await session.forgetRemotePlane(plane.id) }
                        }
                    }
                }
            }
            .padding(4)
        }
        .accessibilityIdentifier("plane-\(plane.id.uuidString.lowercased())")
    }

    @ViewBuilder
    private var capabilitySection: some View {
        if let capabilities = plane.snapshot?.capabilities {
            Divider()
            VStack(alignment: .leading, spacing: 8) {
                Text("Capabilities")
                    .font(.subheadline.weight(.semibold))
                if !capabilities.crafts.isEmpty {
                    LabeledContent("Crafts", value: capabilities.crafts.map(\.id).joined(separator: ", "))
                }
                if !capabilities.harnesses.isEmpty {
                    LabeledContent("Harnesses", value: capabilities.harnesses.joined(separator: ", "))
                }
                if plane.capabilityLimitations.isEmpty {
                    Label("No reported capability limits", systemImage: "checkmark.circle")
                        .foregroundStyle(.green)
                } else {
                    ForEach(plane.capabilityLimitations, id: \.self) { limitation in
                        Label(limitation, systemImage: "minus.circle")
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .font(.caption)
        }
    }

    @ViewBuilder
    private var pairingSection: some View {
        if let pairing = plane.snapshot?.pairing {
            Divider()
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Pairing")
                            .font(.subheadline.weight(.semibold))
                        Text(pairing.gate == "open" ? "Accepting a new client" : "Closed to new clients")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button(pairing.gate == "open" ? "Close Gate" : "Open Gate") {
                        Task {
                            await session.setPairingGate(
                                on: plane.id,
                                open: pairing.gate != "open"
                            )
                        }
                    }
                    .disabled(session.remotePairingOperation != nil)
                    Button("Show Pairing Code") {
                        Task { await session.openManualPairing(on: plane.id) }
                    }
                    .disabled(session.remotePairingOperation != nil)
                }

                if session.openedPairingPlaneID == plane.id,
                   let opened = session.openedPairing
                {
                    switch opened.disclosure {
                    case let .manualCode(code):
                        LabeledContent("One-time code") {
                            Text(code)
                                .font(.title3.monospaced().weight(.semibold))
                                .textSelection(.enabled)
                        }
                    case let .qrPayload(payload):
                        LabeledContent("Pairing payload") {
                            Text(payload).font(.caption.monospaced()).textSelection(.enabled)
                        }
                    case .alreadyDisclosed:
                        Text("This offer's secret was already shown. Open a new offer to disclose another code.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }

                if let pending = pairing.pending,
                   let authenticationString = pending.progress.authenticationString
                {
                    HStack(alignment: .center) {
                        VStack(alignment: .leading, spacing: 3) {
                            Text("Authentication string")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text(authenticationString)
                                .font(.title3.monospaced().weight(.semibold))
                                .textSelection(.enabled)
                            if let clientID = pending.progress.clientID {
                                Text("Client \(clientID.uuidString.lowercased())")
                                    .font(.caption2.monospaced())
                                    .foregroundStyle(.secondary)
                                    .textSelection(.enabled)
                            }
                        }
                        Spacer()
                        if case .awaitingConfirmation = pending.progress {
                            Button("Confirm Match") {
                                Task { await session.confirmPendingPairing(on: plane.id) }
                            }
                            .buttonStyle(.borderedProminent)
                            .disabled(session.remotePairingOperation != nil)
                        } else {
                            Text("Confirmed on this Plane")
                                .font(.caption)
                                .foregroundStyle(.green)
                        }
                    }
                }

                if pairing.clients.isEmpty {
                    Text("No paired clients")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(pairing.clients) { client in
                        HStack(alignment: .center, spacing: 10) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(client.id.uuidString.lowercased())
                                    .font(.caption.monospaced())
                                    .textSelection(.enabled)
                                Text("\(client.access.label) · \(client.pairingProtocol)")
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button(client.access == .enabled ? "Disable" : "Enable") {
                                Task {
                                    await session.setPairedClientAccess(
                                        client,
                                        on: plane.id,
                                        enabled: client.access != .enabled
                                    )
                                }
                            }
                            .disabled(session.remotePairingOperation != nil)
                            Button("Revoke", role: .destructive) {
                                session.requestPairedClientRevocation(client, on: plane.id)
                            }
                            .disabled(session.remotePairingOperation != nil)
                        }
                    }
                }
            }
        }
    }

    private var connectionLabel: String {
        switch plane.connection {
        case .disconnected: "Offline"
        case .connecting: "Connecting"
        case .connected: "Connected"
        case .reconnecting: "Reconnecting"
        case .failed: "Unavailable"
        }
    }

    private var connectionSymbol: String {
        switch plane.connection {
        case .connected: "checkmark.circle.fill"
        case .connecting, .reconnecting: "arrow.clockwise.circle"
        case .disconnected: "wifi.slash"
        case .failed: "exclamationmark.triangle.fill"
        }
    }

    private var connectionColor: Color {
        switch plane.connection {
        case .connected: .green
        case .connecting, .reconnecting: .orange
        case .disconnected: .secondary
        case .failed: .red
        }
    }
}

private struct AddRemotePlaneCard: View {
    @Bindable var session: DesktopSession

    var body: some View {
        GroupBox("Pair a remote Plane") {
            VStack(alignment: .leading, spacing: 12) {
                Text("Resolve SSH host trust first. Jet honors your SSH configuration, agent, jump hosts, and known_hosts, then performs Jet pairing inside that encrypted connection.")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                TextField("Name, such as Studio Mac", text: $session.remotePlaneName)
                    .textFieldStyle(.roundedBorder)
                TextField(
                    "SSH host alias or user@host",
                    text: Binding(
                        get: { session.remoteSSHEndpoint },
                        set: {
                            session.remoteSSHEndpoint = $0
                            session.remotePairingInputDidChange()
                        }
                    )
                )
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("remote-ssh-endpoint")

                if let claim = session.remotePairingClaim {
                    LabeledContent("Authentication string") {
                        Text(claim.authenticationString)
                            .font(.title3.monospaced().weight(.semibold))
                            .textSelection(.enabled)
                    }
                    Text("Confirm the same string on the target Plane before completing pairing here.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    HStack {
                        Button("Start Over") {
                            session.restartRemotePlanePairing()
                        }
                        Spacer()
                        Button("Complete Pairing") {
                            Task { await session.completeRemotePlanePairing() }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(session.remotePairingOperation != nil)
                    }
                } else {
                    TextField(
                        "One-time pairing code",
                        text: Binding(
                            get: { session.remotePairingSecret },
                            set: {
                                session.remotePairingSecret = $0
                                session.remotePairingInputDidChange()
                            }
                        )
                    )
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("remote-pairing-code")
                    HStack {
                        Spacer()
                        Button("Compare Authentication String") {
                            Task { await session.claimRemotePlane() }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(
                            session.remotePairingOperation != nil
                                || session.remoteSSHEndpoint.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                || session.remotePairingSecret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        )
                    }
                }
            }
            .padding(4)
        }
    }
}
