import { invoke } from "@tauri-apps/api/core";

import type { PublicError } from "./bridge";
import type { PlaneHealth, PlaneId, ProtocolKnowledge } from "./planes";
import type { SettingsPane, SettingsSection } from "./settings-window";

/**
 * The compact health summary a feed's `connected` snapshot carries. It is the
 * Plane registry's health (security, store, Deletion ledger); no gauges.
 */
export type PlaneHealthSummary = PlaneHealth;

export type CollectResult = { removed: number };

/**
 * "Free disposable space": one bounded collection of unused temporary
 * Artifact files on a Plane. Tasks, Workspaces and snapshots are untouched.
 * Refused with `storage.collect_busy` while a pass runs on that Plane.
 */
export const collectDisposableStorage = (planeId: PlaneId) =>
  invoke<CollectResult>("collect_disposable_storage", { planeId });

/** A Settings section that can address a degraded condition. */
export type SettingsLink = { pane: SettingsPane; section: SettingsSection };

export type ExternalToolName = "git" | "git_lfs" | "ssh" | "tailscale";

export type DegradedKind =
  | "missing_external_tool"
  | "no_harness_available"
  | "credential_store_unavailable"
  | "credential_store_locked";

export type LedgerView = { kind: "verified"; deletions: string } | { kind: "corrupt" } | { kind: "unsupported" };

/** The store's Recovery state; the snapshot list belongs to the Recovery section. */
export type RecoveryView =
  | { kind: "unsupported" }
  | { kind: "serving"; snapshotCount: number; ledger: LedgerView }
  | {
      kind: "read_only";
      reason: "integrity_check_failed" | "migration_failed" | "unknown";
      snapshotCount: number;
      ledger: LedgerView;
    };

export type AuditBreachKind = "head_missing" | "head_not_in_store" | "head_diverged" | "record_altered" | "target_altered";

/** `absent`: an older minor, or read-only Recovery whose audit could not be validated. */
export type SecurityView =
  | { kind: "trusted" }
  | { kind: "degraded"; breach: AuditBreachKind; breachSequence: string | null; epoch: string }
  | { kind: "absent" };

export type HealthIssueSection = "capabilities" | "storage" | "retention";

/**
 * One Plane's versions, capabilities, degraded conditions, Recovery and
 * Security state (wave 3.3 §4.4). Only the status read is fatal; the other
 * reads fail per section into `issues`.
 */
export type SystemHealth = {
  planeId: PlaneId;
  planeLabel: string;
  service: { coreVersion: string; daemonStarts: string; startedAtUnixMs: string };
  app: { version: string; supportedProtocol: string };
  /** What this app can prove about the negotiated minor. */
  protocol: ProtocolKnowledge;
  platform: string | null;
  tools: Array<{ tool: ExternalToolName; label: string; version: string | null }>;
  crafts: Array<{ id: string; version: string; harnesses: string[] }>;
  credentialStore: {
    state: "available" | "locked" | "unavailable";
    kind: "apple_keychain" | "secret_service";
  } | null;
  degraded: Array<{ kind: DegradedKind; label: string; target: SettingsLink | null }>;
  recovery: RecoveryView;
  security: SecurityView;
  storage: { disposableMiB: number | null };
  retention: { graceDays: number | null };
  issues: Array<{ section: HealthIssueSection; error: PublicError }>;
};

/**
 * Reads one Plane's health. `fresh` asks the Plane to observe its
 * capabilities again (Check again).
 */
export const loadSystemHealth = (planeId: PlaneId, fresh = false) =>
  invoke<SystemHealth>("load_system_health", { planeId, fresh });
