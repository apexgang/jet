import SwiftUI

/// Connect Another Computer…: three steps on this Mac while the other Mac shows
/// a code in Allow Another Mac to Connect…. Pairing never changes the computer
/// New Task uses.
struct ConnectComputerSheet: View {
    enum Step: Int, Equatable {
        case intro = 1
        case details = 2
        case check = 3
        case done = 4
    }

    @Bindable var session: DesktopSession
    var initialStep = Step.intro
    @Environment(\.dismiss) private var dismiss
    @State private var step = Step.intro
    @State private var didStart = false
    @State private var finishFailed = false
    @State private var connectedName = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .firstTextBaseline) {
                Text("Connect Another Computer")
                    .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
                Spacer()
                if step != .done {
                    Text("Step \(step.rawValue) of 3")
                        .settingsCaption()
                }
            }
            switch step {
            case .intro: introStep
            case .details: detailsStep
            case .check: checkStep
            case .done: doneStep
            }
            Spacer(minLength: 0)
            buttons
        }
        .padding(20)
        .frame(minWidth: 480, idealWidth: 520, minHeight: 320, idealHeight: 460)
        .onAppear(perform: begin)
    }

    // MARK: Steps

    private var introStep: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("On the other Mac, open Jet Settings › Computers and choose Allow Another Mac to Connect…. Keep that window open.")
                .fixedSize(horizontal: false, vertical: true)
            Text("Jet connects over SSH, so you need to be able to sign in to that Mac with SSH from here.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var detailsStep: some View {
        VStack(alignment: .leading, spacing: 12) {
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 10) {
                GridRow {
                    Text("Name").gridColumnAlignment(.trailing)
                    TextField("Name", text: $session.remotePlaneName, prompt: Text("Studio Mac"))
                        .labelsHidden()
                        .textFieldStyle(.roundedBorder)
                }
                GridRow {
                    Text("SSH address")
                    VStack(alignment: .leading, spacing: 4) {
                        TextField("SSH address", text: endpointBinding, prompt: Text("user@host"))
                            .labelsHidden()
                            .textFieldStyle(.roundedBorder)
                            .accessibilityIdentifier("remote-ssh-endpoint")
                        if endpointIsInvalid {
                            SettingValidationMessage(text: String(localized: "Use an SSH host name or user@host without spaces."))
                        }
                    }
                }
                GridRow {
                    Text("Code")
                    TextField("Code", text: codeBinding, prompt: Text("1234-5678"))
                        .labelsHidden()
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: JetDesign.TextSize.navigation, design: .monospaced))
                        .frame(width: 140)
                        .accessibilityIdentifier("remote-pairing-code")
                }
            }
            if session.remotePairingOperation != nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Connecting…")
                }
                .foregroundStyle(.secondary)
            } else if let notice = session.remotePairingNotice, session.remotePairingClaim == nil {
                failure(
                    String(localized: "Jet couldn't connect to “\(displayName)”. Check the SSH address and the code. Codes expire after 2 minutes."),
                    details: notice
                )
                .accessibilityIdentifier("remote-pairing-notice")
            }
        }
    }

    private var checkStep: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Check that both computers show:")
            if let claim = session.remotePairingClaim {
                PairingNumbers(text: claim.authenticationString, label: String(localized: "Numbers"))
            }
            Text("On \(displayName), choose Codes Match. Then choose Finish.")
                .fixedSize(horizontal: false, vertical: true)
            if session.remotePairingOperation != nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Finishing…")
                }
                .foregroundStyle(.secondary)
            } else if finishFailed {
                failure(
                    String(localized: "Jet couldn't finish connecting. Make sure you chose Codes Match on \(displayName), then try again."),
                    details: session.remotePairingNotice
                )
                .accessibilityIdentifier("remote-pairing-notice")
            }
        }
    }

    private var doneStep: some View {
        VStack(alignment: .leading, spacing: 8) {
            SettingsStatusLabel(
                text: String(localized: "\(connectedName) is connected."),
                systemImage: "checkmark.circle.fill",
                tint: .green
            )
            Text("New tasks still start on This Mac. Choose \(connectedName) in New Task when you want to work there.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func failure(_ sentence: String, details: String?) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            SettingsStatusLabel(text: sentence, systemImage: "exclamationmark.triangle.fill", tint: .orange)
                .fixedSize(horizontal: false, vertical: true)
            if let details {
                DisclosureGroup("Details") {
                    Text(details)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .font(.system(size: JetDesign.TextSize.metadata))
            }
        }
        .font(.system(size: JetDesign.TextSize.control))
    }

    // MARK: Buttons

    private var buttons: some View {
        HStack {
            if step == .details {
                Button("Back") { step = .intro }
                    .disabled(session.remotePairingOperation != nil)
            }
            Spacer()
            switch step {
            case .intro:
                cancelButton
                Button("Continue") { step = .details }
                    .keyboardShortcut(.defaultAction)
            case .details:
                cancelButton
                Button("Connect", action: claim)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canClaim)
            case .check:
                cancelButton
                Button("Finish", action: finish)
                    .keyboardShortcut(.defaultAction)
                    .disabled(session.remotePairingOperation != nil || session.remotePairingClaim == nil)
            case .done:
                Button("Done") {
                    session.restartRemotePlanePairing()
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
    }

    private var cancelButton: some View {
        Button("Cancel", role: .cancel) {
            session.restartRemotePlanePairing()
            dismiss()
        }
        .keyboardShortcut(.cancelAction)
    }

    // MARK: Actions

    private func begin() {
        guard !didStart else { return }
        didStart = true
        step = initialStep
        if initialStep == .intro {
            // Start clean: the session's pairing notice is shared with other actions.
            session.remotePlaneName = ""
            session.remoteSSHEndpoint = ""
            session.restartRemotePlanePairing()
        }
        if initialStep == .done { connectedName = displayName }
    }

    private func claim() {
        guard canClaim else { return }
        Task {
            await session.claimRemotePlane()
            if session.remotePairingClaim != nil {
                finishFailed = false
                step = .check
            }
        }
    }

    private func finish() {
        let name = displayName
        finishFailed = false
        Task {
            await session.completeRemotePlanePairing()
            if session.remotePairingClaim == nil {
                connectedName = name
                step = .done
            } else {
                finishFailed = true
            }
        }
    }

    // MARK: Input

    private var displayName: String {
        let name = session.remotePlaneName.trimmingCharacters(in: .whitespacesAndNewlines)
        if !name.isEmpty { return name }
        let endpoint = session.remoteSSHEndpoint.trimmingCharacters(in: .whitespacesAndNewlines)
        return endpoint.isEmpty ? String(localized: "the other Mac") : endpoint
    }

    private var trimmedEndpoint: String {
        session.remoteSSHEndpoint.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var endpointIsInvalid: Bool {
        !trimmedEndpoint.isEmpty && (try? JetSSHEndpoint.validated(trimmedEndpoint)) == nil
    }

    private var canClaim: Bool {
        session.remotePairingOperation == nil
            && !trimmedEndpoint.isEmpty
            && !endpointIsInvalid
            && PairingCode.isValid(session.remotePairingSecret)
    }

    private var endpointBinding: Binding<String> {
        Binding(
            get: { session.remoteSSHEndpoint },
            set: {
                session.remoteSSHEndpoint = $0
                session.remotePairingInputDidChange()
            }
        )
    }

    private var codeBinding: Binding<String> {
        Binding(
            get: { session.remotePairingSecret },
            set: {
                session.remotePairingSecret = $0
                session.remotePairingInputDidChange()
            }
        )
    }
}
