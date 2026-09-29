import SwiftUI

/// Temporary bridge for Details › Changes until the inspector's footer shows the
/// Keep status line and Activity embeds `GitActivityList` (the lead deletes it in
/// wave 3). The manual Git form is gone: every step goes through Keep Changes.
struct DeliveryWorkView: View {
    @Bindable var session: DesktopSession

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if let ref = session.selectedDeliveryRef {
                KeepChangesStatusLine(session: session, ref: ref)
            }
            GitActivityList(session: session)
        }
        .padding(14)
    }
}

// MARK: - Step copy (design §6.9)

/// The words for each Git step and its states. `branch` and `remote` are Git
/// names; `reply` is the reply number the commit came from.
enum GitStepCopy {
    typealias Step = DeliveryCoordinator.Step

    static func title(_ step: Step, branch: String?, remote: String, updatesPullRequest: Bool = false) -> String {
        switch step {
        case .branch:
            branch.map { String(localized: "Create branch \($0)") } ?? String(localized: "Create a branch")
        case .commit:
            String(localized: "Commit (Jet writes the message)")
        case .push:
            String(localized: "Push to \(remote)")
        case .draftPullRequest:
            updatesPullRequest
                ? String(localized: "Update the draft pull request")
                : String(localized: "Open a draft pull request")
        }
    }

    static func running(_ step: Step, branch: String?, remote: String) -> String {
        switch step {
        case .branch:
            branch.map { String(localized: "Creating branch \($0)…") } ?? String(localized: "Creating the branch…")
        case .commit:
            String(localized: "Committing the changes…")
        case .push:
            String(localized: "Pushing to \(remote)…")
        case .draftPullRequest:
            String(localized: "Opening a draft pull request…")
        }
    }

    static func done(_ step: Step, branch: String?, remote: String, reply: UInt32?) -> String {
        switch step {
        case .branch:
            branch.map { String(localized: "Created branch \($0)") } ?? String(localized: "Created the branch")
        case .commit:
            reply.map { String(localized: "Committed the changes from reply \($0)") }
                ?? String(localized: "Committed the changes")
        case .push:
            branch.map { String(localized: "Pushed \($0) to \(remote)") } ?? String(localized: "Pushed to \(remote)")
        case .draftPullRequest:
            String(localized: "Opened a draft pull request")
        }
    }

    static func failed(_ step: Step, remote: String) -> String {
        switch step {
        case .branch: String(localized: "Couldn't create the branch.")
        case .commit: String(localized: "Couldn't commit the changes.")
        case .push: String(localized: "Couldn't push to \(remote).")
        case .draftPullRequest: String(localized: "Couldn't open the draft pull request.")
        }
    }

    static func couldNotConfirm(_ step: Step) -> String {
        switch step {
        case .branch: String(localized: "Jet couldn't confirm whether the branch was created.")
        case .commit: String(localized: "Jet couldn't confirm whether the commit was saved.")
        case .push: String(localized: "Jet couldn't confirm whether the push happened.")
        case .draftPullRequest: String(localized: "Jet couldn't confirm whether the pull request was opened.")
        }
    }

    /// What to check before marking an unconfirmed step as checked.
    static func followUp(_ step: Step) -> String {
        switch step {
        case .push, .draftPullRequest:
            String(localized: "Check the branch on GitHub, then mark it as checked.")
        case .branch, .commit:
            String(localized: "Check the branch in your project, then mark it as checked.")
        }
    }

    static var acknowledged: String {
        String(localized: "Marked as checked. Jet didn't retry or undo it.")
    }

    static var admissionUncertain: String {
        String(localized: "Jet couldn't confirm it received the request.")
    }

    /// The short state a step row shows while a plan runs.
    static func stateLabel(_ state: DeliveryCoordinator.StepState) -> String {
        switch state {
        case .notStarted: String(localized: "Not started")
        case .submitting, .pending: String(localized: "Running…")
        case .completed: String(localized: "Done")
        case .failed: String(localized: "Failed")
        case .unconfirmed, .admissionUncertain: String(localized: "Couldn't confirm")
        }
    }

    /// The sentence for a recorded delivery, as Git Activity lists it.
    static func sentence(for delivery: JetGitDelivery, history: [JetGitDelivery]) -> String {
        let branch = delivery.branchName ?? DeliveryCoordinator.knownBranch(in: history)
        let remote = delivery.remoteName ?? "origin"
        switch delivery.outcome {
        case .pending: return running(delivery.step, branch: branch, remote: remote)
        case .completed: return done(delivery.step, branch: branch, remote: remote, reply: delivery.checkpoint?.turn)
        case .failed: return failed(delivery.step, remote: remote)
        case .outcomeUnknown: return couldNotConfirm(delivery.step)
        }
    }
}

// MARK: - Failure copy (design §6.9)

/// A plain sentence and its fix for a Git failure code. Unknown codes read "Git
/// refused this step." The code itself stays in Git Activity's Copy Error Code.
struct GitFailureCopy: Equatable {
    enum Fix: Equatable, Sendable {
        case chooseAnotherName
        case tryAgain
        case markAsChecked
        case keepChanges
        case howToSetUp

        var title: String {
            switch self {
            case .chooseAnotherName: String(localized: "Choose Another Name…")
            case .tryAgain: String(localized: "Try Again…")
            case .markAsChecked: String(localized: "Mark as Checked…")
            case .keepChanges: String(localized: "Keep Changes…")
            case .howToSetUp: String(localized: "How to Set Up…")
            }
        }
    }

    let sentence: String
    let fix: Fix?

    static func make(
        code: String,
        step: DeliveryCoordinator.Step,
        computer: String,
        assistant: String? = nil,
        remote: String = "origin"
    ) -> GitFailureCopy {
        if code.hasPrefix("transport.") {
            return GitFailureCopy(sentence: String(localized: "Not connected to \(computer)."), fix: .tryAgain)
        }
        let name = code.hasPrefix("git.") ? String(code.dropFirst(4)) : code
        switch name {
        case "branch_exists":
            return GitFailureCopy(sentence: String(localized: "A branch with this name already exists."), fix: .chooseAnotherName)
        case "invalid_name" where step == .branch:
            return GitFailureCopy(sentence: String(localized: "Git doesn't accept this branch name."), fix: .chooseAnotherName)
        case "invalid_name", "remote_required", "remote_invalid":
            return GitFailureCopy(sentence: String(localized: "Git couldn't use the remote \(remote)."), fix: .keepChanges)
        case "run_active":
            let sentence = assistant.map { String(localized: "\($0) was still working, so Git didn't run.") }
                ?? String(localized: "The assistant was still working, so Git didn't run.")
            return GitFailureCopy(sentence: sentence, fix: .tryAgain)
        case "delivery_unresolved":
            return GitFailureCopy(sentence: String(localized: "An earlier Git step couldn't be confirmed."), fix: .markAsChecked)
        case "content_changed", "repository_changed", "head_changed", "index_changed", "root_changed":
            return GitFailureCopy(sentence: String(localized: "The working copy changed after you reviewed it."), fix: .keepChanges)
        case "index_locked", "operation_in_progress":
            return GitFailureCopy(sentence: String(localized: "Another Git operation is running in the working copy."), fix: .tryAgain)
        case "policy_changed":
            return GitFailureCopy(sentence: String(localized: "The project's Keep settings changed while this step waited."), fix: .tryAgain)
        case "checkpoint_required", "checkpoint_mismatch":
            return GitFailureCopy(sentence: String(localized: "These changes are no longer available to commit."), fix: .keepChanges)
        case "branch_required":
            return GitFailureCopy(sentence: String(localized: "There's no branch to push yet."), fix: .keepChanges)
        case "push_required":
            return GitFailureCopy(sentence: String(localized: "Push the branch before opening a pull request."), fix: .keepChanges)
        case "github_required":
            return GitFailureCopy(sentence: String(localized: "Pull requests work only with GitHub remotes."), fix: nil)
        case "github_credential_unavailable":
            return GitFailureCopy(sentence: String(localized: "Jet needs a GitHub token saved on \(computer)."), fix: .howToSetUp)
        case "github_permission_denied", "github_refused":
            return GitFailureCopy(
                sentence: String(localized: "GitHub refused the request. Check that your token can write pull requests."),
                fix: .howToSetUp
            )
        case "github_unavailable", "github_timeout", "github_invalid", "github_output_limit":
            return GitFailureCopy(sentence: String(localized: "Jet couldn't get a usable answer from GitHub."), fix: .tryAgain)
        case "draft_conflict":
            return GitFailureCopy(
                sentence: String(localized: "The draft pull request was closed, marked ready, or changed on GitHub."),
                fix: nil
            )
        case "timeout":
            return GitFailureCopy(sentence: String(localized: "Git took too long and was stopped."), fix: .tryAgain)
        case "incomplete_or_dirty_baseline":
            return GitFailureCopy(
                sentence: String(localized: "The project had uncommitted changes when this task started, so Jet can't commit safely."),
                fix: nil
            )
        default:
            return GitFailureCopy(sentence: String(localized: "Git refused this step."), fix: .tryAgain)
        }
    }
}

// MARK: - Helpers

/// Only https links to github.com pull requests are opened.
enum GitHubPullRequestLink {
    static func url(_ value: String?) -> URL? {
        guard let value,
              let components = URLComponents(string: value),
              components.scheme?.lowercased() == "https",
              components.host?.lowercased() == "github.com",
              components.user == nil,
              components.password == nil,
              components.port == nil,
              components.path.contains("/pull/")
        else { return nil }
        return components.url
    }
}

/// When a delivery was created: delivery IDs are UUIDv7, whose first 48 bits are
/// Unix milliseconds.
enum DeliveryTime {
    static func createdAt(_ id: UUID) -> Int64? {
        let bytes = id.uuid
        guard bytes.6 >> 4 == 7 else { return nil }
        let parts = [bytes.0, bytes.1, bytes.2, bytes.3, bytes.4, bytes.5]
        return parts.reduce(Int64(0)) { ($0 << 8) | Int64($1) }
    }
}

/// A small bordered Copy button that reads "Copied" for two seconds and tells
/// VoiceOver.
struct DeliveryCopyButton: View {
    let title: String
    let value: String
    var iconOnly = false

    @State private var copyCount = 0
    @State private var showsCopied = false

    var body: some View {
        Button {
            DesktopSession.copyToPasteboard(value)
            showsCopied = true
            copyCount += 1
            AccessibilityNotification.Announcement(String(localized: "Copied")).post()
        } label: {
            if iconOnly {
                Label(showsCopied ? String(localized: "Copied") : title, systemImage: showsCopied ? "checkmark" : "document.on.document")
                    .labelStyle(.iconOnly)
            } else {
                Text(showsCopied ? String(localized: "Copied") : title)
            }
        }
        .help(title)
        .accessibilityLabel(Text(showsCopied ? String(localized: "Copied") : title))
        .task(id: copyCount) {
            guard copyCount > 0 else { return }
            try? await Task.sleep(for: .seconds(2))
            if !Task.isCancelled { showsCopied = false }
        }
    }
}

/// "How to Set Up…": how to save a GitHub token for Jet on a computer. Jet can't
/// check or store the token itself (protocol gap 7).
struct GitHubSetupButton: View {
    let computer: String

    @State private var isPresented = false
    @Environment(\.openURL) private var openURL

    static let tokenURL = URL(string: "https://github.com/settings/personal-access-tokens/new")!
    static let keychainCommand = "security add-generic-password -U -s me.heeka.jet.github -a github.com -w"
    static let linuxCommand = #"secret-tool store --label="Jet GitHub token" service me.heeka.jet.github account github.com"#

    var body: some View {
        Button(String(localized: "How to Set Up…")) { isPresented = true }
            .popover(isPresented: $isPresented, arrowEdge: .bottom) {
                VStack(alignment: .leading, spacing: 12) {
                    Text("1. Create a GitHub token that can write pull requests.")
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Create a Token on GitHub…") { openURL(Self.tokenURL) }
                    Text("2. In Terminal on \(computer), run this and paste the token:")
                        .fixedSize(horizontal: false, vertical: true)
                    commandRow(Self.keychainCommand)
                    Text("On Linux:")
                        .font(.system(size: JetDesign.TextSize.metadata))
                        .foregroundStyle(.secondary)
                    commandRow(Self.linuxCommand)
                }
                .font(.system(size: JetDesign.TextSize.navigation))
                .controlSize(.small)
                .padding(16)
                .frame(width: 440, alignment: .leading)
            }
    }

    private func commandRow(_ command: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(command)
                .font(.system(size: JetDesign.TextSize.control, design: .monospaced))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            DeliveryCopyButton(title: String(localized: "Copy"), value: command)
                .buttonStyle(.bordered)
        }
    }
}

// MARK: - Shell support

extension View {
    /// Hosts the Keep Changes confirmations (Mark as Checked…, Try Again…) and the
    /// VoiceOver announcements. Attach once at the shell root, after
    /// `ShellPresentations`.
    func keepChangesSupport(session: DesktopSession) -> some View {
        modifier(KeepChangesSupport(session: session))
    }
}

private struct KeepChangesSupport: ViewModifier {
    @Bindable var session: DesktopSession

    func body(content: Content) -> some View {
        content
            .confirmationDialog(
                title,
                isPresented: isPresented,
                titleVisibility: .visible,
                presenting: session.deliveries.pendingConfirmation
            ) { confirmation in
                switch confirmation {
                case let .markAsChecked(key, delivery):
                    Button("Mark as Checked") {
                        session.deliveries.pendingConfirmation = nil
                        Task { await session.confirmMarkAsChecked(delivery, for: key) }
                    }
                    Button("Cancel", role: .cancel) { session.deliveries.pendingConfirmation = nil }
                case .retryStep, .retryDelivery:
                    Button("Try Again") {
                        session.deliveries.pendingConfirmation = nil
                        session.confirmRetry(confirmation)
                    }
                    Button("Cancel", role: .cancel) { session.deliveries.pendingConfirmation = nil }
                }
            } message: { confirmation in
                switch confirmation {
                case .markAsChecked:
                    Text("Check the result yourself first. Jet won't retry or undo it.")
                case .retryStep, .retryDelivery:
                    Text("Jet sends this step to Git again as a new request.")
                }
            }
            // The older acknowledgement flow has no dialog of its own any more: route
            // it into Mark as Checked so it is never left without a host.
            .onChange(of: session.gitDeliveryAcknowledgementConfirmation) { _, delivery in
                guard let delivery else { return }
                session.cancelGitDeliveryAcknowledgement()
                session.requestMarkAsChecked(delivery)
            }
            .onChange(of: announcement) { _, text in
                guard let text else { return }
                AccessibilityNotification.Announcement(text).post()
            }
            .task(id: connectedPlaneIDs) {
                await session.refreshRestoredDeliveries()
            }
    }

    private var title: Text {
        switch session.deliveries.pendingConfirmation {
        case let .markAsChecked(key, delivery):
            Text("Mark “\(session.stepTitle(for: delivery, key: key))” as checked?")
        case let .retryStep(key, step):
            Text("Try “\(session.stepTitle(for: step, key: key))” again?")
        case let .retryDelivery(key, delivery):
            Text("Try “\(session.stepTitle(for: delivery, key: key))” again?")
        case nil:
            Text(verbatim: "")
        }
    }

    private var isPresented: Binding<Bool> {
        Binding(
            get: { session.deliveries.pendingConfirmation != nil },
            set: { if !$0 { session.deliveries.pendingConfirmation = nil } }
        )
    }

    /// The selected task's Keep outcome worth announcing: success, a warning or an
    /// error, never progress.
    private var announcement: String? {
        guard let ref = session.selectedDeliveryRef,
              let summary = session.keepChangesSummary(for: ref)
        else { return nil }
        switch summary.tone {
        case .success, .warning, .failure:
            return String(localized: "Keep Changes: \(summary.headline)")
        case .running, .neutral:
            return nil
        }
    }

    private var connectedPlaneIDs: [UUID] {
        session.planes.map(\.id).filter { session.isPlaneConnected($0) }
    }
}
