import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import { LOCAL_PLANE, type PlaneId } from "$lib/jet/planes";
import {
  changeAutodeleteRule,
  loadAutodeleteRules,
  resolveConversationNames,
  type AutodeleteChange,
  type AutodeleteRule,
  type AutodeleteRules,
} from "$lib/jet/retention";
import { sectionData, sectionStateFor, withFreshness, type SectionState } from "$lib/features/settings/model";
import { NAMES_PER_CALL } from "$lib/features/trash/model";
import {
  NEW_RULE,
  POLL_DELAY_MS,
  POLL_LIMIT,
  parseDays,
  pendingBySlot,
  promptFits,
  refusalReloads,
  type RuleSlot,
} from "./model";

/** Why rule changes can't start right now; `null` when they can. */
export type RulesBlock = "read_only" | "stale" | null;

/** The one inline edit open at a time (wave 3.3 §7.2 `RuleEditor`). */
export type RuleEditor =
  | { kind: "idle" }
  | { kind: "wording"; ruleId: string; prompt: string }
  | { kind: "days"; ruleId: string; days: string }
  | { kind: "sending"; slot: RuleSlot; change: AutodeleteChange }
  | { kind: "error"; slot: RuleSlot; error: PublicError };

/** A confirmation for a consequential change. Tokens come from the native read. */
export type RulesDialog =
  | { kind: "closed" }
  | { kind: "approve"; ruleId: string; token: string; days: number }
  | { kind: "everywhere"; ruleId: string; token: string }
  | { kind: "delete"; ruleId: string }
  /** The rule changed while its Approve dialog was open. */
  | { kind: "changed"; ruleId: string };

/** Setting keys whose value the rules view discloses. */
const DISCLOSED_KEYS: ReadonlySet<string> = new Set(["utility.autodelete_compilation", "utility.account_binding"]);

export type AutodeleteOptions = {
  /** Whether rule changes may start (read-only Recovery, a stale watcher). */
  mutationBlock?: () => RulesBlock;
  /** A failure on this Plane, for the diagnostic summary. */
  observe?: (error: PublicError) => void;
};

/**
 * Settings › Work › Retention › Auto-delete for one Plane. It follows the
 * Settings Plane picker. Every completion is checked against the Plane and
 * the request it started for; a new Jet service start drops what was read.
 */
export class AutodeleteSession {
  planeId = $state<PlaneId>(LOCAL_PLANE);
  rules = $state<SectionState<AutodeleteRules>>({ kind: "loading", last: null });
  /** The wording of a new rule. */
  compose = $state("");
  editor = $state<RuleEditor>({ kind: "idle" });
  dialog = $state<RulesDialog>({ kind: "closed" });
  /** Changes Jet sent but couldn't confirm, by rule slot. */
  pending = $state<Record<RuleSlot, AutodeleteChange>>({});
  /** Task names for candidates; null when the Plane couldn't name one. */
  names = $state<Record<string, string | null>>({});
  /** Five reads found a rule still compiling; the view asks for a refresh. */
  stillPreparing = $state(false);
  /** Counts Plane selections, so a shown pane loads again after each one. */
  selection = $state(0);

  private started = false;
  private loaded = false;
  private disposed = false;
  private generation = 0;
  private request = 0;
  private polls = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  /** Rules whose candidate names were asked for, this generation. */
  private named = new Set<string>();
  private readonly options: AutodeleteOptions;

  constructor(options: AutodeleteOptions = {}) {
    this.options = options;
  }

  /** Shows one Plane. Nothing is read until the Work pane asks. */
  select(planeId: PlaneId): void {
    if (this.started && planeId === this.planeId) return;
    this.started = true;
    this.planeId = planeId;
    this.forget();
    this.selection++;
  }

  dispose(): void {
    this.disposed = true;
    this.generation++;
    this.stopPolling();
  }

  /**
   * The Plane's Jet service started again, possibly from an older store:
   * tokens, names and anything shown were read before it.
   */
  planeRestarted(): void {
    const loaded = this.loaded;
    this.forget();
    if (loaded) {
      this.loaded = true;
      void this.load();
    }
  }

  /** The Settings change watcher is reconnecting or stopped. */
  markStale(): void {
    this.rules = withFreshness(this.rules, "stale");
    this.stopPolling();
  }

  async ensureLoaded(): Promise<void> {
    if (this.loaded) return;
    this.loaded = true;
    await this.load();
  }

  async reloadIfLoaded(): Promise<void> {
    if (this.loaded) await this.load();
  }

  /** A Plane-scope Setting changed; the drafting disclosure may be out of date. */
  async settingChanged(key: string | null): Promise<void> {
    if (key === null || DISCLOSED_KEYS.has(key)) await this.reloadIfLoaded();
  }

  /** Refresh: reads again and restarts the wait for a compiling draft. */
  async refresh(): Promise<void> {
    this.polls = 0;
    this.stillPreparing = false;
    this.loaded = true;
    await this.load();
  }

  async load(): Promise<void> {
    const { planeId, generation } = this;
    const request = ++this.request;
    this.stopPolling();
    const last = sectionData(this.rules);
    // A background read (a poll, a refresh) keeps the shown rules usable.
    if (this.rules.kind !== "ready") this.rules = { kind: "loading", last };
    try {
      const view = await loadAutodeleteRules(planeId);
      if (!this.current(generation) || request !== this.request) return;
      this.rules = { kind: "ready", data: view, freshness: "live", issues: [] };
      // The shell holds the unconfirmed changes; it is authoritative.
      this.pending = pendingBySlot(view.pending);
      this.checkDialog(view);
      this.named = new Set([...this.named].filter((ruleId) => view.rules.some((rule) => rule.ruleId === ruleId)));
      this.schedulePoll(view);
    } catch (thrown: unknown) {
      if (!this.current(generation) || request !== this.request) return;
      const error = this.noted(thrown);
      this.rules = sectionStateFor(error, last);
    }
  }

  /** The rule as last read, or null. */
  rule(ruleId: string): AutodeleteRule | null {
    return sectionData(this.rules)?.rules.find((rule) => rule.ruleId === ruleId) ?? null;
  }

  /** Why changes can't start now, from the Plane state and this view. */
  get block(): RulesBlock {
    const outer = this.options.mutationBlock?.() ?? null;
    if (outer !== null) return outer;
    return this.rules.kind === "ready" && this.rules.freshness === "live" ? null : "stale";
  }

  /** Whether a slot may start a change: nothing sending and nothing unconfirmed there. */
  canChange(slot: RuleSlot): boolean {
    if (this.block !== null || slot in this.pending) return false;
    return this.editor.kind !== "sending";
  }

  // -------------------------------------------------------------------------
  // Changes
  // -------------------------------------------------------------------------

  /** "Create draft" for the wording in the Rule field. */
  async createDraft(): Promise<void> {
    const prompt = this.compose;
    if (!promptFits(prompt) || !this.canChange(NEW_RULE)) return;
    const done = await this.send(NEW_RULE, { kind: "compile", rule_id: null, prompt });
    if (done) this.compose = "";
  }

  editWording(ruleId: string): void {
    const rule = this.rule(ruleId);
    if (!rule || !this.canChange(ruleId)) return;
    this.editor = { kind: "wording", ruleId, prompt: rule.prompt };
  }

  editDays(ruleId: string): void {
    const rule = this.rule(ruleId);
    if (!rule || !this.canChange(ruleId)) return;
    const days = rule.state.kind === "draft" || rule.state.kind === "approved" ? String(rule.state.inactiveDays) : "";
    this.editor = { kind: "days", ruleId, days };
  }

  setEditorText(text: string): void {
    if (this.editor.kind === "wording") this.editor = { ...this.editor, prompt: text };
    else if (this.editor.kind === "days") this.editor = { ...this.editor, days: text };
  }

  cancelEdit(): void {
    if (this.editor.kind === "sending") return;
    this.editor = { kind: "idle" };
  }

  /** Saves the open wording or days edit. */
  async saveEdit(): Promise<void> {
    const editor = this.editor;
    if (editor.kind === "wording") {
      if (!promptFits(editor.prompt)) return;
      await this.send(editor.ruleId, { kind: "compile", rule_id: editor.ruleId, prompt: editor.prompt });
    } else if (editor.kind === "days") {
      const days = parseDays(editor.days);
      if (days === null) return;
      await this.send(editor.ruleId, { kind: "set_inactive_days", rule_id: editor.ruleId, inactive_days: days });
    }
  }

  openApprove(ruleId: string): void {
    const rule = this.rule(ruleId);
    if (rule?.state.kind !== "draft" || !this.canChange(ruleId)) return;
    this.dialog = { kind: "approve", ruleId, token: rule.state.approveToken, days: rule.state.inactiveDays };
  }

  openEverywhere(ruleId: string): void {
    const rule = this.rule(ruleId);
    if (rule?.state.kind !== "approved" || rule.state.everywhereToken === null || !this.canChange(ruleId)) return;
    this.dialog = { kind: "everywhere", ruleId, token: rule.state.everywhereToken };
  }

  openDelete(ruleId: string): void {
    if (!this.rule(ruleId) || !this.canChange(ruleId)) return;
    this.dialog = { kind: "delete", ruleId };
  }

  closeDialog(): void {
    if (this.editor.kind === "sending") return;
    this.dialog = { kind: "closed" };
  }

  /** Confirms the open dialog with the token it was opened with. */
  async confirmDialog(): Promise<void> {
    const dialog = this.dialog;
    let change: AutodeleteChange;
    switch (dialog.kind) {
      case "approve":
        change = { kind: "approve", token_id: dialog.token };
        break;
      case "everywhere":
        change = { kind: "authorize_everywhere", token_id: dialog.token };
        break;
      case "delete":
        change = { kind: "delete", rule_id: dialog.ruleId };
        break;
      default:
        return;
    }
    if (!this.canChange(dialog.ruleId)) return;
    await this.send(dialog.ruleId, change);
    if (this.dialog === dialog) this.dialog = { kind: "closed" };
  }

  /** "Try again": resends the unconfirmed change of one slot unchanged. */
  async retry(slot: RuleSlot): Promise<void> {
    const change = this.pending[slot];
    if (!change || this.editor.kind === "sending") return;
    if (this.options.mutationBlock?.() != null) return;
    await this.send(slot, change, true);
  }

  dismissError(): void {
    if (this.editor.kind === "error") this.editor = { kind: "idle" };
  }

  /**
   * Sends one change. Returns whether the Plane recorded it. A rejection
   * leaves it unconfirmed for "Try again"; a refusal is shown inline.
   */
  private async send(slot: RuleSlot, change: AutodeleteChange, retrying = false): Promise<boolean> {
    const { planeId, generation } = this;
    if (!retrying && !this.canChange(slot)) return false;
    this.editor = { kind: "sending", slot, change };
    let outcome;
    try {
      outcome = await changeAutodeleteRule(planeId, change);
    } catch (thrown: unknown) {
      this.noted(thrown);
      if (!this.current(generation)) return false;
      this.pending = { ...this.pending, [slot]: change };
      this.editor = { kind: "idle" };
      return false;
    }
    if (!this.current(generation)) return false;
    this.clearPending(slot);
    switch (outcome.kind) {
      case "recorded":
      case "deleted":
        this.editor = { kind: "idle" };
        // Its dialog is done; the reload must not report the change as elsewhere.
        if (this.dialog.kind !== "closed" && this.dialog.ruleId === slot) this.dialog = { kind: "closed" };
        this.polls = 0;
        this.stillPreparing = false;
        await this.load();
        return true;
      case "refused":
        this.options.observe?.(outcome.error);
        this.editor = { kind: "error", slot, error: outcome.error };
        if (refusalReloads(outcome.error)) void this.load();
        return false;
    }
  }

  private clearPending(slot: RuleSlot): void {
    if (!(slot in this.pending)) return;
    const { [slot]: _removed, ...rest } = this.pending;
    this.pending = rest;
  }

  // -------------------------------------------------------------------------
  // Candidate names
  // -------------------------------------------------------------------------

  /**
   * Names the candidates of one rule, 32 per native call on one connection.
   * The Settings window has no task list, so every name is looked up.
   */
  async resolveNames(ruleId: string): Promise<void> {
    const rule = this.rule(ruleId);
    if (!rule || this.named.has(ruleId)) return;
    this.named.add(ruleId);
    const { planeId, generation } = this;
    const missing = [...new Set(rule.candidates.map((candidate) => candidate.conversationId))].filter(
      (id) => !(id in this.names),
    );
    for (let start = 0; start < missing.length; start += NAMES_PER_CALL) {
      const chunk = missing.slice(start, start + NAMES_PER_CALL);
      try {
        const names = await resolveConversationNames(planeId, chunk);
        if (!this.current(generation)) return;
        const next = { ...this.names };
        for (const name of names) if (chunk.includes(name.conversationId)) next[name.conversationId] = name.title;
        this.names = next;
      } catch (thrown: unknown) {
        if (!this.current(generation)) return;
        this.noted(thrown);
        // Asked again the next time the list opens.
        this.named.delete(ruleId);
        return;
      }
    }
  }

  // -------------------------------------------------------------------------
  // Internals
  // -------------------------------------------------------------------------

  private current(generation: number): boolean {
    return !this.disposed && generation === this.generation;
  }

  private noted(thrown: unknown): PublicError {
    const error = publicError(thrown);
    this.options.observe?.(error);
    return error;
  }

  /** Drops everything read for the Plane. */
  private forget(): void {
    this.generation++;
    this.loaded = false;
    this.stopPolling();
    this.polls = 0;
    this.stillPreparing = false;
    this.rules = { kind: "loading", last: null };
    this.editor = { kind: "idle" };
    this.dialog = { kind: "closed" };
    this.pending = {};
    this.names = {};
    this.named = new Set();
  }

  /** An open Approve dialog stays only while its rule reads the same. */
  private checkDialog(view: AutodeleteRules): void {
    const dialog = this.dialog;
    if (dialog.kind === "closed" || dialog.kind === "changed") return;
    const rule = view.rules.find((candidate) => candidate.ruleId === dialog.ruleId);
    if (!rule) {
      this.dialog = { kind: "closed" };
      return;
    }
    if (dialog.kind === "approve" && (rule.state.kind !== "draft" || rule.state.approveToken !== dialog.token)) {
      this.dialog = { kind: "changed", ruleId: dialog.ruleId };
    } else if (
      dialog.kind === "everywhere" &&
      (rule.state.kind !== "approved" || rule.state.everywhereToken !== dialog.token)
    ) {
      this.dialog = { kind: "changed", ruleId: dialog.ruleId };
    }
  }

  /** While a rule is compiling, reads again every 2 s, five times at most. */
  private schedulePoll(view: AutodeleteRules): void {
    if (!view.rules.some((rule) => rule.state.kind === "compiling")) {
      this.polls = 0;
      this.stillPreparing = false;
      return;
    }
    if (this.polls >= POLL_LIMIT) {
      this.stillPreparing = true;
      return;
    }
    const generation = this.generation;
    this.timer = setTimeout(() => {
      this.timer = null;
      if (!this.current(generation)) return;
      this.polls++;
      void this.load();
    }, POLL_DELAY_MS);
  }

  private stopPolling(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }
}
