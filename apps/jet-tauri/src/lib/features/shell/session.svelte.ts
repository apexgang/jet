import {
  openPlaneFeed,
  type ConnectionSnapshot,
  type PlaneUpdate,
  type PublicError,
} from "$lib/jet/bridge";
import {
  fixtureForState,
  type DesktopFixtureScenario,
  type FixtureState,
} from "$lib/jet/fixture";

export type SidebarDestination =
  | "new-task"
  | "search"
  | "attention"
  | "project"
  | "conversation"
  | "schedules"
  | "planes"
  | "settings";

export type WorkPanelTab = "changes" | "files" | "terminal" | "run";
export type ConnectionViewState = "connecting" | "online" | "reconnecting" | "failed";

export class DesktopSession {
  scenario = $state<DesktopFixtureScenario>(fixtureForState("active"));
  sidebarSelection = $state<SidebarDestination>("conversation");
  sidebarPresented = $state(true);
  workPanelPresented = $state(true);
  selectedWorkPanel = $state<WorkPanelTab>("run");
  draft = $state("");
  actionNotice = $state<string | null>(null);
  composerFocusRequest = $state(0);
  connectionState = $state<ConnectionViewState>("connecting");
  connection = $state<ConnectionSnapshot | null>(null);
  failure = $state<PublicError | null>(null);

  get canSubmitDraft(): boolean {
    return this.draft.trim().length > 0;
  }

  get connectionLabel(): string {
    switch (this.connectionState) {
      case "online":
        return "Connected";
      case "connecting":
        return "Connecting";
      case "reconnecting":
        return "Reconnecting";
      case "failed":
        return "Unavailable";
    }
  }

  connect(): void {
    void openPlaneFeed((update) => this.receive(update))
      .then((snapshot) => {
        this.connection = snapshot.state === "online" ? snapshot : null;
        this.connectionState = snapshot.state;
      })
      .catch((error: unknown) => {
        const candidate = error as Partial<PublicError>;
        this.failure = {
          category: typeof candidate.category === "string" ? candidate.category : "offline",
          code: typeof candidate.code === "string" ? candidate.code : "transport.offline",
          message:
            typeof candidate.message === "string"
              ? candidate.message
              : "Jet could not reach this Plane.",
          retryable: candidate.retryable === true,
        };
        this.connectionState = candidate.retryable === false ? "failed" : "reconnecting";
      });
  }

  select(destination: SidebarDestination): void {
    this.sidebarSelection = destination;
    this.actionNotice = null;

    switch (destination) {
      case "new-task":
        this.showFixture("ready");
        this.workPanelPresented = false;
        this.composerFocusRequest += 1;
        break;
      case "search":
        this.actionNotice = "Search arrives with the Conversation list in Wave 1.3.";
        break;
      case "attention":
        this.showFixture("approval");
        this.workPanelPresented = true;
        break;
      case "project":
        this.showFixture("ready");
        this.workPanelPresented = false;
        break;
      case "conversation":
        this.showFixture("active");
        this.workPanelPresented = true;
        break;
      case "schedules":
        this.actionNotice = "Schedules are planned for Wave 3.";
        break;
      case "planes":
        this.actionNotice = "Connection management is planned for Wave 3.";
        break;
      case "settings":
        this.actionNotice = "Desktop settings arrive with the platform integration slices.";
        break;
    }
  }

  submitDraft(): void {
    if (!this.canSubmitDraft) return;
    // ASVS 2.1.1 and 2.2.2: this draft remains presentation-only. A later
    // typed Rust command must validate it again at the trusted boundary.
    this.actionNotice = "Task submission is not connected in this shell yet. Your draft was kept.";
  }

  showPanel(tab: WorkPanelTab): void {
    this.selectedWorkPanel = tab;
    this.workPanelPresented = true;
  }

  handleShortcut(event: KeyboardEvent): void {
    if (!(event.metaKey || event.ctrlKey)) return;

    if (event.key.toLowerCase() === "n") {
      event.preventDefault();
      this.select("new-task");
    } else if (event.key.toLowerCase() === "k") {
      event.preventDefault();
      this.select("search");
    } else if (event.altKey && event.key === "0") {
      event.preventDefault();
      this.workPanelPresented = !this.workPanelPresented;
    }
  }

  showFixture(state: FixtureState): void {
    this.actionNotice = null;
    this.scenario = fixtureForState(state);
  }

  private receive(update: PlaneUpdate): void {
    switch (update.type) {
      case "connected":
        this.connection = update.connection;
        this.connectionState = "online";
        this.failure = null;
        break;
      case "resumed":
      case "event":
        this.connectionState = "online";
        this.failure = null;
        break;
      case "reconnecting":
        this.connectionState = "reconnecting";
        this.failure = update.error;
        break;
      case "failed":
        this.connectionState = "failed";
        this.failure = update.error;
        break;
    }
  }
}
