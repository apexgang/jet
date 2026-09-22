import {
  bindHarnessAccount,
  loadSetup,
  openPlaneFeed,
  previewProject,
  previewProjectRemoval,
  registerProject,
  removeProject,
  type ConnectionSnapshot,
  type PlaneUpdate,
  type ProjectPreview,
  type ProjectRemovalPreview,
  type PublicError,
  type SetupSnapshot,
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
export type SetupViewState =
  | { kind: "loading" }
  | { kind: "ready"; snapshot: SetupSnapshot }
  | { kind: "failed"; error: PublicError };

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
  setup = $state<SetupViewState>({ kind: "loading" });
  selectedProjectId = $state<string | null>(null);
  projectPath = $state("");
  projectPreview = $state<ProjectPreview | null>(null);
  removalPreview = $state<ProjectRemovalPreview | null>(null);
  permanentRemovalAllowed = $state(false);
  setupBusy = $state<string | null>(null);
  setupNotice = $state<string | null>(null);
  remotePairingSkipped = $state(false);

  private setupRequest = 0;

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

  get setupSnapshot(): SetupSnapshot | null {
    return this.setup.kind === "ready" ? this.setup.snapshot : null;
  }

  setupIssue(section: SetupSnapshot["issues"][number]["section"]) {
    return this.setupSnapshot?.issues.find((issue) => issue.section === section) ?? null;
  }

  get selectedProject() {
    return this.setupSnapshot?.projects.find((project) => project.id === this.selectedProjectId) ?? null;
  }

  get selectedProjectName(): string {
    return this.selectedProject?.name ?? this.scenario.project?.name ?? "Choose a Project";
  }

  get selectedHarnessName(): string {
    return (
      this.setupSnapshot?.accounts[0]?.label ??
      this.setupSnapshot?.capabilities.authProviders[0]?.harness ??
      this.scenario.capabilities.harnesses[0] ??
      "Choose"
    );
  }

  connect(): void {
    void this.refreshSetup(true);
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

  async refreshSetup(openWhenIncomplete = false): Promise<void> {
    const request = ++this.setupRequest;
    if (this.setup.kind !== "ready") this.setup = { kind: "loading" };
    try {
      const snapshot = await loadSetup();
      if (request !== this.setupRequest) return;
      this.setup = { kind: "ready", snapshot };
      this.failure = null;
      const projectsAvailable = !snapshot.issues.some((issue) => issue.section === "projects");
      const accountsAvailable = !snapshot.issues.some((issue) => issue.section === "accounts");
      if (
        projectsAvailable &&
        !snapshot.projects.some((project) => project.id === this.selectedProjectId)
      ) {
        this.selectedProjectId = snapshot.projects[0]?.id ?? null;
      }
      if (
        openWhenIncomplete &&
        ((projectsAvailable && snapshot.projects.length === 0) ||
          (accountsAvailable && snapshot.accounts.length === 0))
      ) {
        this.sidebarSelection = "project";
        this.workPanelPresented = false;
      }
    } catch (error: unknown) {
      if (request !== this.setupRequest) return;
      this.setup = { kind: "failed", error: publicError(error) };
    }
  }

  selectProject(projectId: string): void {
    if (!this.setupSnapshot?.projects.some((project) => project.id === projectId)) return;
    this.selectedProjectId = projectId;
    this.sidebarSelection = "project";
    this.workPanelPresented = false;
    this.setupNotice = null;
  }

  async previewProjectPath(): Promise<void> {
    if (!this.projectPath.trim() || this.setupBusy) return;
    this.setupBusy = "preview-project";
    this.setupNotice = null;
    this.projectPreview = null;
    try {
      this.projectPreview = await previewProject(this.projectPath.trim());
    } catch (error: unknown) {
      this.setupNotice = publicError(error).message;
    } finally {
      this.setupBusy = null;
    }
  }

  async registerPreviewedProject(): Promise<void> {
    const previewId = this.projectPreview?.previewId;
    if (!previewId || this.setupBusy) return;
    this.setupBusy = "register-project";
    this.setupNotice = null;
    try {
      const project = await registerProject(previewId);
      this.projectPath = "";
      this.projectPreview = null;
      this.setupNotice = `${project.name} is ready.`;
      await this.refreshSetup();
      this.selectedProjectId = project.id;
    } catch (error: unknown) {
      this.setupNotice = publicError(error).message;
    } finally {
      this.setupBusy = null;
    }
  }

  async prepareProjectRemoval(projectId: string): Promise<void> {
    if (this.setupBusy) return;
    this.setupBusy = `remove-${projectId}`;
    this.setupNotice = null;
    this.permanentRemovalAllowed = false;
    try {
      this.removalPreview = await previewProjectRemoval(projectId);
    } catch (error: unknown) {
      this.setupNotice = publicError(error).message;
    } finally {
      this.setupBusy = null;
    }
  }

  cancelProjectRemoval(): void {
    this.removalPreview = null;
    this.permanentRemovalAllowed = false;
  }

  async confirmProjectRemoval(typedName: string, permanent: boolean): Promise<void> {
    if (!this.removalPreview || this.setupBusy) return;
    this.setupBusy = "confirm-removal";
    this.setupNotice = null;
    try {
      const removed = await removeProject(
        this.removalPreview.previewId,
        typedName,
        permanent,
      );
      this.removalPreview = null;
      this.permanentRemovalAllowed = false;
      this.setupNotice = permanent
        ? `${removed.name} was deleted.`
        : `${removed.name} was moved to Trash.`;
      await this.refreshSetup();
    } catch (error: unknown) {
      const failure = publicError(error);
      this.permanentRemovalAllowed = failure.code === "project.trash_unavailable";
      this.setupNotice = failure.message;
    } finally {
      this.setupBusy = null;
    }
  }

  async connectHarness(provider: string): Promise<void> {
    if (this.setupBusy) return;
    this.setupBusy = `account-${provider}`;
    this.setupNotice = null;
    try {
      const binding = await bindHarnessAccount(provider);
      this.setupNotice = `${binding.name} is connected.`;
      await this.refreshSetup();
    } catch (error: unknown) {
      this.setupNotice = publicError(error).message;
    } finally {
      this.setupBusy = null;
    }
  }

  skipRemotePairing(): void {
    this.remotePairingSkipped = true;
    this.setupNotice = "Remote pairing was skipped. You can return here at any time.";
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

export function publicError(error: unknown): PublicError {
  const candidate = publicErrorCandidate(error);
  return {
    category: typeof candidate.category === "string" ? candidate.category : "internal",
    code: typeof candidate.code === "string" ? candidate.code : "client.request_failed",
    message:
      typeof candidate.message === "string"
        ? candidate.message
        : "Jet could not complete the request.",
    retryable: candidate.retryable === true,
  };
}

function publicErrorCandidate(error: unknown): Partial<PublicError> {
  let candidate = error;
  if (typeof candidate === "string") {
    try {
      candidate = JSON.parse(candidate) as unknown;
    } catch {
      return {};
    }
  }

  if (candidate && typeof candidate === "object") {
    const record = candidate as Record<string, unknown>;
    if ("category" in record || "code" in record || "retryable" in record) {
      return record as Partial<PublicError>;
    }
    for (const key of ["error", "data", "cause"] as const) {
      if (key in record && record[key] !== candidate) {
        const nested = publicErrorCandidate(record[key]);
        if (nested.code || nested.category) return nested;
      }
    }
  }
  return {};
}
