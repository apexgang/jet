import { Channel, invoke } from "@tauri-apps/api/core";

import type { PublicError } from "./bridge";

/**
 * The local Jet service on this computer (Wave 4 §A). The shell observes and
 * provisions it natively; the webview never names a path, version or command.
 */
export type LocalServicePhase =
  | "checking"
  | "installing"
  | "updating"
  | "starting"
  | "running"
  | "stopped"
  | "not_installed"
  | "failed";

/** Which installation manages the service (ADR-0026). */
export type LocalServiceChannel = "gui" | "homebrew" | "development";

export type LocalServiceManager = "systemd" | "autostart" | "brew_services";

/** What the last provisioning pass changed. */
export type LocalServiceAction = "installed" | "updated" | "started" | "rolled_back";

export type LocalServiceView = {
  /**
   * Increases with every view the shell publishes. A reply or push with a
   * lower revision than one already shown is older and is dropped.
   */
  revision: number;
  phase: LocalServicePhase;
  /** The owner's channel, or the one provisioning chose; null when unknown. */
  channel: LocalServiceChannel | null;
  manager: LocalServiceManager | null;
  /** The service version this app carries; null in development builds. */
  bundledVersion: string | null;
  /** The version this app's installation runs now. */
  currentVersion: string | null;
  /** The version a rollback goes back to. */
  previousVersion: string | null;
  /** The version the running daemon reports. */
  runningVersion: string | null;
  /** A repair could install or start the service. */
  canRepair: boolean;
  /** Managed by this app and an earlier version is kept. */
  canRollback: boolean;
  lastAction: LocalServiceAction | null;
  /** Stable `service.*` codes, such as `service.start_timeout`. */
  error: PublicError | null;
};

/** A native rollback review, usable once within 10 minutes. */
export type LocalServiceRollbackReview = {
  reviewId: string;
  currentVersion: string;
  previousVersion: string;
};

/** Whether `next` is at least as new as `shown`, which a window may replace with it. */
export function isCurrentView(next: { revision: number }, shown: { revision: number } | null): boolean {
  return shown === null || next.revision >= shown.revision;
}

/** Phases while the shell works on the service; nothing can be started twice. */
export function isProvisioning(phase: LocalServicePhase): boolean {
  return phase === "checking" || phase === "installing" || phase === "updating" || phase === "starting";
}

export const loadLocalService = () => invoke<LocalServiceView>("load_local_service");

/**
 * Follows the service for this window: one watcher per window, replaced by
 * the next call and stopped natively when the window closes. Resolves with
 * the view now; later views arrive through `on`.
 */
export async function watchLocalService(on: (view: LocalServiceView) => void): Promise<LocalServiceView> {
  const onChange = new Channel<LocalServiceView>();
  onChange.onmessage = on;
  return invoke<LocalServiceView>("watch_local_service", { onChange });
}

/** Runs the native decision table again: install, update or start. */
export const repairLocalService = () => invoke<LocalServiceView>("repair_local_service");

/** Reviews going back to the previous version (Settings window only). */
export const prepareLocalServiceRollback = () =>
  invoke<LocalServiceRollbackReview>("prepare_local_service_rollback");

/**
 * Sends a reviewed rollback at most once. Rejects with a `PublicError` when
 * nothing changed (for example `service.review_stale`).
 */
export const executeLocalServiceRollback = (reviewId: string) =>
  invoke<LocalServiceView>("execute_local_service_rollback", { reviewId });
