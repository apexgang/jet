import { Channel, invoke } from "@tauri-apps/api/core";

import type { PublicError } from "./bridge";
import type { PlaneId } from "./planes";

/** The 24 Setting spellings (`jet-protocol/src/setting.rs`). */
export type SettingKeyId =
  | "storage.disposable_mib"
  | "energy.concurrency"
  | "energy.low_power_concurrency"
  | "energy.constrained"
  | "energy.foreground_override"
  | "artifact.max_mib"
  | "artifact.run_mib"
  | "utility.account_binding"
  | "utility.content_consent"
  | "utility.autodelete_compilation"
  | "utility.git_text"
  | "utility.automatic_naming"
  | "git.auto_commit"
  | "git.auto_branch"
  | "git.auto_push"
  | "git.auto_draft_pull_request"
  | "git.branch_prefix"
  | "git.message_instructions"
  | "security.audit_retention_days"
  | "retention.trash_grace_days"
  | "craft.developer_mode"
  | "review.automatic"
  | "review.account_binding"
  | "review.cross_provider_consent";

/**
 * A Setting value as the shell shows it. `undisplayable` text is never
 * substituted: the row can only be cleared.
 */
export type SettingValue =
  | { type: "flag"; value: boolean }
  | { type: "text"; value: string }
  | { type: "count"; value: number }
  | { type: "undisplayable" };

/** A value the webview may send: never `undisplayable`. */
export type WritableSettingValue = Exclude<SettingValue, { type: "undisplayable" }>;

export type SettingSource =
  | { source: "built_in" }
  | { source: "plane" }
  | { source: "project"; projectId: string }
  | { source: "conversation"; conversationId: string };

export type ResolvedSetting = { key: SettingKeyId; value: SettingValue; source: SettingSource };

/** `unknown`: an older Jet service, or a store Recovery has not validated. */
export type PlaneState = {
  security: "trusted" | "degraded" | "unknown";
  recovery: "serving" | "read_only" | "unknown";
};

/** Where a snapshot was resolved. Input field names are snake_case. */
export type SettingsScope = { type: "plane" } | { type: "project"; project_id: string };

/** The scope as the shell reports it back (camelCase view). */
export type SettingsScopeView = { type: "plane" } | { type: "project"; projectId: string };

export type SettingsSnapshot = {
  snapshotId: string;
  cursor: string;
  scope: SettingsScopeView;
  planeState: PlaneState;
  settings: ResolvedSetting[];
};

export type SettingChange = { kind: "set"; value: WritableSettingValue } | { kind: "clear" };

export type SettingsReview = {
  reviewId: string;
  subject: { kind: "setting"; key: SettingKeyId; scope: SettingsScopeView };
  before: ResolvedSetting | null;
  /** `null` clears the scope's own value, so the inherited value applies. */
  after: SettingValue | null;
};

export type SettingChangePreparation =
  | { kind: "review"; review: SettingsReview }
  | { kind: "changed"; current: ResolvedSetting };

export type SettingsReceipt =
  | { kind: "applied"; detail: { kind: "setting"; key: SettingKeyId; value: SettingValue | null } }
  | { kind: "refused"; error: PublicError }
  | { kind: "changed"; current: ResolvedSetting };

export type WorkContext = {
  planeState: PlaneState;
  projects: Array<{ id: string; name: string }>;
  projectsCursor: string | null;
  bindings: Array<{ id: string; label: string; provider: string }>;
  autodelete: { count: number } | null;
  issues: Array<{ section: "projects" | "accounts" | "autodelete"; error: PublicError }>;
};

/** The staleness kinds the shell forwards (wave 3.2 §3.2). */
export type SettingsChangeKind =
  | "setting.changed"
  | "setting.cleared"
  | "account.bound"
  | "account.unbound"
  | "auto_continue.changed"
  | "auto_continue.configured"
  | "project.registered"
  | "project.removed"
  | "usage.recorded"
  | "audit.epoch_begun"
  | "schedule.created"
  | "schedule.canceled"
  | "schedule.fired";

export type SettingsChange =
  | { type: "resumed"; after: string }
  | {
      type: "change";
      sequence: string;
      kind: SettingsChangeKind;
      settingKey: SettingKeyId | null;
      settingScope: "plane" | "project" | "conversation" | null;
      projectId: string | null;
    }
  | { type: "reconnecting"; error: PublicError }
  | { type: "failed"; error: PublicError };

/** One connection: the Plane's status and every Setting its minor names. */
export const loadSettings = (planeId: PlaneId, scope: SettingsScope) =>
  invoke<SettingsSnapshot>("load_settings", { planeId, scope });

/**
 * Reads the key again and admits one exact change only if the Plane still
 * shows the value the snapshot did. The Plane is checked against the
 * snapshot's; a snapshot of another Plane is expired.
 */
export const prepareSettingChange = (
  planeId: PlaneId,
  snapshotId: string,
  key: SettingKeyId,
  change: SettingChange,
) => invoke<SettingChangePreparation>("prepare_setting_change", { planeId, snapshotId, key, change });

/** Sends the reviewed change. A thrown error is uncertain: retry the same ID. */
export const applySettingsChange = (planeId: PlaneId, reviewId: string) =>
  invoke<SettingsReceipt>("apply_settings_change", { planeId, reviewId });

export const loadWorkContext = (planeId: PlaneId) =>
  invoke<WorkContext>("load_work_context", { planeId });

/**
 * Watches one Plane's journal for kinds that make Settings stale. A new
 * watch replaces the previous one natively.
 */
export async function watchSettingsChanges(
  planeId: PlaneId,
  after: string,
  on: (change: SettingsChange) => void,
): Promise<void> {
  const onChange = new Channel<SettingsChange>();
  onChange.onmessage = on;
  await invoke<void>("watch_settings_changes", { planeId, after, onChange });
}
