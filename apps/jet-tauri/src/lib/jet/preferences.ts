import { invoke } from "@tauri-apps/api/core";

/**
 * Device-local desktop preferences. They never change Plane policy.
 * `checkForUpdates`: one automatic update check after launch, which contacts
 * github.com.
 */
export type DesktopPreferences = { reopenLastTask: boolean; checkForUpdates: boolean };

/**
 * One or both preferences. The shell keeps the stored value of one left out,
 * so each pane sends only the choice it shows and can't undo another pane's.
 */
export type DesktopPreferencesChange =
  | { reopenLastTask: boolean; checkForUpdates?: boolean }
  | { reopenLastTask?: boolean; checkForUpdates: boolean };

export const loadDesktopPreferences = () => invoke<DesktopPreferences>("load_desktop_preferences");

/** Resolves with every preference as stored after the change. */
export const setDesktopPreferences = (preferences: DesktopPreferencesChange) =>
  invoke<DesktopPreferences>("set_desktop_preferences", { preferences });
