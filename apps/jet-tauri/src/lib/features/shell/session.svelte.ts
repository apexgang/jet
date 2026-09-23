import {
  authorizeApprovalRetry,
  bindHarnessAccount,
  attachWorkspaceTerminal,
  closeWorkspaceTerminal,
  createConversation,
  detachWorkspaceTerminal,
  interruptTurn,
  loadConversation,
  loadConversations,
  loadRunSupervision,
  loadSetup,
  loadMoreChanges,
  loadPatchChunk,
  loadWorkFile,
  loadWorkPanel,
  openWorkspaceTerminal,
  openPlaneFeed,
  previewProject,
  previewProjectRemoval,
  registerProject,
  removeProject,
  resizeWorkspaceTerminal,
  saveWorkFile,
  searchConversations,
  sendTerminalInput,
  startRun,
  stopRun,
  submitFileReview,
  submitTurn,
  withdrawTurn,
  type ApprovalPresentation,
  type ConversationDetail,
  type ConversationRow,
  type ConversationSearchResult,
  type ConnectionSnapshot,
  type EditableFile,
  type PlaneUpdate,
  type ProjectPreview,
  type ProjectRemovalPreview,
  type PublicError,
  type PublicRecoveryAction,
  type WorkCheckpoint,
  type RunSupervision,
  type SetupSnapshot,
  type TerminalUpdate,
  type TurnQueueItem,
  type WorkPanelSnapshot,
} from "$lib/jet/bridge";
import { TerminalTranscriptDecoder } from "$lib/jet/terminal-text";
import { shouldLoadNextWorkPage } from "$lib/jet/work-continuity";
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

export type ConversationFreshness = "loading" | "live" | "cached" | "failed";
export type LiveTimelineEntry = {
  id: string;
  kind: "user" | "agent" | "activity" | "approval" | "result";
  text: string;
  sequence: string | null;
  rawCount: number;
  approval: ApprovalPresentation | null;
};

export type RunControlChoice = "interrupt_turn" | "stop_run";

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
  conversations = $state<ConversationRow[]>([]);
  conversationCursor = $state("0");
  nextConversationPage = $state<string | null>(null);
  selectedConversationId = $state<string | null>(null);
  conversationDetail = $state<ConversationDetail | null>(null);
  conversationFreshness = $state<ConversationFreshness>("loading");
  conversationBusy = $state(false);
  supervision = $state<RunSupervision | null>(null);
  supervisionBusy = $state(false);
  controlBusy = $state<string | null>(null);
  runControlConfirmation = $state<RunControlChoice | null>(null);
  timeline = $state<LiveTimelineEntry[]>([]);
  searchText = $state("");
  searchResult = $state<ConversationSearchResult | null>(null);
  searchBusy = $state(false);
  workPanel = $state<WorkPanelSnapshot | null>(null);
  workPanelBusy = $state(false);
  workPanelError = $state<PublicError | null>(null);
  workPanelNotice = $state<string | null>(null);
  selectedWorkFileId = $state<string | null>(null);
  editableFile = $state<EditableFile | null>(null);
  fileDraft = $state("");
  reviewLine = $state(1);
  reviewComment = $state("");
  selectedTerminalId = $state<string | null>(null);
  attachedTerminalId = $state<string | null>(null);
  terminalInput = $state("");
  terminalOutput = $state<Record<string, string>>({});
  checkpointKind = $state<WorkCheckpoint["kind"]>("current");
  checkpointTurn = $state(1);
  checkpointFromTurn = $state(0);
  checkpointToTurn = $state(1);
  terminalRows = $state(24);
  terminalColumns = $state(80);
  workPanelNoticeError = $state<PublicError | null>(null);

  private setupRequest = 0;
  private conversationRequest = 0;
  private searchRequest = 0;
  private workRequest = 0;
  private workFileRequest = 0;
  private detailRefresh: ReturnType<typeof setTimeout> | null = null;
  private patchDecoder: TextDecoder | null = null;
  private terminalDecoders = new Map<string, TerminalTranscriptDecoder>();
  private lastTerminalSize: { terminalId: string; rows: number; columns: number } | null = null;
  private feedGeneration = 0;

  get canSubmitDraft(): boolean {
    return (
      this.draft.trim().length > 0 &&
      this.draftBytes <= this.maximumPromptBytes &&
      !this.queueIsFull
    );
  }

  get draftBytes(): number {
    return new TextEncoder().encode(this.draft).byteLength;
  }

  get maximumPromptBytes(): number {
    return this.supervision?.maximumPromptBytes ?? 65_536;
  }

  get queueIsFull(): boolean {
    return (
      this.supervision !== null &&
      this.supervision.turns.length >= this.supervision.maximumEntries
    );
  }

  get attentionCount(): number {
    return this.timeline.filter(
      (entry) =>
        entry.approval?.state === "requested" ||
        entry.approval?.state === "unavailable" ||
        (entry.approval?.state === "denied" && entry.approval.canAuthorizeRetry),
    ).length;
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
    if (this.selectedConversationId) {
      if (!this.selectedConversation) return "Project unavailable";
      if (!this.selectedConversation.projectId) return "No Project";
      return (
        this.setupSnapshot?.projects.find(
          (project) => project.id === this.selectedConversation?.projectId,
        )?.name ?? "Project unavailable"
      );
    }
    return this.selectedProject?.name ?? "Choose a Project";
  }

  get selectedHarnessName(): string {
    return (
      this.setupSnapshot?.accounts[0]?.label ??
      this.setupSnapshot?.capabilities.authProviders[0]?.harness ??
      "Choose an Agent"
    );
  }

  get selectedCraftId(): string | null {
    return this.setupSnapshot?.capabilities.crafts[0]?.id ?? null;
  }

  get selectedConversation(): ConversationRow | null {
    return this.conversations.find((item) => item.id === this.selectedConversationId) ?? null;
  }

  get selectedConversationTitle(): string {
    return this.selectedConversation?.title ?? "New task";
  }

  get selectedRun() {
    return this.supervision?.execution?.run ?? this.conversationDetail?.runs.at(-1) ?? null;
  }

  get hasLiveRun(): boolean {
    return ["created", "starting", "active", "stopping"].includes(
      this.selectedRun?.lifecycle ?? "",
    );
  }

  get canInterruptTurn(): boolean {
    return (
      this.supervision?.execution?.run.lifecycle === "active" &&
      this.supervision.turns.some(
        (turn) => turn.state === "active" && turn.runId === this.selectedRun?.id,
      )
    );
  }

  get canStopRun(): boolean {
    return this.supervision?.execution?.run.lifecycle === "active";
  }

  connect(): void {
    void this.refreshSetup(true);
    void this.connectConversationFeed();
  }

  private async connectConversationFeed(): Promise<void> {
    await this.refreshConversations(true);
    await this.restartConversationFeed(this.conversationCursor);
  }

  private async restartConversationFeed(after: string): Promise<void> {
    const generation = ++this.feedGeneration;
    await openPlaneFeed(
      (update) => {
        if (generation === this.feedGeneration) this.receive(update);
      },
      after,
    )
      .then((snapshot) => {
        if (generation !== this.feedGeneration) return;
        this.connection = snapshot.state === "online" ? snapshot : null;
        this.connectionState = snapshot.state;
      })
      .catch((error: unknown) => {
        if (generation !== this.feedGeneration) return;
        const failure = publicError(error);
        this.failure = failure;
        this.connectionState = failure.retryable ? "reconnecting" : "failed";
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

  async refreshConversations(restoreSelection = false): Promise<void> {
    const request = ++this.conversationRequest;
    if (this.conversations.length === 0) this.conversationFreshness = "loading";
    try {
      const previousSelection = this.selectedConversationId;
      const page = await loadConversations();
      if (request !== this.conversationRequest) return;
      this.conversations = page.conversations;
      this.conversationCursor = page.cursor;
      this.nextConversationPage = page.nextPage;
      this.conversationFreshness = "live";
      const restored = restoreSelection ? page.restoredId : previousSelection;
      let restoredDetail: ConversationDetail | null = null;
      if (restored && !this.conversations.some((item) => item.id === restored)) {
        try {
          restoredDetail = await loadConversation(restored);
          this.conversations = [...this.conversations, restoredDetail.conversation];
        } catch {
          restoredDetail = null;
        }
        if (request !== this.conversationRequest) return;
      }
      const selected = page.conversations.some((item) => item.id === restored)
        ? restored
        : restoredDetail?.conversation.id ?? page.conversations[0]?.id ?? null;
      if (selected) {
        if (restoredDetail?.conversation.id === selected) {
          if (this.selectedConversationId !== selected) this.timeline = [];
          this.selectedConversationId = selected;
          this.conversationDetail = restoredDetail;
          await this.refreshSupervision(
            selected,
            restoredDetail.runs.at(-1)?.id ?? null,
          );
        } else {
          await this.openConversation(selected, false);
        }
      } else {
        this.selectedConversationId = null;
        this.conversationDetail = null;
        this.supervision = null;
      }
    } catch (error: unknown) {
      if (request !== this.conversationRequest) return;
      const failure = publicError(error);
      this.failure = failure;
      this.conversationFreshness = this.conversations.length > 0 ? "cached" : "failed";
    }
  }

  async loadMoreConversations(): Promise<void> {
    if (!this.nextConversationPage || this.conversationBusy) return;
    this.conversationBusy = true;
    const pageCursor = this.nextConversationPage;
    try {
      const page = await loadConversations(pageCursor);
      const known = new Set(this.conversations.map((item) => item.id));
      this.conversations = [
        ...this.conversations,
        ...page.conversations.filter((item) => !known.has(item.id)),
      ];
      this.nextConversationPage = page.nextPage;
      this.conversationFreshness = "live";
    } catch (error: unknown) {
      const failure = publicError(error);
      if (failure.restart?.reason === "pagination_stale") {
        await this.refreshConversations();
      } else {
        this.actionNotice = failure.message;
      }
    } finally {
      this.conversationBusy = false;
    }
  }

  async search(): Promise<void> {
    const text = this.searchText.trim();
    const request = ++this.searchRequest;
    if (!text) {
      this.searchResult = null;
      this.searchBusy = false;
      return;
    }
    this.searchBusy = true;
    try {
      const result = await searchConversations(text);
      if (request === this.searchRequest) this.searchResult = result;
    } catch (error: unknown) {
      if (request === this.searchRequest) this.actionNotice = publicError(error).message;
    } finally {
      if (request === this.searchRequest) this.searchBusy = false;
    }
  }

  async openConversation(conversationId: string, show = true): Promise<void> {
    if (this.selectedConversationId !== conversationId) {
      this.detachCurrentTerminal();
      this.timeline = [];
      this.conversationDetail = null;
      this.supervision = null;
      this.resetWorkPanel();
    }
    this.selectedConversationId = conversationId;
    if (show) {
      this.sidebarSelection = "conversation";
      this.workPanelPresented = true;
    }
    await this.loadSelectedConversation(true);
  }

  async openSearchHit(conversationId: string): Promise<void> {
    if (!this.conversations.some((item) => item.id === conversationId)) {
      await this.refreshConversations();
    }
    await this.openConversation(conversationId);
  }

  private async loadSelectedConversation(showLoading: boolean): Promise<void> {
    const id = this.selectedConversationId;
    if (!id) return;
    if (showLoading) this.conversationBusy = true;
    try {
      const detail = await loadConversation(id);
      if (this.selectedConversationId !== id) return;
      this.conversationDetail = detail;
      this.conversationFreshness = "live";
      this.mergeConversation(detail.conversation);
      await this.refreshSupervision(id, detail.runs.at(-1)?.id ?? null);
    } catch (error: unknown) {
      const failure = publicError(error);
      if (this.conversationDetail) {
        this.conversationFreshness = "cached";
      } else {
        this.conversationFreshness = "failed";
      }
      this.actionNotice = failure.message;
    } finally {
      this.conversationBusy = false;
    }
  }

  private mergeConversation(conversation: ConversationRow): void {
    const index = this.conversations.findIndex((item) => item.id === conversation.id);
    if (index === -1) {
      this.conversations = [conversation, ...this.conversations];
    } else {
      this.conversations[index] = conversation;
      this.conversations = [...this.conversations];
    }
  }

  private async refreshSupervision(
    conversationId: string,
    runId: string | null,
  ): Promise<void> {
    this.supervisionBusy = true;
    try {
      const supervision = await loadRunSupervision(conversationId, runId);
      if (this.selectedConversationId !== conversationId) return;
      this.supervision = supervision;
      const selectedRunId = supervision.execution?.run.id ?? runId;
      if (selectedRunId) await this.refreshWorkPanel(conversationId, selectedRunId);
    } catch (error: unknown) {
      if (this.selectedConversationId !== conversationId) return;
      this.actionNotice = publicError(error).message;
    } finally {
      this.supervisionBusy = false;
    }
  }

  select(destination: SidebarDestination): void {
    this.sidebarSelection = destination;
    this.actionNotice = null;

    switch (destination) {
      case "new-task":
        this.selectedConversationId = null;
        this.conversationDetail = null;
        this.timeline = [];
        this.supervision = null;
        this.workPanelPresented = false;
        this.composerFocusRequest += 1;
        break;
      case "search":
        this.workPanelPresented = false;
        break;
      case "attention":
        this.workPanelPresented = true;
        this.selectedWorkPanel = "run";
        this.actionNotice =
          this.attentionCount > 0 || this.supervision?.execution?.needsAttention
            ? "Review the highlighted request and the current Run controls."
            : "No current task needs your attention.";
        break;
      case "project":
        this.workPanelPresented = false;
        break;
      case "conversation":
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

  async submitDraft(): Promise<void> {
    if (!this.canSubmitDraft || this.conversationBusy) return;
    if (this.connectionState !== "online") {
      this.actionNotice = "Reconnect to the Plane before sending. Your draft was kept.";
      return;
    }
    const prompt = this.draft;
    const craft = this.selectedCraftId;
    if (!craft) {
      this.actionNotice = "Install an available Craft before starting work.";
      return;
    }
    this.conversationBusy = true;
    this.actionNotice = null;
    try {
      let conversationId = this.selectedConversationId;
      if (!conversationId) {
        if (!this.selectedProjectId) {
          this.actionNotice = "Choose a Project before starting a task.";
          return;
        }
        const conversation = await createConversation(this.selectedProjectId);
        this.mergeConversation(conversation);
        conversationId = conversation.id;
        this.selectedConversationId = conversation.id;
        this.sidebarSelection = "conversation";
        this.workPanelPresented = true;
        this.conversationDetail = await loadConversation(conversation.id);
      }

      if (this.hasLiveRun) {
        await submitTurn(conversationId, prompt);
      } else {
        await startRun(conversationId, craft, prompt);
      }
      this.draft = "";
      this.actionNotice = "Sent to the Plane.";
      await this.loadSelectedConversation(false);
    } catch (error: unknown) {
      this.actionNotice = publicError(error).message;
      await this.refreshConversations();
    } finally {
      this.conversationBusy = false;
    }
  }

  async withdrawQueuedTurn(turn: TurnQueueItem): Promise<void> {
    const conversationId = this.selectedConversationId;
    if (!conversationId || !turn.withdrawable || this.controlBusy) return;
    this.controlBusy = `withdraw-${turn.id}`;
    this.actionNotice = null;
    try {
      await withdrawTurn(conversationId, turn.id);
      this.actionNotice = "The queued Turn was withdrawn.";
      await this.refreshSupervision(conversationId, this.selectedRun?.id ?? null);
    } catch (error: unknown) {
      this.actionNotice = publicError(error).message;
    } finally {
      this.controlBusy = null;
    }
  }

  requestRunControl(control: RunControlChoice): void {
    if (control === "interrupt_turn" && !this.canInterruptTurn) return;
    if (control === "stop_run" && !this.canStopRun) return;
    this.runControlConfirmation = control;
  }

  cancelRunControl(): void {
    this.runControlConfirmation = null;
  }

  async confirmRunControl(): Promise<void> {
    const control = this.runControlConfirmation;
    const runId = this.selectedRun?.id;
    if (!control || !runId || this.controlBusy) return;
    this.runControlConfirmation = null;
    this.controlBusy = control;
    this.actionNotice = null;
    try {
      const accepted =
        control === "interrupt_turn" ? await interruptTurn(runId) : await stopRun(runId);
      this.actionNotice = accepted.message;
      const conversationId = this.selectedConversationId;
      if (conversationId) await this.refreshSupervision(conversationId, runId);
    } catch (error: unknown) {
      this.actionNotice = publicError(error).message;
    } finally {
      this.controlBusy = null;
    }
  }

  async retryApproval(approval: ApprovalPresentation): Promise<void> {
    if (
      !approval.canAuthorizeRetry ||
      !approval.reviewId ||
      !approval.runId ||
      this.controlBusy
    ) {
      return;
    }
    this.controlBusy = `approval-${approval.reviewId}`;
    this.actionNotice = null;
    try {
      const accepted = await authorizeApprovalRetry(approval.runId, approval.reviewId);
      this.actionNotice = accepted.message;
      this.timeline = this.timeline.map((entry) =>
        entry.approval?.reviewId === approval.reviewId
          ? {
              ...entry,
              approval: { ...entry.approval, canAuthorizeRetry: false },
            }
          : entry,
      );
    } catch (error: unknown) {
      this.actionNotice = publicError(error).message;
    } finally {
      this.controlBusy = null;
    }
  }

  showPanel(tab: WorkPanelTab): void {
    this.selectedWorkPanel = tab;
    this.workPanelPresented = true;
    const conversationId = this.selectedConversationId;
    const runId = this.selectedRun?.id;
    if (conversationId && runId && this.workPanel?.runId !== runId) {
      void this.refreshWorkPanel(conversationId, runId);
    }
  }

  get workCheckpoint(): WorkCheckpoint {
    switch (this.checkpointKind) {
      case "final":
        return { kind: "final" };
      case "turn":
        return { kind: "turn", turn: Math.max(1, Math.trunc(this.checkpointTurn)) };
      case "historical":
        return {
          kind: "historical",
          fromTurn: Math.max(0, Math.trunc(this.checkpointFromTurn)),
          toTurn: Math.max(1, Math.trunc(this.checkpointToTurn)),
        };
      default:
        return { kind: "current" };
    }
  }

  get canApplyWorkCheckpoint(): boolean {
    if (this.workPanelBusy) return false;
    const latestTurn = this.workPanel?.latestTurn ?? 0;
    switch (this.checkpointKind) {
      case "final":
        return this.selectedRun?.lifecycle !== "active" &&
          this.selectedRun?.lifecycle !== "starting" &&
          this.selectedRun?.lifecycle !== "stopping";
      case "turn":
        return Number.isInteger(this.checkpointTurn) &&
          this.checkpointTurn > 0 &&
          this.checkpointTurn <= latestTurn;
      case "historical":
        return Number.isInteger(this.checkpointFromTurn) &&
          Number.isInteger(this.checkpointToTurn) &&
          this.checkpointFromTurn >= 0 &&
          this.checkpointFromTurn <= this.checkpointToTurn &&
          this.checkpointToTurn <= latestTurn;
      default:
        return true;
    }
  }

  async applyWorkCheckpoint(): Promise<void> {
    if (!this.canApplyWorkCheckpoint) return;
    this.selectedWorkFileId = null;
    this.editableFile = null;
    this.fileDraft = "";
    await this.refreshWorkPanel(undefined, undefined, false);
  }

  async refreshWorkPanel(
    conversationId = this.selectedConversationId,
    runId = this.selectedRun?.id ?? null,
    preserveContinuity = true,
  ): Promise<void> {
    if (!conversationId || !runId) {
      this.resetWorkPanel();
      return;
    }
    const request = ++this.workRequest;
    this.workPanelBusy = true;
    this.workPanelError = null;
    this.workPanelNotice = null;
    this.workPanelNoticeError = null;
    const previous = this.workPanel;
    const preservePrevious = preserveContinuity && previous?.runId === runId;
    const previousSelectedFileId = preservePrevious ? this.selectedWorkFileId : null;
    const targetFileCount = preservePrevious
      ? previous.files.length
      : 0;
    if (previous?.runId !== runId) {
      this.checkpointKind = this.selectedRun?.lifecycle === "completed" ||
        this.selectedRun?.lifecycle === "failed" ||
        this.selectedRun?.lifecycle === "canceled" ||
        this.selectedRun?.lifecycle === "lost"
        ? "final"
        : "current";
    }
    try {
      const snapshot = await loadWorkPanel(conversationId, runId, this.workCheckpoint);
      while (shouldLoadNextWorkPage(
        new Set(snapshot.files.map((file) => file.id)),
        targetFileCount,
        previousSelectedFileId,
        snapshot.nextPage !== null,
      ) && snapshot.nextPage) {
        const page = await loadMoreChanges(snapshot.nextPage);
        const known = new Set(snapshot.files.map((file) => file.id));
        snapshot.files.push(...page.files.filter((file) => !known.has(file.id)));
        snapshot.nextPage = page.nextPage;
      }
      if (
        request !== this.workRequest ||
        this.selectedConversationId !== conversationId ||
        this.selectedRun?.id !== runId
      ) {
        return;
      }
      this.workPanel = snapshot;
      this.patchDecoder = new TextDecoder("utf-8", { fatal: true });
      this.workPanelNotice = snapshot.terminalIssue
        ? `Changes loaded, but terminals are unavailable: ${snapshot.terminalIssue.message}`
        : null;
      const selectedFileStillExists = snapshot.files.some(
        (file) => file.id === this.selectedWorkFileId,
      );
      if (!selectedFileStillExists) {
        this.selectedWorkFileId = snapshot.files[0]?.id ?? null;
        this.editableFile = null;
        this.fileDraft = "";
      }
      const selectedTerminalStillExists = snapshot.terminals.some(
        (terminal) => terminal.id === this.selectedTerminalId,
      );
      if (!selectedTerminalStillExists) {
        this.selectedTerminalId =
          snapshot.terminals.find((terminal) => terminal.state === "open")?.id ??
          snapshot.terminals[0]?.id ??
          null;
      }
    } catch (error: unknown) {
      if (request === this.workRequest) {
        this.workPanelError = publicError(error);
        this.applyRevisionConflict(this.workPanelError);
      }
    } finally {
      if (request === this.workRequest) this.workPanelBusy = false;
    }
  }

  async applyWorkRecovery(action: PublicRecoveryAction): Promise<void> {
    this.workPanelNoticeError = null;
    switch (action.type) {
      case "refresh_file":
        if (this.selectedWorkFileId) await this.selectWorkFile(this.selectedWorkFileId);
        return;
      case "refresh_conversation":
        await this.loadSelectedConversation(false);
        return;
      case "refresh_run":
        if (this.selectedConversationId) {
          await this.refreshSupervision(
            this.selectedConversationId,
            this.selectedRun?.id ?? null,
          );
        }
        return;
      case "resume_events":
        await this.restartConversationFeed(action.after);
        this.workPanelNotice = "Activity reconnected from the requested checkpoint.";
        return;
    }
  }

  async loadMoreWorkFiles(): Promise<void> {
    const pageId = this.workPanel?.nextPage;
    if (!pageId || this.workPanelBusy || !this.workPanel) return;
    this.workPanelBusy = true;
    this.workPanelNotice = null;
    this.workPanelNoticeError = null;
    try {
      const page = await loadMoreChanges(pageId);
      const known = new Set(this.workPanel.files.map((file) => file.id));
      this.workPanel.files = [
        ...this.workPanel.files,
        ...page.files.filter((file) => !known.has(file.id)),
      ];
      this.workPanel.nextPage = page.nextPage;
      this.workPanel = { ...this.workPanel };
    } catch (error: unknown) {
      const failure = publicError(error);
      this.workPanelNotice = failure.message;
      this.workPanelNoticeError = failure;
      this.applyRevisionConflict(failure);
      if (failure.restart?.reason === "pagination_stale") {
        await this.refreshWorkPanel();
      }
    } finally {
      this.workPanelBusy = false;
    }
  }

  async loadMorePatch(): Promise<void> {
    const readId = this.workPanel?.artifactReadId;
    if (!readId || this.workPanelBusy || !this.workPanel) return;
    this.workPanelBusy = true;
    this.workPanelNotice = null;
    this.workPanelNoticeError = null;
    try {
      const chunk = await loadPatchChunk(readId);
      const decoder = this.patchDecoder ?? new TextDecoder("utf-8", { fatal: true });
      this.patchDecoder = decoder;
      this.workPanel.patch += decoder.decode(new Uint8Array(chunk.bytes), {
        stream: !chunk.complete,
      });
      if (chunk.complete) {
        this.workPanel.artifactReadId = null;
        this.workPanel.patchTruncated = false;
      }
      this.workPanel = { ...this.workPanel };
      if (chunk.complete && chunk.verified) {
        this.workPanelNotice = "The complete patch was verified.";
      }
    } catch (error: unknown) {
      const failure = publicError(error);
      this.workPanelNotice = failure.message;
      this.workPanelNoticeError = failure;
      this.applyRevisionConflict(failure);
    } finally {
      this.workPanelBusy = false;
    }
  }

  async selectWorkFile(fileId: string): Promise<void> {
    if (!this.workPanel?.files.some((file) => file.id === fileId)) return;
    const request = ++this.workFileRequest;
    this.selectedWorkFileId = fileId;
    this.selectedWorkPanel = "files";
    this.editableFile = null;
    this.fileDraft = "";
    this.workPanelBusy = true;
    this.workPanelNotice = null;
    this.workPanelNoticeError = null;
    try {
      const file = await loadWorkFile(fileId);
      if (request !== this.workFileRequest || this.selectedWorkFileId !== fileId) return;
      this.editableFile = file;
      this.fileDraft = file.content ?? "";
    } catch (error: unknown) {
      if (request === this.workFileRequest) this.recordWorkFailure(error);
    } finally {
      if (request === this.workFileRequest) this.workPanelBusy = false;
    }
  }

  async saveSelectedFile(): Promise<void> {
    const fileId = this.selectedWorkFileId;
    if (!fileId || !this.editableFile || this.workPanelBusy) return;
    this.workPanelBusy = true;
    this.workPanelNotice = null;
    this.workPanelNoticeError = null;
    try {
      const saved = await saveWorkFile(fileId, this.fileDraft);
      this.editableFile.revision = saved.revision;
      this.editableFile.content = this.fileDraft;
      this.editableFile.contentBytes = new TextEncoder().encode(this.fileDraft).byteLength;
      this.editableFile = { ...this.editableFile };
      this.workPanelNotice = saved.message;
    } catch (error: unknown) {
      this.recordWorkFailure(error);
    } finally {
      this.workPanelBusy = false;
    }
  }

  async submitSelectedFileReview(): Promise<void> {
    const fileId = this.selectedWorkFileId;
    const comment = this.reviewComment.trim();
    if (!fileId || !comment || this.reviewLine < 1 || this.workPanelBusy) return;
    this.workPanelBusy = true;
    this.workPanelNotice = null;
    this.workPanelNoticeError = null;
    try {
      const submitted = await submitFileReview(fileId, this.reviewLine, comment);
      this.reviewComment = "";
      this.workPanelNotice = submitted.message;
      const conversationId = this.selectedConversationId;
      if (conversationId) await this.refreshSupervision(conversationId, this.selectedRun?.id ?? null);
    } catch (error: unknown) {
      this.recordWorkFailure(error);
    } finally {
      this.workPanelBusy = false;
    }
  }

  async createTerminal(): Promise<void> {
    const conversationId = this.selectedConversationId;
    if (!conversationId || this.workPanelBusy) return;
    this.workPanelBusy = true;
    this.workPanelNotice = null;
    this.workPanelNoticeError = null;
    try {
      const terminal = await openWorkspaceTerminal(
        conversationId,
        this.terminalRows,
        this.terminalColumns,
      );
      if (this.workPanel) {
        this.workPanel.terminals = [
          ...this.workPanel.terminals.filter((item) => item.id !== terminal.id),
          terminal,
        ];
        this.workPanel = { ...this.workPanel };
      }
      this.selectedTerminalId = terminal.id;
      await this.attachTerminal(terminal.id);
    } catch (error: unknown) {
      this.recordWorkFailure(error);
    } finally {
      this.workPanelBusy = false;
    }
  }

  selectTerminal(terminalId: string): void {
    if (!this.workPanel?.terminals.some((terminal) => terminal.id === terminalId)) return;
    this.selectedTerminalId = terminalId;
  }

  async attachSelectedTerminal(): Promise<void> {
    if (this.selectedTerminalId) await this.attachTerminal(this.selectedTerminalId);
  }

  async detachSelectedTerminal(): Promise<void> {
    const terminalId = this.attachedTerminalId;
    if (!terminalId) return;
    this.attachedTerminalId = null;
    try {
      await detachWorkspaceTerminal(terminalId);
    } catch (error: unknown) {
      this.recordWorkFailure(error);
    }
  }

  async closeSelectedTerminal(): Promise<void> {
    const terminalId = this.selectedTerminalId;
    if (!terminalId || this.workPanelBusy) return;
    this.workPanelBusy = true;
    this.workPanelNotice = null;
    this.workPanelNoticeError = null;
    try {
      if (this.attachedTerminalId === terminalId) await this.detachSelectedTerminal();
      const terminal = await closeWorkspaceTerminal(terminalId);
      if (this.workPanel) {
        this.workPanel.terminals = this.workPanel.terminals.map((item) =>
          item.id === terminal.id ? terminal : item,
        );
        this.workPanel = { ...this.workPanel };
      }
      this.workPanelNotice = "The Workspace terminal was closed.";
    } catch (error: unknown) {
      this.recordWorkFailure(error);
    } finally {
      this.workPanelBusy = false;
    }
  }

  async sendTerminalLine(): Promise<void> {
    const terminalId = this.attachedTerminalId;
    if (!terminalId || !this.terminalInput) return;
    const input = `${this.terminalInput}\n`;
    this.terminalInput = "";
    try {
      await sendTerminalInput(terminalId, input);
    } catch (error: unknown) {
      this.terminalInput = input.slice(0, -1);
      this.recordWorkFailure(error);
    }
  }

  setTerminalGeometry(rows: number, columns: number): void {
    const boundedRows = Math.min(1000, Math.max(1, Math.trunc(rows)));
    const boundedColumns = Math.min(1000, Math.max(1, Math.trunc(columns)));
    if (boundedRows === this.terminalRows && boundedColumns === this.terminalColumns) return;
    this.terminalRows = boundedRows;
    this.terminalColumns = boundedColumns;
    void this.resizeAttachedTerminal();
  }

  private async resizeAttachedTerminal(): Promise<void> {
    const terminalId = this.attachedTerminalId;
    if (!terminalId) return;
    if (
      this.lastTerminalSize?.terminalId === terminalId &&
      this.lastTerminalSize.rows === this.terminalRows &&
      this.lastTerminalSize.columns === this.terminalColumns
    ) return;
    try {
      await resizeWorkspaceTerminal(terminalId, this.terminalRows, this.terminalColumns);
      this.lastTerminalSize = {
        terminalId,
        rows: this.terminalRows,
        columns: this.terminalColumns,
      };
    } catch (error: unknown) {
      const failure = publicError(error);
      this.workPanelNotice = failure.message;
      this.workPanelNoticeError = failure;
    }
  }

  private async attachTerminal(terminalId: string): Promise<void> {
    if (this.attachedTerminalId === terminalId) return;
    this.detachCurrentTerminal();
    this.attachedTerminalId = terminalId;
    try {
      await attachWorkspaceTerminal(terminalId, (update) => this.receiveTerminal(update));
      await this.resizeAttachedTerminal();
    } catch (error: unknown) {
      if (this.attachedTerminalId === terminalId) this.attachedTerminalId = null;
      this.recordWorkFailure(error);
    }
  }

  private receiveTerminal(update: TerminalUpdate): void {
    if (update.terminalId !== this.attachedTerminalId) return;
    if (update.type === "output") {
      const previous = this.terminalOutput[update.terminalId] ?? "";
      const decoder = this.terminalDecoders.get(update.terminalId) ?? new TerminalTranscriptDecoder();
      this.terminalDecoders.set(update.terminalId, decoder);
      this.terminalOutput[update.terminalId] = `${previous}${decoder.push(update.bytes)}`.slice(-1_048_576);
      this.terminalOutput = { ...this.terminalOutput };
    } else if (update.type === "gap") {
      this.terminalDecoders.set(update.terminalId, new TerminalTranscriptDecoder());
      const previous = this.terminalOutput[update.terminalId] ?? "";
      this.terminalOutput[update.terminalId] = `${previous}\n[${update.missingBytes} earlier bytes unavailable]\n`.slice(-1_048_576);
      this.terminalOutput = { ...this.terminalOutput };
    } else if (update.type === "finished") {
      const tail = this.terminalDecoders.get(update.terminalId)?.finish() ?? "";
      if (tail) {
        this.terminalOutput[update.terminalId] = `${this.terminalOutput[update.terminalId] ?? ""}${tail}`.slice(-1_048_576);
        this.terminalOutput = { ...this.terminalOutput };
      }
      this.attachedTerminalId = null;
      this.workPanelNotice = "The terminal session finished.";
    } else if (update.type === "failed") {
      this.attachedTerminalId = null;
      this.workPanelNotice = update.error.message;
      this.workPanelNoticeError = update.error;
    }
  }

  private detachCurrentTerminal(): void {
    const terminalId = this.attachedTerminalId;
    this.attachedTerminalId = null;
    if (terminalId) void detachWorkspaceTerminal(terminalId);
  }

  private resetWorkPanel(): void {
    this.workRequest += 1;
    this.workFileRequest += 1;
    this.workPanel = null;
    this.workPanelBusy = false;
    this.workPanelError = null;
    this.workPanelNotice = null;
    this.workPanelNoticeError = null;
    this.selectedWorkFileId = null;
    this.editableFile = null;
    this.fileDraft = "";
    this.patchDecoder = null;
    this.selectedTerminalId = null;
    this.terminalDecoders.clear();
    this.lastTerminalSize = null;
    this.detachCurrentTerminal();
  }

  private applyRevisionConflict(error: PublicError): void {
    const safeState = error.revisionConflict?.safeState;
    if (!safeState) return;
    if (safeState.type === "conversation") {
      this.conversations = this.conversations.map((conversation) =>
        conversation.id === safeState.conversationId
          ? { ...conversation, revision: safeState.revision }
          : conversation,
      );
      if (this.conversationDetail?.conversation.id === safeState.conversationId) {
        this.conversationDetail.conversation = {
          ...this.conversationDetail.conversation,
          revision: safeState.revision,
        };
        this.conversationDetail = { ...this.conversationDetail };
      }
      return;
    }
    if (this.conversationDetail) {
      this.conversationDetail.runs = this.conversationDetail.runs.map((run) =>
        run.id === safeState.runId
          ? { ...run, revision: safeState.revision, lifecycle: safeState.lifecycle }
          : run,
      );
      this.conversationDetail = { ...this.conversationDetail };
    }
    if (this.supervision?.execution?.run.id === safeState.runId) {
      this.supervision.execution.run = {
        ...this.supervision.execution.run,
        revision: safeState.revision,
        lifecycle: safeState.lifecycle,
      };
      this.supervision = { ...this.supervision };
    }
  }

  private recordWorkFailure(error: unknown): PublicError {
    const failure = publicError(error);
    this.workPanelNotice = failure.message;
    this.workPanelNoticeError = failure;
    this.applyRevisionConflict(failure);
    return failure;
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
        this.connectionState = "online";
        this.failure = null;
        void this.refreshConversations();
        break;
      case "event":
        this.connectionState = "online";
        this.failure = null;
        if (update.conversation_id === this.selectedConversationId) {
          this.receiveTimeline(update);
          this.scheduleDetailRefresh();
        }
        if (
          update.kind === "conversation.created" ||
          update.kind === "conversation.name_changed" ||
          update.kind === "conversation.trashed"
        ) {
          void this.refreshConversations();
        }
        break;
      case "reconnecting":
        this.connectionState = "reconnecting";
        this.failure = update.error;
        if (this.conversationDetail) this.conversationFreshness = "cached";
        break;
      case "failed":
        if (
          update.error.restart?.reason === "cursor_expired" ||
          update.error.restart?.reason === "cursor_ahead"
        ) {
          void this.recoverSnapshot();
          break;
        }
        this.connectionState = "failed";
        this.failure = update.error;
        if (this.conversationDetail) this.conversationFreshness = "cached";
        break;
    }
  }

  private receiveTimeline(update: Extract<PlaneUpdate, { type: "event" }>): void {
    if (update.timeline.length === 0) {
      const last = this.timeline.at(-1);
      if (last && last.rawCount > 0) {
        last.rawCount += 1;
        last.sequence = update.sequence;
        last.text = `${last.rawCount} background updates`;
        this.timeline = [...this.timeline];
      } else {
        this.timeline = [
          ...this.timeline,
          {
            id: `raw-${update.sequence}`,
            kind: "activity",
            text: "1 background update",
            sequence: update.sequence,
            rawCount: 1,
            approval: null,
          },
        ];
      }
      return;
    }

    for (const [index, item] of update.timeline.entries()) {
      const id = item.itemId ?? `${update.sequence}-${index}`;
      const existing = this.timeline.find((entry) => entry.id === id);
      if (existing && (item.kind === "user" || item.kind === "approval")) {
        if (item.kind === "user") existing.text += item.text;
        else {
          existing.text = item.text;
          existing.approval = item.approval;
        }
        existing.sequence = update.sequence;
        this.timeline = [...this.timeline];
      } else {
        this.timeline = [
          ...this.timeline,
          {
            id,
            kind: item.kind,
            text: item.text,
            sequence: update.sequence,
            rawCount: 0,
            approval: item.approval,
          },
        ].slice(-256);
      }
    }
  }

  private scheduleDetailRefresh(): void {
    if (this.detailRefresh) clearTimeout(this.detailRefresh);
    this.detailRefresh = setTimeout(() => {
      this.detailRefresh = null;
      void this.loadSelectedConversation(false);
    }, 200);
  }

  private async recoverSnapshot(): Promise<void> {
    this.timeline = [];
    this.actionNotice = "The activity cursor expired. Jet refreshed the full Conversation snapshot.";
    await this.refreshConversations();
    try {
      await this.restartConversationFeed(this.conversationCursor);
    } catch (error: unknown) {
      const failure = publicError(error);
      this.failure = failure;
      this.connectionState = failure.retryable ? "reconnecting" : "failed";
      this.actionNotice = failure.message;
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
    recoveryActions: Array.isArray(candidate.recoveryActions)
      ? candidate.recoveryActions.filter(isPublicRecoveryAction)
      : [],
    restart: candidate.restart && typeof candidate.restart === "object"
      ? candidate.restart
      : null,
    revisionConflict:
      candidate.revisionConflict && typeof candidate.revisionConflict === "object"
        ? candidate.revisionConflict
        : null,
  };
}

function isPublicRecoveryAction(value: unknown): value is PublicRecoveryAction {
  if (!value || typeof value !== "object" || !("type" in value)) return false;
  const type = (value as { type?: unknown }).type;
  if (type === "refresh_file" || type === "refresh_conversation" || type === "refresh_run") {
    return true;
  }
  return type === "resume_events" && typeof (value as { after?: unknown }).after === "string";
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
