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

export type SnapshotReason = "daily" | "migration" | "maintenance";

/**
 * One verified Recovery snapshot. `snapshotId` is an opaque token the shell
 * issued for this read; the snapshot's file name never reaches the webview.
 */
export type RecoverySnapshot = {
  snapshotId: string;
  takenAtUnixMs: string;
  reason: SnapshotReason;
  bytes: string;
};

/**
 * The store's Recovery state. `snapshotCount` counts every snapshot the Plane
 * reported; `snapshots` lists at most the newest 64, newest first.
 */
export type RecoveryView =
  | { kind: "unsupported" }
  | { kind: "serving"; snapshotCount: number; snapshots: RecoverySnapshot[]; ledger: LedgerView }
  | {
      kind: "read_only";
      reason: "integrity_check_failed" | "migration_failed" | "unknown";
      snapshotCount: number;
      snapshots: RecoverySnapshot[];
      ledger: LedgerView;
    };

export type AuditBreachKind = "head_missing" | "head_not_in_store" | "head_diverged" | "record_altered" | "target_altered";

/** `absent`: an older minor, or read-only Recovery whose audit could not be validated. */
export type SecurityView =
  | { kind: "trusted" }
  | {
      kind: "degraded";
      breach: AuditBreachKind;
      breachSequence: string | null;
      epoch: string;
      /** This app saved the evidence of this epoch, so a new audit period may be reviewed. */
      exported: boolean;
    }
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

/** What the Recovery section asks the shell to review. Snake_case inner fields, as the shell reads them. */
export type RecoveryAction =
  | { kind: "restore_snapshot"; snapshot_id: string }
  | { kind: "purge_snapshots" }
  | { kind: "begin_audit_epoch" };

/** A native review of a restore or purge, valid for 10 minutes and usable once. */
export type RecoveryReview =
  | {
      kind: "restore_snapshot";
      reviewId: string;
      planeLabel: string;
      takenAtUnixMs: string;
      reason: SnapshotReason;
      bytes: string;
    }
  | {
      kind: "purge_snapshots";
      reviewId: string;
      planeLabel: string;
      snapshotCount: number;
      totalBytes: string;
      deletionsRecorded: string;
      /** A rollback copy for the previous Jet release may be removed too. */
      includesRollback: boolean;
    }
  | {
      kind: "begin_audit_epoch";
      reviewId: string;
      planeLabel: string;
      /** The epoch that failed to validate. */
      degradedEpoch: string;
      breach: AuditBreachKind;
      /** The audit position the saved evidence reaches. */
      exportedThrough: string;
    };

/**
 * What a reviewed restore or purge did. `unconfirmed`: it may or may not have
 * run; re-read the Plane and never resend.
 */
export type RecoveryOutcome =
  | { kind: "restored"; takenAtUnixMs: string; reason: SnapshotReason; replacedName: string | null }
  | { kind: "purged"; removedCount: number }
  | { kind: "epoch_begun"; epoch: string }
  | { kind: "refused"; error: PublicError }
  | { kind: "unconfirmed"; error: PublicError };

/** Reviews a restore (read-only Recovery only) or a purge after a fresh status read. */
export const prepareRecoveryAction = (planeId: PlaneId, action: RecoveryAction) =>
  invoke<RecoveryReview>("prepare_recovery_action", { planeId, action });

/**
 * Sends a reviewed restore or purge at most once. A new audit epoch is
 * receipt-deduplicated instead: an uncertain send rejects with a
 * `PublicError`, and executing the same review again resends it.
 */
export const executeRecoveryAction = (planeId: PlaneId, reviewId: string) =>
  invoke<RecoveryOutcome>("execute_recovery_action", { planeId, reviewId });

// Security audit ------------------------------------------------------------

export type AuditActorKind = "this_device" | "other_client" | "craft_revocation" | "retention";
export type AuditRisk = "routine" | "elevated" | "destructive";
export type AuditOutcome = "succeeded" | "denied" | "failed";

/**
 * One Security-audit record, redacted natively. `clientId`, `identity` and
 * `reference` are null unless identifiers were requested; `kind` and
 * `decision` are stable codes, or "unknown".
 */
export type AuditEntry = {
  sequence: string;
  epoch: string;
  recordedAtUnixMs: string;
  actor: { kind: AuditActorKind; clientId: string | null };
  target: { kind: string; identity: string | null; reference: string | null };
  decision: string;
  risk: AuditRisk;
  outcome: AuditOutcome;
};

/** One page, oldest first. `complete`: no newer record exists beyond it. */
export type AuditPage = { cursor: string; complete: boolean; entries: AuditEntry[] };

/** Only the chosen file's name comes back; its path stays in the shell. */
export type AuditExport = { kind: "saved"; records: string; fileName: string } | { kind: "canceled" };

/**
 * Reads one page of a Plane's owner-only Security audit strictly after
 * `after` (null: from the oldest retained record).
 */
export const loadSecurityAudit = (planeId: PlaneId, after: string | null, reveal: boolean) =>
  invoke<AuditPage>("load_security_audit", { planeId, after, reveal });

/**
 * Saves the whole audit as JSON Lines to a file chosen in a native save
 * dialog. Refused with `audit.export_busy` while an export of that Plane runs.
 */
export const exportSecurityAudit = (planeId: PlaneId) =>
  invoke<AuditExport>("export_security_audit", { planeId });
