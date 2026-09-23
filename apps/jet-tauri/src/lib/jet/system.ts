import { invoke } from "@tauri-apps/api/core";

import type { PlaneHealth, PlaneId } from "./planes";

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
