import {
  discoverCraft,
  loadAccountDetail,
  loadAgents,
  loadUsageHistory,
  pickLocalCraftSource,
  prepareAccountBind,
  prepareAccountUnbind,
  prepareAutoContinue,
  prepareCraftDisable,
  type AccountDetail,
  type AgentsView,
  type AutoContinuePolicyInput,
  type CraftDisableMode,
  type CraftSourceInput,
  type HistoryDays,
  type LocalCraftSource,
  type UsageHistoryView,
} from "$lib/jet/agents";
import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import { LOCAL_PLANE, type PlaneId } from "$lib/jet/planes";
import { applySettingsChange, type AppliedDetail, type PlaneState, type SettingsReview } from "$lib/jet/settings";
import { sectionData, sectionStateFor, withFreshness, type SectionState } from "./model";

/** Which Agents action an operation belongs to. */
export type OperationTarget =
  | { action: "bind"; provider: string }
  | { action: "unbind"; bindingId: string }
  | { action: "auto_continue"; bindingId: string }
  | { action: "disable"; craftId: string }
  | { action: "install" };

/**
 * One Agents mutation. `confirm` shows the review's facts; `uncertain`
 * keeps the review ID so Retry resends the same request.
 */
export type AgentOperation =
  | { kind: "idle" }
  | { kind: "preparing"; target: OperationTarget }
  | { kind: "confirm"; target: OperationTarget; review: SettingsReview }
  | { kind: "applying"; target: OperationTarget; reviewId: string }
  | { kind: "uncertain"; target: OperationTarget; reviewId: string; error: PublicError }
  | { kind: "refused"; target: OperationTarget; error: PublicError }
  | { kind: "done"; target: OperationTarget; detail: AppliedDetail };

/** Picking local Craft files: the native dialog is open, or its result. */
export type LocalPick =
  | { kind: "none" }
  | { kind: "picking" }
  | { kind: "picked"; source: LocalCraftSource }
  | { kind: "failed"; error: PublicError };

/** What the Agents pane needs from the Settings session that owns it. */
export type AgentsHost = {
  planeStateChanged(state: PlaneState): void;
  /** A refusal that names Plane state (degraded audit, read-only recovery). */
  planeStateStale(): void;
  /** Why changes are paused now (read-only Recovery, a stale view); `null` when they may be sent. */
  mutationBlock(): "read_only" | "stale" | null;
};

function newer(sequence: string, cursor: string | null): boolean {
  if (cursor === null) return false;
  try {
    return BigInt(sequence) > BigInt(cursor);
  } catch {
    return false;
  }
}

function sameTarget(left: OperationTarget, right: OperationTarget): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

/**
 * The Agents pane of one Plane: Harnesses and Crafts, Accounts, usage and
 * Auto-continue. Every mutation goes through a native review and
 * `apply_settings_change`; every completion is checked against the Plane it
 * started for.
 */
export class AgentsSession {
  planeId = $state<PlaneId>(LOCAL_PLANE);
  view = $state<SectionState<AgentsView>>({ kind: "loading", last: null });
  /** An account event arrived after the loaded Account list. */
  accountsChanged = $state(false);
  /** Open account details an Auto-continue event made out of date. */
  changedDetails = $state<string[]>([]);
  details = $state<Record<string, SectionState<AccountDetail>>>({});
  historyDays = $state<HistoryDays>(7);
  /** `null`: the whole Plane. */
  historyBinding = $state<string | null>(null);
  history = $state<SectionState<UsageHistoryView>>({ kind: "loading", last: null });
  operation = $state<AgentOperation>({ kind: "idle" });
  localPick = $state<LocalPick>({ kind: "none" });

  private host: AgentsHost;
  private started = false;
  /** Whether this Plane's pane has been loaded since it was selected. */
  private loaded = false;
  private generation = 0;
  private viewRequest = 0;
  private historyRequest = 0;
  private detailRequests: Record<string, number> = {};
  private operationRequest = 0;
  private disposed = false;
  /** The newest account event that arrived while the view reloaded. */
  private missedAccounts: string | null = null;
  /** Applying and uncertain operations of Planes not shown now. */
  private retained = new Map<PlaneId, AgentOperation>();

  constructor(host: AgentsHost) {
    this.host = host;
  }

  /** Shows one Plane. Keeps an applying or uncertain operation for when the user returns. */
  select(planeId: PlaneId): void {
    if (this.started && planeId === this.planeId) return;
    if (this.operation.kind === "applying" || this.operation.kind === "uncertain") {
      this.retained.set(this.planeId, this.operation);
    }
    this.started = true;
    this.loaded = false;
    this.generation++;
    this.planeId = planeId;
    this.view = { kind: "loading", last: null };
    this.details = {};
    this.detailRequests = {};
    this.history = { kind: "loading", last: null };
    this.historyBinding = null;
    this.accountsChanged = false;
    this.changedDetails = [];
    this.localPick = { kind: "none" };
    this.missedAccounts = null;
    this.operation = this.retained.get(planeId) ?? { kind: "idle" };
    this.retained.delete(planeId);
  }

  /** Stops accepting completions (the window is closing). */
  dispose(): void {
    this.disposed = true;
    this.generation++;
  }

  /** The change watcher is reconnecting or stopped: every section shows its last values as stale. */
  markStale(): void {
    this.view = withFreshness(this.view, "stale");
    this.history = withFreshness(this.history, "stale");
    this.details = Object.fromEntries(
      Object.entries(this.details).map(([id, state]) => [id, withFreshness(state, "stale")]),
    );
  }

  /** Reloads what the pane has loaded (the watcher resumed). */
  async reloadIfLoaded(): Promise<void> {
    if (!this.loaded) return;
    await Promise.all([this.load(), this.loadHistory(), ...Object.keys(this.details).map((id) => this.loadDetail(id))]);
  }

  /**
   * Loads the pane. `freshCredentials` re-checks credential state, which is
   * what Check again does after unlocking the keyring.
   */
  async load(freshCredentials = false): Promise<void> {
    const { planeId, generation } = this;
    const request = ++this.viewRequest;
    const last = sectionData(this.view);
    this.view = { kind: "loading", last };
    try {
      const view = await loadAgents(planeId, freshCredentials);
      if (generation !== this.generation || request !== this.viewRequest) return;
      this.view = {
        kind: "ready",
        data: view,
        freshness: "live",
        issues: view.issues.map((issue) => ({ section: issue.section, error: issue.error })),
      };
      // An account event that arrived during the reload and is newer than it.
      this.accountsChanged = this.missedAccounts !== null && newer(this.missedAccounts, view.cursor);
      this.missedAccounts = null;
      this.host.planeStateChanged(view.planeState);
      // Details of accounts that are gone are dropped.
      const ids = new Set(view.accounts.map((account) => account.id));
      this.details = Object.fromEntries(Object.entries(this.details).filter(([id]) => ids.has(id)));
      if (this.historyBinding !== null && !ids.has(this.historyBinding)) this.historyBinding = null;
    } catch (error: unknown) {
      if (generation !== this.generation || request !== this.viewRequest) return;
      this.view = sectionStateFor(publicError(error), last);
      this.missedAccounts = null;
    }
  }

  /** Loads the pane once; later loads come from focus, events and Check again. */
  async ensureLoaded(): Promise<void> {
    if (this.loaded) return;
    this.loaded = true;
    await Promise.all([this.load(), this.loadHistory()]);
  }

  /** The watcher saw the Account list or Auto-continue change. */
  noteChange(kind: string, sequence: string): void {
    const data = sectionData(this.view);
    if (kind === "account.bound" || kind === "account.unbound") {
      if (this.view.kind === "loading") {
        // Compared with the cursor of the view being loaded once it arrives.
        if (this.missedAccounts === null || newer(sequence, this.missedAccounts)) this.missedAccounts = sequence;
      } else if (data && newer(sequence, data.cursor)) {
        this.accountsChanged = true;
      }
    } else if (kind === "auto_continue.changed" || kind === "auto_continue.configured") {
      // The event names no account: every detail loaded before it may be out of date.
      const stale = Object.entries(this.details)
        .filter(([, state]) => state.kind === "ready" && newer(sequence, state.data.cursor))
        .map(([id]) => id);
      this.changedDetails = [...new Set([...this.changedDetails, ...stale])];
    }
  }

  async loadDetail(bindingId: string): Promise<void> {
    const { planeId, generation } = this;
    const request = (this.detailRequests[bindingId] ?? 0) + 1;
    this.detailRequests[bindingId] = request;
    const previous = this.details[bindingId];
    const last = previous ? sectionData(previous) : null;
    this.details = { ...this.details, [bindingId]: { kind: "loading", last } };
    try {
      const detail = await loadAccountDetail(planeId, bindingId);
      if (generation !== this.generation || request !== this.detailRequests[bindingId]) return;
      this.details = {
        ...this.details,
        [bindingId]: {
          kind: "ready",
          data: detail,
          freshness: "live",
          issues: detail.issues.map((issue) => ({ section: issue.section, error: issue.error })),
        },
      };
      this.changedDetails = this.changedDetails.filter((id) => id !== bindingId);
    } catch (error: unknown) {
      if (generation !== this.generation || request !== this.detailRequests[bindingId]) return;
      this.details = { ...this.details, [bindingId]: sectionStateFor(publicError(error), last) };
    }
  }

  closeDetail(bindingId: string): void {
    const { [bindingId]: _closed, ...rest } = this.details;
    this.details = rest;
    this.changedDetails = this.changedDetails.filter((id) => id !== bindingId);
    this.detailRequests[bindingId] = (this.detailRequests[bindingId] ?? 0) + 1;
  }

  async loadHistory(days: HistoryDays = this.historyDays, bindingId: string | null = this.historyBinding): Promise<void> {
    const { planeId, generation } = this;
    const request = ++this.historyRequest;
    this.historyDays = days;
    this.historyBinding = bindingId;
    const last = sectionData(this.history);
    this.history = { kind: "loading", last };
    try {
      const history = await loadUsageHistory(planeId, bindingId, days);
      if (generation !== this.generation || request !== this.historyRequest) return;
      this.history = { kind: "ready", data: history, freshness: "live", issues: [] };
    } catch (error: unknown) {
      if (generation !== this.generation || request !== this.historyRequest) return;
      this.history = sectionStateFor(publicError(error), last);
    }
  }

  // -------------------------------------------------------------------------
  // Reviewed changes
  // -------------------------------------------------------------------------

  /** Whether a new change may start: nothing else is in flight or uncertain. */
  get idle(): boolean {
    return !["preparing", "applying", "uncertain"].includes(this.operation.kind);
  }

  /** Reviews a Harness-native sign-in; the user confirms the review. */
  prepareBind(provider: string): Promise<void> {
    return this.prepare({ action: "bind", provider }, () => prepareAccountBind(this.planeId, provider), false);
  }

  /** Reviews an Auto-continue policy; the user confirms the review. */
  prepareAutoContinue(bindingId: string, policy: AutoContinuePolicyInput): Promise<void> {
    return this.prepare(
      { action: "auto_continue", bindingId },
      () => prepareAutoContinue(this.planeId, bindingId, policy),
      false,
    );
  }

  /** Removes an Account binding. The dialog that calls this was the confirmation. */
  unbind(bindingId: string): Promise<void> {
    return this.prepare({ action: "unbind", bindingId }, () => prepareAccountUnbind(this.planeId, bindingId), true);
  }

  /** Disables a Craft. The dialog that calls this was the confirmation. */
  disableCraft(craftId: string, mode: CraftDisableMode): Promise<void> {
    return this.prepare({ action: "disable", craftId }, () => prepareCraftDisable(this.planeId, craftId, mode), true);
  }

  /**
   * Verifies a Craft source; the user confirms the preview. A local source
   * token is single-use and spent even when the check fails, so a refusal
   * asks for the files again instead of offering a Check that can't work.
   */
  discover(source: CraftSourceInput): Promise<void> {
    return this.prepare({ action: "install" }, () => discoverCraft(this.planeId, source), false, () => {
      if (source.type === "local" && this.localPick.kind === "picked") this.localPick = { kind: "none" };
    });
  }

  /** Opens the native file dialogs for a local Craft (this computer only). */
  async pickLocalSource(): Promise<void> {
    if (this.planeId !== LOCAL_PLANE || this.localPick.kind === "picking") return;
    const { planeId, generation } = this;
    this.localPick = { kind: "picking" };
    try {
      const source = await pickLocalCraftSource(planeId);
      if (generation !== this.generation) return;
      this.localPick = source ? { kind: "picked", source } : { kind: "none" };
    } catch (error: unknown) {
      if (generation !== this.generation) return;
      this.localPick = { kind: "failed", error: publicError(error) };
    }
  }

  /**
   * Sends the reviewed change shown in `confirm`. Nothing is sent while
   * changes are paused, even from a review opened before they were.
   */
  async confirm(): Promise<void> {
    const current = this.operation;
    if (current.kind !== "confirm") return;
    if (this.host.mutationBlock() !== null) return;
    await this.apply(current.target, current.review.reviewId);
  }

  /** Resends an uncertain change: same review ID, same body. */
  async retry(): Promise<void> {
    const current = this.operation;
    if (current.kind !== "uncertain") return;
    if (this.host.mutationBlock() !== null) return;
    await this.apply(current.target, current.reviewId);
  }

  /** Leaves a review, a refusal or a receipt. In-flight and uncertain changes stay. */
  dismiss(): void {
    if (!this.idle) return;
    this.operation = { kind: "idle" };
    if (this.localPick.kind !== "picking") this.localPick = { kind: "none" };
  }

  /** The operation, when it belongs to `target`. */
  operationFor(target: OperationTarget): AgentOperation {
    const current = this.operation;
    return current.kind !== "idle" && sameTarget(current.target, target) ? current : { kind: "idle" };
  }

  private async prepare(
    target: OperationTarget,
    run: () => Promise<SettingsReview>,
    confirmAtOnce: boolean,
    refused?: () => void,
  ): Promise<void> {
    if (!this.idle) return;
    const { generation } = this;
    const request = ++this.operationRequest;
    this.operation = { kind: "preparing", target };
    let review: SettingsReview;
    try {
      review = await run();
    } catch (error: unknown) {
      if (generation !== this.generation || request !== this.operationRequest) return;
      // Nothing was sent: a prepare only reads.
      this.operation = { kind: "refused", target, error: publicError(error) };
      refused?.();
      return;
    }
    if (generation !== this.generation || request !== this.operationRequest) return;
    this.operation = { kind: "confirm", target, review };
    if (confirmAtOnce) await this.confirm();
  }

  private async apply(target: OperationTarget, reviewId: string): Promise<void> {
    const { planeId } = this;
    // Any prepare still in flight is out of date.
    this.operationRequest++;
    this.operation = { kind: "applying", target, reviewId };
    let receipt;
    try {
      receipt = await applySettingsChange(planeId, reviewId);
    } catch (thrown: unknown) {
      const uncertain: AgentOperation = { kind: "uncertain", target, reviewId, error: publicError(thrown) };
      if (this.owns(planeId, reviewId)) this.operation = uncertain;
      // The user switched Planes; keep the uncertainty for when they return.
      else if (!this.disposed && planeId !== this.planeId) this.retained.set(planeId, uncertain);
      return;
    }
    if (!this.owns(planeId, reviewId)) {
      // Answered while its Plane isn't shown: nothing is left to retry.
      const held = this.retained.get(planeId);
      if (held && (held.kind === "applying" || held.kind === "uncertain") && held.reviewId === reviewId) {
        this.retained.delete(planeId);
      }
      return;
    }
    switch (receipt.kind) {
      case "applied":
        this.operation = { kind: "done", target, detail: receipt.detail };
        if (target.action === "install") this.localPick = { kind: "none" };
        await this.load();
        if (target.action === "auto_continue") await this.loadDetail(target.bindingId);
        return;
      case "refused":
        this.operation = { kind: "refused", target, error: receipt.error };
        if (receipt.error.code === "security.audit_degraded" || receipt.error.code === "recovery.read_only") {
          this.host.planeStateStale();
        }
        return;
      case "changed":
        // Only Setting reviews have a fresh-read guard.
        this.operation = { kind: "refused", target, error: publicError(null) };
        return;
    }
  }

  /** Whether the Plane on screen still shows this review being sent. */
  private owns(planeId: PlaneId, reviewId: string): boolean {
    if (this.disposed || planeId !== this.planeId) return false;
    return this.operation.kind === "applying" && this.operation.reviewId === reviewId;
  }
}
