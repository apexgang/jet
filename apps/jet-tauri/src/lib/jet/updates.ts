import { Channel, invoke } from "@tauri-apps/api/core";

import type { PublicError } from "./bridge";

/**
 * Signed app updates (Wave 4 §B). The shell checks, downloads, verifies and
 * installs natively; the webview has no updater permission. A check contacts
 * github.com.
 */
export type AppUpdateDisabledReason = "homebrew" | "development_build" | "unsupported_install";

export type AppUpdateState =
  | { kind: "disabled"; reason: AppUpdateDisabledReason }
  /** `upToDate`: a check this session found nothing newer. */
  | { kind: "idle"; upToDate: boolean }
  | { kind: "checking" }
  | { kind: "available"; version: string; dateUnixMs: string | null }
  | { kind: "downloading"; version: string; downloaded: number; total: number | null }
  /** Installed; takes effect after a restart. */
  | { kind: "ready"; version: string }
  | { kind: "failed"; error: PublicError };

export type AppUpdate = { currentVersion: string; state: AppUpdateState };

export const loadAppUpdate = () => invoke<AppUpdate>("load_app_update");

/**
 * Follows the update state for this window: one watcher per window, replaced
 * by the next call and stopped natively when the window closes. Resolves with
 * the state now; later states arrive through `on`, including checks and
 * installs this window did not start and their download progress.
 */
export async function watchAppUpdate(on: (update: AppUpdate) => void): Promise<AppUpdate> {
  const onChange = new Channel<AppUpdate>();
  onChange.onmessage = on;
  return invoke<AppUpdate>("watch_app_update", { onChange });
}

export const checkAppUpdate = () => invoke<AppUpdate>("check_app_update");

/** Downloads and installs the announced update; its progress reaches every window's watcher. */
export const installAppUpdate = () => invoke<AppUpdate>("install_app_update");

/** Restarts Jet into the installed update. Confirm with the user first. */
export const restartAfterUpdate = () => invoke<void>("restart_after_update");
