import { invoke } from "@tauri-apps/api/core";

/** Device-local desktop preferences. They never change Plane policy. */
export type DesktopPreferences = { reopenLastTask: boolean };

export const loadDesktopPreferences = () => invoke<DesktopPreferences>("load_desktop_preferences");

export const setDesktopPreferences = (preferences: DesktopPreferences) =>
  invoke<DesktopPreferences>("set_desktop_preferences", { preferences });
