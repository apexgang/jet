import { invoke } from "@tauri-apps/api/core";

import type { PublicError } from "./bridge";
import type { PlaneId } from "./planes";
import type { SettingsReview } from "./settings";

/** The four native lifecycle operations (`jet-protocol` `ExtensionAction`). */
export type ExtensionAction = "install" | "update" | "disable" | "remove";

export const EXTENSION_ACTIONS: readonly ExtensionAction[] = ["install", "update", "disable", "remove"];

/** A standalone entry (skill, hook, MCP server) of the Harness's own configuration. */
export type StandaloneEntry = { entryToken: string; id: string; enabled: boolean };

/** A plugin. `installed: null` when the catalog doesn't say. */
export type PluginEntry = { entryToken: string; id: string; installed: boolean | null };

/** A change queued from this app that hasn't reached a final state. */
export type ExtensionChangeRef = { changeId: string; extensionId: string; action: ExtensionAction };

export type ExtensionCatalogIssue = {
  /** `plugins`: the plugin list couldn't be read. `catalog`: some entries can't be shown. */
  section: "plugins" | "catalog";
  error: PublicError;
};

/** One Craft's catalog: identifiers and opaque entry tokens, never paths. */
export type ExtensionCatalogView = {
  craftId: string;
  /** The Harness's product name. */
  harness: string;
  standalone: StandaloneEntry[];
  plugins: PluginEntry[];
  changes: ExtensionChangeRef[];
  truncated: boolean;
  issues: ExtensionCatalogIssue[];
};

export type ExtensionFile = { path: string; sha256: string };

/** Exact facts of one inspected entry. Text is inert display text. */
export type ExtensionFacts = {
  publisher: string | null;
  version: string | null;
  source: string | null;
  files: ExtensionFile[];
  /** Every file the Craft listed, including any not shown. */
  fileCount: number;
};

export type ExtensionInspection = ExtensionFacts & {
  inspectionId: string;
  extensionId: string;
  disabled: boolean | null;
  /** Only where the Craft names them (Claude plugins). */
  supportedActions: ExtensionAction[] | null;
  /** Every file the change touches is shown; only then can it be confirmed. */
  reviewable: boolean;
};

/** What a reviewed extension change will do (the review subject's preview). */
export type ExtensionReviewPreview = ExtensionFacts & {
  craftId: string;
  harness: string;
  extensionId: string;
  action: ExtensionAction;
};

export type ExtensionChangeState = "staged" | "applied" | "refused" | "outcome_unknown";

export type ExtensionChangeStatus = {
  changeId: string;
  craftId: string;
  extensionId: string;
  action: ExtensionAction;
  state: ExtensionChangeState;
};

/** Reads one Craft's native catalog through the Plane. */
export const loadExtensionCatalog = (planeId: PlaneId, craftId: string) =>
  invoke<ExtensionCatalogView>("load_extension_catalog", { planeId, craftId });

/** Inspects the entry an entry token names. The identifier stays native. */
export const inspectExtension = (planeId: PlaneId, entryToken: string) =>
  invoke<ExtensionInspection>("inspect_extension", { planeId, entryToken });

/**
 * Reviews one change of an inspected entry. The shell builds the
 * confirmation from its own copy of the inspection; apply the review with
 * `applySettingsChange`.
 */
export const prepareExtensionChange = (planeId: PlaneId, inspectionId: string, action: ExtensionAction) =>
  invoke<SettingsReview>("prepare_extension_change", { planeId, inspectionId, action });

/** Reads a queued change's state. Only changes queued on that Plane from this app. */
export const loadExtensionChange = (planeId: PlaneId, changeId: string) =>
  invoke<ExtensionChangeStatus>("load_extension_change", { planeId, changeId });
