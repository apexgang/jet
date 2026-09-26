import { publicError } from "$lib/jet/errors";
import { loadSchedules, createSchedule, cancelSchedule, type Schedule, type ScheduleRequest } from "$lib/jet/schedules";
import type { PlaneId } from "$lib/jet/planes";

type Scope = { planeId: PlaneId; conversationId: string };
export class SchedulesSession {
  scope = $state<Scope | null>(null);
  tasks = $state<Schedule[]>([]);
  loading = $state(false);
  busy = $state(false);
  notice = $state<string | null>(null);
  error = $state<string | null>(null);
  localTime = $state("09:00");
  timeZone = $state(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
  prompt = $state("");
  private generation = 0;
  private attempts = new Map<string, string>();

  get valid() { return this.prompt.trim().length > 0 && new TextEncoder().encode(this.prompt).length <= 8192 && /^\d{2}:\d{2}$/.test(this.localTime) && this.timeZone.trim().length > 0; }
  select(planeId: PlaneId, conversationId: string | null) {
    if (this.scope?.planeId === planeId && this.scope.conversationId === conversationId) return;
    this.generation += 1;
    this.scope = conversationId ? { planeId, conversationId } : null;
    this.tasks = []; this.error = null; this.notice = null;
    void this.refresh();
  }
  async refresh() {
    const scope = this.scope; const generation = this.generation;
    if (!scope) { this.loading = false; return; }
    this.loading = true; this.error = null;
    try {
      const tasks = await loadSchedules(scope.planeId, scope.conversationId);
      if (generation === this.generation) this.tasks = tasks;
    } catch (error) { if (generation === this.generation) this.error = publicError(error).message; }
    finally { if (generation === this.generation) this.loading = false; }
  }
  private attempt(key: string) {
    const existing = this.attempts.get(key);
    if (existing) return existing;
    const attempt = crypto.randomUUID(); this.attempts.set(key, attempt); return attempt;
  }
  private failed(error: unknown, key: string, generation: number) {
    const failure = publicError(error);
    // A conclusive refusal completes this attempt. Transport failures and
    // malformed replies can hide a committed command, so retain their identity.
    if (["invalid_input", "unauthorized", "conflict", "unavailable", "incompatible", "rate_limited", "not_found"].includes(failure.category)) {
      this.attempts.delete(key);
    }
    if (generation === this.generation) this.error = failure.message;
  }
  async create(): Promise<boolean> {
    const scope = this.scope; const generation = this.generation;
    if (!scope || !this.valid || this.busy) return false;
    const input = { conversationId: scope.conversationId, timeZone: this.timeZone.trim(), localTime: `${this.localTime}:00`, prompt: this.prompt };
    const key = JSON.stringify([scope.planeId, "create", input]);
    const request: ScheduleRequest = { ...input, attempt: this.attempt(key) };
    this.busy = true; this.error = null;
    try {
      await createSchedule(scope.planeId, request);
      this.attempts.delete(key);
      if (generation === this.generation) {
        if (this.prompt === input.prompt) this.prompt = "";
        this.notice = "Daily schedule created.";
        await this.refresh();
      }
      return true;
    } catch (error) { this.failed(error, key, generation); return false; }
    finally { this.busy = false; }
  }
  async cancel(task: Schedule): Promise<boolean> {
    const scope = this.scope; const generation = this.generation;
    if (!scope || task.conversationId !== scope.conversationId || this.busy) return false;
    const key = `${scope.planeId}:cancel:${task.id}`;
    this.busy = true; this.error = null;
    try {
      await cancelSchedule(scope.planeId, task.id, this.attempt(key));
      this.attempts.delete(key);
      if (generation === this.generation) { this.notice = "Schedule canceled. Future and queued messages from it were removed."; await this.refresh(); }
      return true;
    } catch (error) { this.failed(error, key, generation); return false; }
    finally { this.busy = false; }
  }
}
