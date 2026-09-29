import SwiftUI

/// Allow Another Mac to Connect…: shows a one-time code, then asks the person to
/// compare the numbers both Macs show. The other Mac enters the code in
/// Connect Another Computer….
struct AllowConnectionSheet: View {
    @Bindable var model: ComputerPairingModel
    let planeRegistryID: UUID
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Allow Another Mac to Connect")
                .font(.system(size: JetDesign.TextSize.title, weight: .semibold))
            content
            if let issue = model.issue {
                SettingsIssueRow(error: issue, sentence: issueSentence(issue))
            }
            Spacer(minLength: 0)
            buttons
        }
        .padding(20)
        .frame(minWidth: 480, idealWidth: 520, minHeight: 320, idealHeight: 460)
        .task {
            // Previews seed a phase; only a fresh sheet asks for a code.
            guard model.allowPhase == .idle else { return }
            await model.beginAllowing(on: planeRegistryID)
        }
        .onDisappear {
            Task { await model.endAllowing() }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch model.allowPhase {
        case .idle, .gettingCode:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Getting a code…")
            }
            .foregroundStyle(.secondary)
        case let .showingCode(code):
            VStack(alignment: .leading, spacing: 12) {
                Text("On the other Mac, open Jet Settings › Computers, choose Connect Another Computer…, and enter this code:")
                    .fixedSize(horizontal: false, vertical: true)
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    PairingNumbers(text: code, label: String(localized: "Code"))
                    Button("Copy") { DesktopSession.copyToPasteboard(code) }
                }
                if let expiresAt = model.offerExpiresAt {
                    TimelineView(.periodic(from: .now, by: 1)) { context in
                        Text("The code works once and expires in \(Self.countdown(until: expiresAt, now: context.date)).")
                            .settingsCaption()
                            .monospacedDigit()
                    }
                }
                waiting(String(localized: "Waiting for the other Mac…"))
            }
        case let .comparing(numbers):
            VStack(alignment: .leading, spacing: 12) {
                Text("Check that both computers show:")
                PairingNumbers(text: numbers, label: String(localized: "Numbers"))
                Text("If they're different, choose Don't Match. Someone else may be trying to use the code.")
                    .settingsCaption()
                    .fixedSize(horizontal: false, vertical: true)
            }
        case .waitingForOtherMac:
            waiting(String(localized: "Waiting for the other Mac to finish…"))
        case .connected:
            VStack(alignment: .leading, spacing: 6) {
                SettingsStatusLabel(
                    text: String(localized: "Another Mac can now use This Mac."),
                    systemImage: "checkmark.circle.fill",
                    tint: .green
                )
                Text("You can revoke its access here at any time.")
                    .foregroundStyle(.secondary)
            }
        case let .ended(end):
            SettingsStatusLabel(text: endText(end), systemImage: "xmark.circle")
                .fixedSize(horizontal: false, vertical: true)
        case .failed:
            EmptyView()
        }
    }

    @ViewBuilder
    private var buttons: some View {
        HStack {
            switch model.allowPhase {
            case .comparing:
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                // Neither answer is the default: the person has to choose.
                Button("Don't Match") { Task { await model.codesDontMatch() } }
                    .disabled(model.isWorking)
                Button("Codes Match") { Task { await model.confirmCodesMatch() } }
                    .disabled(model.isWorking)
                    .accessibilityIdentifier("codes-match")
            case .connected:
                Spacer()
                Button("Done") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            case .ended, .failed:
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Get New Code") {
                    Task { await model.beginAllowing(on: planeRegistryID) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(model.isWorking)
            case .idle, .gettingCode, .showingCode, .waitingForOtherMac:
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
        }
    }

    private func waiting(_ text: String) -> some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(text)
        }
        .foregroundStyle(.secondary)
    }

    private func endText(_ end: ComputerPairingModel.AllowEnd) -> String {
        switch end {
        case .expired:
            String(localized: "The code expired.")
        case .tooManyAttempts:
            String(localized: "The code was entered wrong too many times.")
        case .stopped:
            String(localized: "Connection stopped. Nothing was connected. If the numbers didn't match, someone else may have tried to use the code.")
        }
    }

    private func issueSentence(_ issue: JetPresentationError) -> String {
        switch issue.category {
        case .outcomeUnknown: String(localized: "Jet couldn't confirm this change.")
        case .offline: String(localized: "Can't reach This Mac's background service.")
        default: String(localized: "Jet couldn't get a code.")
        }
    }

    /// "1:52".
    static func countdown(until date: Date, now: Date) -> String {
        let seconds = max(0, Int(date.timeIntervalSince(now).rounded(.up)))
        return Duration.seconds(seconds).formatted(.time(pattern: .minuteSecond))
    }
}

/// A code or the numbers to compare, large and monospaced. VoiceOver reads it
/// one digit at a time.
struct PairingNumbers: View {
    let text: String
    let label: String

    var body: some View {
        Text(text)
            .font(.system(size: JetDesign.TextSize.invitation, weight: .semibold, design: .monospaced))
            .textSelection(.enabled)
            .accessibilityLabel(Text(label))
            .accessibilityValue(Text(text).speechSpellsOutCharacters())
    }
}
