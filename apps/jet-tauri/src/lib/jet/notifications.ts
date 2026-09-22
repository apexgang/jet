import { invoke } from "@tauri-apps/api/core";
export type NotificationPreferences = { enabled: boolean; approvals: boolean; completion: boolean; failure: boolean };
export type NotificationSettings = { preferences: NotificationPreferences; error: string | null };
export const loadNotificationSettings = () => invoke<NotificationSettings>("load_notification_settings");
export const setNotificationSettings = (preferences: NotificationPreferences) => invoke<NotificationSettings>("set_notification_settings", { preferences });
