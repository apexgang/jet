import Foundation
import Observation

enum SidebarDestination: String, CaseIterable, Hashable, Sendable {
    case newTask
    case search
    case needsAttention
    case project
    case conversation
    case schedules
    case planes
}

enum WorkPanelTab: String, CaseIterable, Hashable, Sendable {
    case changes
    case files
    case terminal
    case run

    var title: String {
        switch self {
        case .changes: "Changes"
        case .files: "Files"
        case .terminal: "Terminal"
        case .run: "Run"
        }
    }
}

@MainActor
@Observable
final class DesktopSession {
    enum ContentState {
        case loading
        case ready(DesktopFixtureScenario)
        case failed(String)
    }

    private enum FixtureLoadResult: Sendable {
        case success(DesktopFixtureCorpus)
        case failure(String)
    }

    var contentState: ContentState = .loading
    var sidebarSelection: SidebarDestination = .conversation
    var selectedWorkPanel: WorkPanelTab = .run
    var isWorkPanelPresented = true
    var draft = ""
    var composerFocusRequest = 0
    var actionNotice: String?

    private var scenarios: [DesktopFixtureState: DesktopFixtureScenario] = [:]
    private var didLoadFixtures = false

    var scenario: DesktopFixtureScenario? {
        guard case let .ready(scenario) = contentState else { return nil }
        return scenario
    }

    var canSubmitDraft: Bool {
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    func loadFoundationFixture() async {
        guard !didLoadFixtures else { return }
        didLoadFixtures = true

        guard let fixtureURL = Bundle.main.url(
            forResource: "presentation-v1",
            withExtension: "json"
        ) else {
            contentState = .failed("The shared desktop fixture is missing from this build.")
            return
        }

        let result = await Task.detached(priority: .userInitiated) {
            do {
                let data = try Data(contentsOf: fixtureURL, options: .mappedIfSafe)
                return FixtureLoadResult.success(try DesktopFixtureCorpus(data: data))
            } catch {
                return FixtureLoadResult.failure("Jet could not load the shared desktop fixture.")
            }
        }.value

        switch result {
        case let .success(corpus):
            scenarios = Dictionary(
                uniqueKeysWithValues: corpus.scenarios.map { ($0.state, $0) }
            )
            showScenario(.active, fallback: .ready)
        case let .failure(message):
            contentState = .failed(message)
        }
    }

    func restore(selection: String, workPanel: String, panelPresented: Bool) {
        if let selection = SidebarDestination(rawValue: selection) {
            sidebarSelection = selection
        }
        if let workPanel = WorkPanelTab(rawValue: workPanel) {
            selectedWorkPanel = workPanel
        }
        isWorkPanelPresented = panelPresented
        applySidebarSelection()
    }

    func beginNewTask() {
        sidebarSelection = .newTask
        showScenario(.ready)
        isWorkPanelPresented = false
        actionNotice = nil
        composerFocusRequest += 1
    }

    func selectSearch() {
        sidebarSelection = .search
        actionNotice = "Search arrives with the Conversation list in Wave 1.3."
    }

    func applySidebarSelection() {
        actionNotice = nil
        switch sidebarSelection {
        case .newTask:
            showScenario(.ready)
            isWorkPanelPresented = false
            composerFocusRequest += 1
        case .search:
            actionNotice = "Search arrives with the Conversation list in Wave 1.3."
        case .needsAttention:
            showScenario(.approval, fallback: .recovery)
            isWorkPanelPresented = true
        case .project:
            showScenario(.ready)
            isWorkPanelPresented = false
        case .conversation:
            showScenario(.active)
            isWorkPanelPresented = true
        case .schedules:
            actionNotice = "Schedules are planned for Wave 3."
        case .planes:
            actionNotice = "Connection management is planned for Wave 3."
        }
    }

    func submitDraft() {
        guard canSubmitDraft else { return }
        // ASVS 2.1.1 and 2.2.2: Wave 1.1 keeps the draft in presentation
        // state. A later typed adapter must validate it at the trusted Plane
        // boundary before it can become a Command.
        actionNotice = "Task submission is not connected in this shell yet. Your draft was kept."
    }

    func showFixture(_ state: DesktopFixtureState) {
        actionNotice = nil
        showScenario(state)
    }

    func retryFixtureLoad() {
        didLoadFixtures = false
        contentState = .loading
        Task { await loadFoundationFixture() }
    }

    private func showScenario(
        _ state: DesktopFixtureState,
        fallback: DesktopFixtureState? = nil
    ) {
        if let scenario = scenarios[state] ?? fallback.flatMap({ scenarios[$0] }) {
            contentState = .ready(scenario)
        }
    }
}
