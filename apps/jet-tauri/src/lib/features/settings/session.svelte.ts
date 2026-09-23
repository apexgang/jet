import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import { LOCAL_PLANE, type PlaneId } from "$lib/jet/planes";
import {
  applySettingsChange,
  loadSettings,
  loadWorkContext,
  prepareSettingChange,
  watchSettingsChanges,
  type PlaneState,
  type ResolvedSetting,
  type SettingChange,
  type SettingKeyId,
  type SettingsChange,
  type SettingsSnapshot,
  type SettingValue,
  type WorkContext,
} from "$lib/jet/settings";
import { AutodeleteSession, type RulesBlock } from "$lib/features/autodelete/session.svelte";
import { SystemSession } from "$lib/features/system/session.svelte";
import { AgentsSession } from "./agents-session.svelte";
import { ExtensionsSession } from "./extensions-session.svelte";
import {
  isSensitive,
  sameSetting,
  sectionData,
  sectionStateFor,
  withFreshness,
  type SectionState,
  type SettingRowScope,
  type SettingRowState,
} from "./model";

/** The loaders a Settings Plane pane reads from. */
export type SettingsLoader = "plane" | "project" | "work";

/** Why a row cannot change right now; `null` when it can. */
export type MutationBlock = "read_only" | "stale" | null;

/**
 * The change watcher. `stale`: reconnecting, values are the last ones seen.
 * `failed`: stopped; only Try again restarts it.
 */
export type WatchState = "idle" | "live" | "stale" | "failed";

/** A row is one key at one scope. */
export function rowKey(key: SettingKeyId, scope: SettingRowScope): string {
  return scope.type === "plane" ? `plane|${key}` : `project|${scope.projectId}|${key}`;
}

function newer(sequence: string, cursor: string | null): boolean {
  if (cursor === null) return false;
  try {
    return BigInt(sequence) > BigInt(cursor);
  } catch {
    return false;
  }
}

function ready<T>(data: T): SectionState<T> {
  return { kind: "ready", data, freshness: "live", issues: [] };
}

/** Events kept while a section reloads; beyond this the section is marked changed. */
const MAX_MISSED_EVENTS = 64;

type MissedEvents = { changes: Array<Extract<SettingsChange, { type: "change" }>>; overflow: boolean };

function noMissed(): Record<SettingsLoader, MissedEvents> {
  return {
    plane: { changes: [], overflow: false },
    project: { changes: [], overflow: false },
    work: { changes: [], overflow: false },
  };
}

/** The draft a row in flight carries, so "Apply my value again" can resend it. */
function draftOf(row: SettingRowState | undefined): SettingValue | null {
  switch (row?.kind) {
    case "editing":
      return row.draft;
    case "checking":
      return row.draft;
    case "confirm":
      return row.review.after;
    case "changed_elsewhere":
    case "applying":
    case "uncertain":
      return row.draft;
    default:
      return null;
  }
}

function changeFor(draft: SettingValue | null): SettingChange | null {
  if (draft === null) return { kind: "clear" };
  if (draft.type === "undisplayable") return null;
  return { kind: "set", value: draft };
}

/**
 * One Settings window's Plane settings: the Plane snapshot, the selected
 * Project's snapshot, the Work context, every row's editing state and the
 * change watcher. Every async completion is checked against the Plane and
 * request it started for.
 */
export class SettingsSession {
  planeId = $state<PlaneId>(LOCAL_PLANE);
  planeLabel = $state("This computer");
  planeState = $state<PlaneState | null>(null);
  plane = $state<SectionState<SettingsSnapshot>>({ kind: "loading", last: null });
  work = $state<SectionState<WorkContext>>({ kind: "loading", last: null });
  projectId = $state<string | null>(null);
  project = $state<SectionState<SettingsSnapshot> | null>(null);
  rows = $state<Record<string, SettingRowState>>({});
  watch = $state<WatchState>("idle");
  watchError = $state<PublicError | null>(null);
  /** `usage.recorded` arrived; usage is never reloaded automatically. */
  usageNewer = $state(false);
  /** A `schedule.*` event arrived; there is no list to reload. */
  schedulesChanged = $state(false);
  /** The Agents pane of the same Plane. */
  readonly agents = new AgentsSession({
    planeStateChanged: (state) => {
      this.planeState = state;
    },
    planeStateStale: () => void this.loadPlane(),
    mutationBlock: () => this.agentsBlock(),
  });
  /** Agents › Extensions of the same Plane. */
  readonly extensions = new ExtensionsSession({
    planeStateStale: () => void this.loadPlane(),
    mutationBlock: () => this.agentsBlock(),
  });
  /**
   * Safety › Versions, Storage health, Recovery and Diagnostics of the same
   * Plane. A new Jet service start it detects invalidates the auto-delete
   * rules too, and a restored snapshot reloads every section.
   */
  readonly system = new SystemSession(
    Date.now,
    () => this.autodelete.planeRestarted(),
    () => void this.reload(),
  );
  /** Work › Retention › Auto-delete of the same Plane. */
  readonly autodelete = new AutodeleteSession({
    mutationBlock: () => this.autodeleteBlock(),
    observe: (error) => this.system.observe(error),
  });

  private started = false;
  private disposed = false;
  /** Bumped on every Plane switch; every completion checks it. */
  private generation = 0;
  private requests: Record<SettingsLoader, number> = { plane: 0, project: 0, work: 0 };
  private watchGeneration = 0;
  /** Applying and uncertain rows of Planes not shown now, keyed by Plane and row. */
  private retained = new Map<string, SettingRowState>();
  /**
   * Watcher events that arrived while their section reloaded. Replayed
   * against the new snapshot's cursor, so a change committed after the
   * snapshot was read is never lost.
   */
  private missed = noMissed();

  /**
   * Shows one Plane. Rows of the previous Plane are dropped except those
   * still being sent or uncertain: their outcome is kept for when the user
   * returns.
   */
  select(planeId: PlaneId, label?: string): void {
    if (label !== undefined) this.planeLabel = label;
    if (this.started && planeId === this.planeId) return;
    this.retainPending();
    this.started = true;
    this.generation++;
    this.watchGeneration++;
    this.requests = { plane: this.requests.plane + 1, project: this.requests.project + 1, work: this.requests.work + 1 };
    this.planeId = planeId;
    this.planeState = null;
    this.plane = { kind: "loading", last: null };
    this.work = { kind: "loading", last: null };
    this.projectId = null;
    this.project = null;
    this.rows = this.restorePending(planeId);
    this.missed = noMissed();
    this.watch = "idle";
    this.watchError = null;
    this.usageNewer = false;
    this.schedulesChanged = false;
    this.agents.select(planeId);
    this.extensions.select(planeId);
    this.system.select(planeId);
    this.autodelete.select(planeId);
    void this.reload();
  }

  /** Stops watching. The native watcher also stops when the window closes. */
  dispose(): void {
    this.disposed = true;
    this.generation++;
    this.watchGeneration++;
    this.watch = "idle";
    this.agents.dispose();
    this.extensions.dispose();
    this.system.dispose();
    this.autodelete.dispose();
  }

  /** Reloads every section, then watches from the lowest section cursor. */
  async reload(): Promise<void> {
    const generation = this.generation;
    this.watchGeneration++;
    // Agents and Extensions reload only once their pane has loaded them.
    await Promise.all([
      this.loadPlane(),
      this.loadWork(),
      this.agents.reloadIfLoaded(),
      this.extensions.reloadLoaded(),
      this.system.reloadIfLoaded(),
      this.autodelete.reloadIfLoaded(),
    ]);
    if (generation !== this.generation) return;
    const projects = sectionData(this.work)?.projects ?? [];
    if (!this.projectId || !projects.some((project) => project.id === this.projectId)) {
      this.projectId = projects[0]?.id ?? null;
      this.project = this.projectId ? { kind: "loading", last: null } : null;
    }
    if (this.projectId) await this.loadProject();
    if (generation !== this.generation) return;
    this.startWatch();
  }

  /** Chooses the Project whose defaults the Projects section edits. */
  async selectProject(projectId: string): Promise<void> {
    if (projectId === this.projectId) return;
    const projects = sectionData(this.work)?.projects ?? [];
    if (!projects.some((project) => project.id === projectId)) return;
    this.projectId = projectId;
    this.project = { kind: "loading", last: null };
    this.missed.project = { changes: [], overflow: false };
    await this.loadProject();
  }

  async loadPlane(): Promise<void> {
    const { planeId, generation } = this;
    const request = ++this.requests.plane;
    const last = sectionData(this.plane);
    this.plane = { kind: "loading", last };
    try {
      const snapshot = await loadSettings(planeId, { type: "plane" });
      if (generation !== this.generation || request !== this.requests.plane) return;
      this.plane = ready(snapshot);
      this.planeState = snapshot.planeState;
      this.replayMissed("plane");
    } catch (error: unknown) {
      if (generation !== this.generation || request !== this.requests.plane) return;
      this.plane = sectionStateFor(this.noted(error), last);
      this.missed.plane = { changes: [], overflow: false };
    }
  }

  async loadWork(): Promise<void> {
    const { planeId, generation } = this;
    const request = ++this.requests.work;
    const last = sectionData(this.work);
    this.work = { kind: "loading", last };
    try {
      const context = await loadWorkContext(planeId);
      if (generation !== this.generation || request !== this.requests.work) return;
      this.work = {
        kind: "ready",
        data: context,
        freshness: "live",
        issues: context.issues.map((issue) => ({ section: issue.section, error: issue.error })),
      };
      this.planeState = context.planeState;
      this.replayMissed("work");
    } catch (error: unknown) {
      if (generation !== this.generation || request !== this.requests.work) return;
      this.work = sectionStateFor(this.noted(error), last);
      this.missed.work = { changes: [], overflow: false };
    }
  }

  async loadProject(): Promise<void> {
    const { planeId, generation, projectId } = this;
    if (!projectId) return;
    const request = ++this.requests.project;
    const last = this.project ? sectionData(this.project) : null;
    this.project = { kind: "loading", last };
    try {
      const snapshot = await loadSettings(planeId, { type: "project", project_id: projectId });
      if (generation !== this.generation || request !== this.requests.project || projectId !== this.projectId) return;
      this.project = ready(snapshot);
      this.replayMissed("project");
    } catch (error: unknown) {
      if (generation !== this.generation || request !== this.requests.project || projectId !== this.projectId) return;
      this.project = sectionStateFor(this.noted(error), last);
      this.missed.project = { changes: [], overflow: false };
    }
  }

  /** The section state a row's scope reads from. */
  sectionFor(scope: SettingRowScope): SectionState<SettingsSnapshot> | null {
    if (scope.type === "plane") return this.plane;
    return scope.projectId === this.projectId ? this.project : null;
  }

  /** The resolved value a row shows, `undefined` when the Plane's minor lacks the key. */
  setting(key: SettingKeyId, scope: SettingRowScope): ResolvedSetting | undefined {
    const state = this.sectionFor(scope);
    const snapshot = state ? sectionData(state) : null;
    return snapshot?.settings.find((setting) => setting.key === key);
  }

  row(key: SettingKeyId, scope: SettingRowScope): SettingRowState {
    return this.rows[rowKey(key, scope)] ?? { kind: "idle" };
  }

  /**
   * Controls are disabled by Plane state, never by a copied key list:
   * read-only Recovery pauses every change, and so does a stale view.
   */
  mutationBlock(scope: SettingRowScope): MutationBlock {
    if (this.planeState?.recovery === "read_only") return "read_only";
    if (this.watch === "stale" || this.watch === "failed") return "stale";
    const state = this.sectionFor(scope);
    if (!state || state.kind !== "ready" || state.freshness === "stale") return "stale";
    return null;
  }

  /**
   * Whether Agents changes may start. Like setting rows: read-only
   * Recovery and a stale view pause them; the Agents data must be live.
   */
  agentsBlock(): MutationBlock {
    if (this.planeState?.recovery === "read_only") return "read_only";
    if (this.watch === "stale" || this.watch === "failed") return "stale";
    const view = this.agents.view;
    if (view.kind !== "ready" || view.freshness === "stale") return "stale";
    return null;
  }

  /**
   * Whether auto-delete rule changes may start: read-only Recovery and a
   * stale watcher pause them. The rules view checks its own freshness.
   */
  autodeleteBlock(): RulesBlock {
    if (this.planeState?.recovery === "read_only") return "read_only";
    if (this.watch === "stale" || this.watch === "failed") return "stale";
    return null;
  }

  /** Starts editing a text or number row. */
  edit(key: SettingKeyId, scope: SettingRowScope, draft: SettingValue): void {
    const current = this.row(key, scope);
    if (current.kind !== "idle" && current.kind !== "editing" && current.kind !== "refused") return;
    this.setRow(rowKey(key, scope), { kind: "editing", draft });
  }

  /** Abandons an edit, a confirm step, a refusal or a changed-elsewhere notice. */
  dismiss(key: SettingKeyId, scope: SettingRowScope): void {
    const current = this.row(key, scope);
    if (current.kind === "applying" || current.kind === "checking" || current.kind === "uncertain") return;
    this.setRow(rowKey(key, scope), { kind: "idle" });
  }

  /**
   * Prepares one change against the section's snapshot. Non-sensitive rows
   * apply right away; sensitive rows stop at `confirm`.
   */
  async change(key: SettingKeyId, scope: SettingRowScope, change: SettingChange): Promise<void> {
    if (this.mutationBlock(scope) !== null) return;
    const current = this.row(key, scope);
    if (["checking", "confirm", "applying", "uncertain"].includes(current.kind)) return;
    const state = this.sectionFor(scope);
    const snapshot = state?.kind === "ready" ? state.data : null;
    const before = snapshot?.settings.find((setting) => setting.key === key);
    if (!snapshot || !before) return;
    const draft = change.kind === "set" ? change.value : null;
    await this.prepare(key, scope, snapshot.snapshotId, change, draft, before, true);
  }

  /**
   * Sends a sensitive row's reviewed change. A dialog opened before the
   * view went stale or read-only sends nothing until changes resume.
   */
  async confirm(key: SettingKeyId, scope: SettingRowScope): Promise<void> {
    const current = this.row(key, scope);
    if (current.kind !== "confirm") return;
    if (this.mutationBlock(scope) !== null) return;
    await this.apply(key, scope, current.review.reviewId, current.review.after);
  }

  /** Resends an uncertain change: same review ID, same body, same draft. */
  async retry(key: SettingKeyId, scope: SettingRowScope): Promise<void> {
    const current = this.row(key, scope);
    if (current.kind !== "uncertain") return;
    if (this.mutationBlock(scope) !== null) return;
    await this.apply(key, scope, current.reviewId, current.draft);
  }

  /** "Apply my value again": a new prepare against the reloaded values. */
  async applyAgain(key: SettingKeyId, scope: SettingRowScope): Promise<void> {
    const current = this.row(key, scope);
    if (current.kind !== "changed_elsewhere") return;
    const change = changeFor(current.draft);
    if (!change) return;
    this.setRow(rowKey(key, scope), { kind: "idle" });
    await this.change(key, scope, change);
  }

  /** Reloads the section a row belongs to, or every section. */
  async showCurrent(scope: SettingRowScope | null = null): Promise<void> {
    if (scope === null) {
      await this.reload();
      return;
    }
    await this.loadSection(scope);
  }

  private async prepare(
    key: SettingKeyId,
    scope: SettingRowScope,
    snapshotId: string,
    change: SettingChange,
    draft: SettingValue | null,
    before: ResolvedSetting,
    reloadIfExpired: boolean,
  ): Promise<void> {
    const { planeId, generation } = this;
    const row = rowKey(key, scope);
    this.setRow(row, { kind: "checking", draft });
    let preparation;
    try {
      preparation = await prepareSettingChange(planeId, snapshotId, key, change);
    } catch (thrown: unknown) {
      if (generation !== this.generation) return;
      const error = this.noted(thrown);
      if (error.code === "settings.snapshot_expired" && reloadIfExpired) {
        // Reload once; re-prepare only if the value is still the one shown.
        await this.loadSection(scope);
        if (generation !== this.generation) return;
        const state = this.sectionFor(scope);
        const reloaded = state?.kind === "ready" ? state.data : null;
        const current = reloaded?.settings.find((setting) => setting.key === key);
        if (reloaded && current && sameSetting(current, before)) {
          await this.prepare(key, scope, reloaded.snapshotId, change, draft, before, false);
        } else if (current) {
          this.setRow(row, { kind: "changed_elsewhere", current, draft });
        } else {
          this.setRow(row, { kind: "refused", error });
        }
        return;
      }
      this.setRow(row, { kind: "refused", error });
      return;
    }
    if (generation !== this.generation) return;
    if (preparation.kind === "changed") {
      this.setRow(row, { kind: "changed_elsewhere", current: preparation.current, draft });
      void this.loadSection(scope);
      return;
    }
    if (isSensitive(key)) {
      this.setRow(row, { kind: "confirm", review: preparation.review });
      return;
    }
    await this.apply(key, scope, preparation.review.reviewId, draft);
  }

  private async apply(
    key: SettingKeyId,
    scope: SettingRowScope,
    reviewId: string,
    draft: SettingValue | null,
  ): Promise<void> {
    const { planeId } = this;
    const row = rowKey(key, scope);
    this.setRow(row, { kind: "applying", reviewId, draft });
    let receipt;
    try {
      receipt = await applySettingsChange(planeId, reviewId);
    } catch (thrown: unknown) {
      const uncertain: SettingRowState = { kind: "uncertain", reviewId, draft, error: this.noted(thrown) };
      if (this.owns(planeId, row, reviewId)) this.setRow(row, uncertain);
      // The user switched Planes; keep the uncertainty for when they return.
      else if (!this.disposed && planeId !== this.planeId) this.retained.set(`${planeId}#${row}`, uncertain);
      return;
    }
    if (!this.owns(planeId, row, reviewId)) {
      // Answered while its Plane isn't shown: nothing is left to retry.
      const held = this.retained.get(`${planeId}#${row}`);
      if (held && (held.kind === "applying" || held.kind === "uncertain") && held.reviewId === reviewId) {
        this.retained.delete(`${planeId}#${row}`);
      }
      return;
    }
    switch (receipt.kind) {
      case "applied":
        this.setRow(row, { kind: "idle" });
        // The reloaded cursor covers the event this change produced, so it
        // is never reported as a change from elsewhere.
        await this.loadSection(scope);
        return;
      case "refused":
        this.system.observe(receipt.error);
        this.setRow(row, { kind: "refused", error: receipt.error });
        if (receipt.error.code === "security.audit_degraded" || receipt.error.code === "recovery.read_only") {
          void this.loadPlane();
        }
        return;
      case "changed":
        this.setRow(row, { kind: "changed_elsewhere", current: receipt.current, draft });
        await this.loadSection(scope);
        return;
    }
  }

  private async loadSection(scope: SettingRowScope): Promise<void> {
    if (scope.type === "plane") await this.loadPlane();
    else if (scope.projectId === this.projectId) await this.loadProject();
  }

  private setRow(key: string, state: SettingRowState): void {
    if (state.kind === "idle") {
      if (!(key in this.rows)) return;
      const { [key]: _removed, ...rest } = this.rows;
      this.rows = rest;
    } else {
      this.rows = { ...this.rows, [key]: state };
    }
  }

  /** Whether `row` of the Plane on screen still shows this review being sent. */
  private owns(planeId: PlaneId, row: string, reviewId: string): boolean {
    if (this.disposed || planeId !== this.planeId) return false;
    const current = this.rows[row];
    return current?.kind === "applying" && current.reviewId === reviewId;
  }

  private retainPending(): void {
    for (const [key, state] of Object.entries(this.rows)) {
      if (state.kind === "applying" || state.kind === "uncertain") this.retained.set(`${this.planeId}#${key}`, state);
    }
  }

  private restorePending(planeId: PlaneId): Record<string, SettingRowState> {
    const rows: Record<string, SettingRowState> = {};
    for (const [key, state] of this.retained) {
      const separator = key.indexOf("#");
      if (key.slice(0, separator) !== planeId) continue;
      rows[key.slice(separator + 1)] = state;
      this.retained.delete(key);
    }
    return rows;
  }

  // -------------------------------------------------------------------------
  // Change watcher
  // -------------------------------------------------------------------------

  private cursor(loader: SettingsLoader): string | null {
    switch (loader) {
      case "plane":
        return this.plane.kind === "ready" ? this.plane.data.cursor : null;
      case "project":
        return this.project?.kind === "ready" ? this.project.data.cursor : null;
      case "work":
        return this.work.kind === "ready" ? this.work.data.projectsCursor : null;
    }
  }

  private loading(loader: SettingsLoader): boolean {
    switch (loader) {
      case "plane":
        return this.plane.kind === "loading";
      case "project":
        return this.project?.kind === "loading";
      case "work":
        return this.work.kind === "loading";
    }
  }

  /** Keeps an event for a section that is reloading. */
  private remember(loader: SettingsLoader, change: Extract<SettingsChange, { type: "change" }>): void {
    const missed = this.missed[loader];
    if (missed.changes.length >= MAX_MISSED_EVENTS) {
      missed.overflow = true;
      return;
    }
    missed.changes.push(change);
  }

  /** Replays what arrived during a reload; events the new cursor covers are ignored. */
  private replayMissed(loader: SettingsLoader): void {
    const { changes, overflow } = this.missed[loader];
    this.missed[loader] = { changes: [], overflow: false };
    if (overflow) {
      if (loader === "plane") this.plane = withFreshness(this.plane, "changed");
      else if (loader === "work") this.work = withFreshness(this.work, "changed");
      else if (this.project) this.project = withFreshness(this.project, "changed");
    }
    for (const change of changes) void this.receive(change);
  }

  private startWatch(): void {
    // The Agents view counts once its pane has loaded it: account events
    // after its cursor must still arrive.
    const agentsCursor = this.agents.view.kind === "ready" ? this.agents.view.data.cursor : null;
    const cursors = [...(["plane", "project", "work"] as const).map((loader) => this.cursor(loader)), agentsCursor].filter(
      (cursor): cursor is string => cursor !== null,
    );
    if (cursors.length === 0) {
      this.watch = "idle";
      return;
    }
    const after = cursors.reduce((lowest, cursor) => (BigInt(cursor) < BigInt(lowest) ? cursor : lowest));
    const generation = ++this.watchGeneration;
    this.watch = "live";
    this.watchError = null;
    watchSettingsChanges(this.planeId, after, (change) => {
      if (generation !== this.watchGeneration) return;
      void this.receive(change);
    }).catch((thrown: unknown) => {
      if (generation !== this.watchGeneration) return;
      this.lost("failed", publicError(thrown));
    });
  }

  private lost(state: "stale" | "failed", error: PublicError): void {
    this.watch = state;
    this.watchError = error;
    this.plane = withFreshness(this.plane, "stale");
    this.work = withFreshness(this.work, "stale");
    if (this.project) this.project = withFreshness(this.project, "stale");
    this.agents.markStale();
    this.extensions.markStale();
    this.system.markStale();
    this.autodelete.markStale();
  }

  /** Handles one watcher message. Exposed for the session tests. */
  async receive(change: SettingsChange): Promise<void> {
    switch (change.type) {
      case "resumed":
        if (this.watch === "stale") {
          this.watch = "live";
          this.watchError = null;
          await this.reload();
        }
        return;
      case "reconnecting":
        this.lost("stale", change.error);
        return;
      case "failed":
        this.lost("failed", change.error);
        return;
      case "change":
        break;
    }
    switch (change.kind) {
      case "setting.changed":
      case "setting.cleared":
        if (change.settingScope === "plane") {
          void this.system.settingChanged(change.settingKey);
          void this.autodelete.settingChanged(change.settingKey);
        }
        await this.settingChanged(change);
        return;
      case "account.bound":
      case "account.unbound":
        this.agents.noteChange(change.kind, change.sequence);
        this.workChanged(change);
        return;
      case "project.registered":
      case "project.removed":
        this.workChanged(change);
        return;
      case "usage.recorded":
        this.usageNewer = true;
        return;
      case "audit.epoch_begun":
        // A new audit epoch may clear the degraded banner.
        void this.loadPlane();
        void this.system.reloadIfLoaded();
        return;
      case "schedule.created":
      case "schedule.canceled":
      case "schedule.fired":
        this.schedulesChanged = true;
        return;
      case "auto_continue.changed":
      case "auto_continue.configured":
        this.agents.noteChange(change.kind, change.sequence);
        return;
    }
  }

  /** A failure on this Plane, recorded for the diagnostic summary. */
  private noted(thrown: unknown): PublicError {
    const error = publicError(thrown);
    this.system.observe(error);
    return error;
  }

  private workChanged(change: Extract<SettingsChange, { type: "change" }>): void {
    if (this.loading("work")) {
      this.remember("work", change);
      return;
    }
    if (newer(change.sequence, this.cursor("work"))) this.work = withFreshness(this.work, "changed");
  }

  private async settingChanged(change: Extract<SettingsChange, { type: "change" }>): Promise<void> {
    const { sequence, settingKey: key, settingScope, projectId } = change;
    let scope: SettingRowScope;
    let loader: SettingsLoader;
    if (settingScope === "plane") {
      scope = { type: "plane" };
      loader = "plane";
    } else if (settingScope === "project" && projectId !== null && projectId === this.projectId) {
      scope = { type: "project", projectId };
      loader = "project";
    } else {
      // Conversation scope and other Projects are not shown here.
      return;
    }
    if (this.loading(loader)) {
      this.remember(loader, change);
      return;
    }
    if (!newer(sequence, this.cursor(loader))) return;
    const current = key ? this.rows[rowKey(key, scope)] : undefined;
    if (key && current && (current.kind === "applying" || current.kind === "uncertain")) {
      // Most likely this row's own change: reload, never "changed elsewhere".
      await this.loadSection(scope);
      return;
    }
    if (key && current && (current.kind === "editing" || current.kind === "checking" || current.kind === "confirm")) {
      const before = this.setting(key, scope);
      const generation = this.generation;
      await this.loadSection(scope);
      if (generation !== this.generation) return;
      const after = this.setting(key, scope);
      const row = this.rows[rowKey(key, scope)];
      if (
        before &&
        after &&
        !sameSetting(before, after) &&
        (row?.kind === "editing" || row?.kind === "checking" || row?.kind === "confirm")
      ) {
        this.setRow(rowKey(key, scope), { kind: "changed_elsewhere", current: after, draft: draftOf(row) });
      }
      return;
    }
    if (loader === "plane") this.plane = withFreshness(this.plane, "changed");
    else if (this.project) this.project = withFreshness(this.project, "changed");
  }
}
