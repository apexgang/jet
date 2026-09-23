import { Channel, invoke } from "@tauri-apps/api/core";

import type { PlaneId } from "./planes";

/** The five Settings panes, in design-language order. */
export type SettingsPane = "general" | "agents" | "work" | "connections" | "safety";

export type SettingsSection =
  | "appearance"
  | "notifications"
  | "restoration"
  | "harnesses"
  | "extensions"
  | "accounts"
  | "usage"
  | "utility"
  | "projects"
  | "delivery"
  | "reviews"
  | "schedules"
  | "retention"
  | "local_service"
  | "planes"
  | "execution"
  | "permissions"
  | "storage"
  | "recovery"
  | "diagnostics"
  | "audit"
  | "versions";

/**
 * A typed deep link. The shell validates it and drops a Plane it does not
 * know; it is never a URL. Field names are snake_case like other input enums.
 */
export type SettingsTarget = {
  pane: SettingsPane;
  section?: SettingsSection | null;
  plane_id?: PlaneId | null;
};

export type SettingsNavigation = { generation: string; target: SettingsTarget };

/** Opens or focuses the Settings window (main window only). */
export const openSettings = (target: SettingsTarget | null = null) =>
  invoke<void>("open_settings", { target });

/**
 * Registers the Settings window's navigation channel and returns the target
 * to show first: a pending deep link once, else the last pane.
 */
export async function watchSettingsNavigation(
  on: (navigation: SettingsNavigation) => void,
): Promise<SettingsNavigation> {
  const onNavigate = new Channel<SettingsNavigation>();
  onNavigate.onmessage = on;
  return invoke<SettingsNavigation>("watch_settings_navigation", { onNavigate });
}

/** Remembers the pane natively. The Plane is never persisted. */
export const rememberSettingsPane = (target: SettingsTarget) =>
  invoke<void>("remember_settings_pane", { target });

/** Closes the Settings window (Ctrl+W). */
export const closeSettings = () => invoke<void>("close_settings");
