import { invoke } from "@tauri-apps/api/core";

import type { PublicError } from "./bridge";
import type { PlaneId } from "./planes";

/** Why a task is in Jet Trash (protocol `TrashReason`). */
export type TrashReason =
  | "manual_forget"
  | "automatic_forget"
  | "delete_everywhere"
  | "autodelete_rule"
  | "autodelete_everywhere"
  | "plane_transfer";

/** One task in Jet Trash. Dates are signed Unix milliseconds as decimal strings. */
export type TrashEntry = {
  conversationId: string;
  reason: TrashReason;
  trashedAtUnixMs: string;
  expiresAtUnixMs: string;
  /** False for a Transfer tombstone: that copy can't be restored. */
  restorable: boolean;
};

/** One Plane's Jet Trash, soonest deletion first. */
export type TrashView = {
  planeId: PlaneId;
  planeLabel: string;
  cursor: string;
  /** `retention.trash_grace_days`; null when the Plane did not say. */
  graceDays: number | null;
  /** The Plane returned its limit of 256 entries; more may be in Jet Trash. */
  capped: boolean;
  entries: TrashEntry[];
};

export type TrashStatus = { trash: TrashEntry | null };

/** A resolved task name; null when the Plane could not name it. */
export type ConversationName = { conversationId: string; title: string | null };

export type RetentionProtection =
  | "active_run"
  | "pending_turn"
  | "enabled_schedule"
  | "dirty_workspace"
  | "unpushed_work"
  | "unresolved_effect";

export type ProtectionView = { kind: RetentionProtection; label: string };

/**
 * A native review of moving one task to Jet Trash. `reviewId` is empty when
 * the task is already there (`trash` is set) and nothing can be reviewed.
 */
export type TrashPreview = {
  reviewId: string;
  conversationId: string;
  title: string;
  planeLabel: string;
  /** Null when the Plane couldn't inspect the task's workspace. */
  protections: ProtectionView[] | null;
  workspaceUnchecked: boolean;
  trash: TrashEntry | null;
  auditRecords: string | null;
  graceDays: number | null;
  activeRun: boolean;
  pendingTurn: boolean;
  stopAcknowledgementRequired: boolean;
};

export type TrashMode = "forget" | "delete_everywhere";

export type TrashOutcome = { kind: "trashed"; entry: TrashEntry } | { kind: "refused"; error: PublicError };

export type RestoreOutcome = { kind: "restored"; conversationId: string } | { kind: "refused"; error: PublicError };

export const loadTrash = (planeId: PlaneId) => invoke<TrashView>("load_trash", { planeId });

/** The task's own Trash state, read from its retention preview. */
export const loadTrashStatus = (planeId: PlaneId, conversationId: string) =>
  invoke<TrashStatus>("load_trash_status", { planeId, conversationId });

/** Names for 1–32 distinct tasks the loaded task list does not hold. */
export const resolveConversationNames = (planeId: PlaneId, conversationIds: string[]) =>
  invoke<ConversationName[]>("resolve_conversation_names", { planeId, conversationIds });

export const previewTrash = (planeId: PlaneId, conversationId: string) =>
  invoke<TrashPreview>("preview_trash", { planeId, conversationId });

/**
 * Sends a reviewed request. The first attempt locks `mode`; an uncertain
 * failure rejects and "Try again" resends the same review.
 */
export const trashConversation = (planeId: PlaneId, reviewId: string, mode: TrashMode, acknowledgeStop: boolean) =>
  invoke<TrashOutcome>("trash_conversation", { planeId, reviewId, mode, acknowledgeStop });

/** Restores the task's Trash entry as last read natively. */
export const restoreConversation = (planeId: PlaneId, conversationId: string) =>
  invoke<RestoreOutcome>("restore_conversation", { planeId, conversationId });
