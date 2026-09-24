import type { PublicError } from "$lib/jet/bridge";
import { publicError } from "$lib/jet/errors";
import { loadDesktopPreferences, setDesktopPreferences, type DesktopPreferences } from "$lib/jet/preferences";
import {
  checkAppUpdate,
  installAppUpdate,
  restartAfterUpdate,
  watchAppUpdate,
  type AppUpdate,
} from "$lib/jet/updates";

/** "Check for updates automatically", stored with this computer's desktop preferences. */
export type AutomaticCheck =
  | { kind: "loading" }
  | { kind: "ready"; preferences: DesktopPreferences; saving: boolean; notice: string | null }
  | { kind: "failed"; error: PublicError };

/**
 * Settings › Versions › App updates (Wave 4 §B). Checking, installing and
 * restarting are native; this keeps the last state the shell reported and
 * asks before restarting. It owns the window's native watcher: `start`
 * registers it once, the shell stops it when the window closes, and
 * `dispose` drops every later completion.
 */
export class AppUpdateSession {
  update = $state<AppUpdate | null>(null);
  /** Reading or changing the update state failed. */
  error = $state<PublicError | null>(null);
  busy = $state<"checking" | "installing" | "restarting" | null>(null);
  /** The restart confirmation is open. */
  confirmingRestart = $state(false);
  automatic = $state<AutomaticCheck>({ kind: "loading" });

  private started = false;
  private disposed = false;
  /** Set once a pushed state arrived, so an older initial answer never replaces it. */
  private pushed = false;

  /** Registers this window's watcher and reads the preference; later calls do nothing. */
  async start(): Promise<void> {
    if (this.started) return;
    this.started = true;
    await Promise.all([this.watch(), this.loadAutomatic()]);
  }

  dispose(): void {
    this.disposed = true;
  }

  /** Follows the native update state; a second call replaces this window's watcher. */
  async watch(): Promise<void> {
    this.pushed = false;
    try {
      const initial = await watchAppUpdate((update) => {
        if (this.disposed) return;
        this.pushed = true;
        this.show(update);
      });
      if (!this.disposed && !this.pushed) this.show(initial);
    } catch (error: unknown) {
      if (!this.disposed) this.error = publicError(error);
    }
  }

  async check(): Promise<void> {
    if (this.busy !== null) return;
    this.busy = "checking";
    if (this.update) this.update = { ...this.update, state: { kind: "checking" } };
    try {
      const update = await checkAppUpdate();
      if (!this.disposed) this.show(update);
    } catch (error: unknown) {
      if (!this.disposed) this.error = publicError(error);
    } finally {
      this.busy = null;
    }
  }

  /** Downloads and installs the announced update; the watcher shows its progress. */
  async install(): Promise<void> {
    if (this.busy !== null || this.update?.state.kind !== "available") return;
    this.busy = "installing";
    try {
      const update = await installAppUpdate();
      if (!this.disposed) this.show(update);
    } catch (error: unknown) {
      if (!this.disposed) this.error = publicError(error);
    } finally {
      this.busy = null;
    }
  }

  requestRestart(): void {
    if (this.update?.state.kind === "ready") this.confirmingRestart = true;
  }

  cancelRestart(): void {
    this.confirmingRestart = false;
  }

  /** Restarts Jet; every window closes. */
  async restart(): Promise<void> {
    if (!this.confirmingRestart || this.busy !== null) return;
    this.busy = "restarting";
    try {
      await restartAfterUpdate();
    } catch (error: unknown) {
      if (!this.disposed) {
        this.error = publicError(error);
        this.confirmingRestart = false;
      }
    } finally {
      this.busy = null;
    }
  }

  async loadAutomatic(): Promise<void> {
    try {
      const preferences = await loadDesktopPreferences();
      if (!this.disposed) this.automatic = { kind: "ready", preferences, saving: false, notice: null };
    } catch (error: unknown) {
      if (!this.disposed) this.automatic = { kind: "failed", error: publicError(error) };
    }
  }

  /**
   * Saves only this choice: the shell keeps the other desktop preferences as
   * stored, whichever copy this window read.
   */
  async setAutomatic(checkForUpdates: boolean): Promise<void> {
    if (this.automatic.kind !== "ready" || this.automatic.saving) return;
    const previous = this.automatic.preferences;
    this.automatic = { kind: "ready", preferences: { ...previous, checkForUpdates }, saving: true, notice: null };
    try {
      const saved = await setDesktopPreferences({ checkForUpdates });
      if (!this.disposed) {
        this.automatic = { kind: "ready", preferences: saved, saving: false, notice: "Saved on this computer." };
      }
    } catch (error: unknown) {
      if (!this.disposed) {
        this.automatic = { kind: "ready", preferences: previous, saving: false, notice: publicError(error).message };
      }
    }
  }

  private show(update: AppUpdate): void {
    this.update = update;
    this.error = null;
  }
}
