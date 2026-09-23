import { invoke } from "@tauri-apps/api/core";

/**
 * Device-local notification routing (v2). `mutedPlanes` holds opaque Plane
 * handles (`"local"` or a registry UUID) whose signals stay silent here.
 */
export type NotificationPreferences = {
  enabled: boolean;
  approvals: boolean;
  completion: boolean;
  failure: boolean;
  mutedPlanes: string[];
};
/** A registered Plane a notification can come from. */
export type NotificationPlane = { planeId: string; label: string };
export type NotificationSettings = {
  preferences: NotificationPreferences;
  error: string | null;
  planes: NotificationPlane[];
};
export const loadNotificationSettings = () => invoke<NotificationSettings>("load_notification_settings");
export const setNotificationSettings = (preferences: NotificationPreferences) =>
  invoke<NotificationSettings>("set_notification_settings", { preferences });
