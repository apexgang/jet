import {
  EXTENSION_ACTIONS,
  type ExtensionAction,
  type ExtensionChangeState,
  type ExtensionInspection,
  type PluginEntry,
  type StandaloneEntry,
} from "$lib/jet/extensions";

/** One catalog row, as the Extensions section lists it. */
export type ExtensionEntry =
  | ({ kind: "standalone" } & StandaloneEntry)
  | ({ kind: "plugin" } & PluginEntry);

/**
 * The actions a row offers. They are affordances only: the Craft or `jetd`
 * refuses what it doesn't allow (`extension.refused`).
 *
 * - Standalone, enabled: Disable, Remove. Disabled: Install (restores the
 *   saved native content), Remove. Adding one from a folder isn't offered.
 * - Plugin: the Craft's own `supportedActions` when its inspection names
 *   them; otherwise by installation state, or all four when unknown.
 */
export function offeredActions(
  entry: ExtensionEntry,
  inspection: Pick<ExtensionInspection, "supportedActions"> | null = null,
): ExtensionAction[] {
  if (entry.kind === "standalone") return entry.enabled ? ["disable", "remove"] : ["install", "remove"];
  const supported = inspection?.supportedActions;
  if (supported) return EXTENSION_ACTIONS.filter((action) => supported.includes(action));
  if (entry.installed === true) return ["update", "disable", "remove"];
  if (entry.installed === false) return ["install"];
  return [...EXTENSION_ACTIONS];
}

export function actionLabel(action: ExtensionAction): string {
  switch (action) {
    case "install":
      return "Install";
    case "update":
      return "Update";
    case "disable":
      return "Disable";
    case "remove":
      return "Remove";
  }
}

/** A standalone entry's action label: installing a disabled one restores it. */
export function entryActionLabel(entry: ExtensionEntry, action: ExtensionAction): string {
  return entry.kind === "standalone" && action === "install" ? "Turn back on" : actionLabel(action);
}

/** What a row's state is, in words. */
export function entryStateText(entry: ExtensionEntry): string | null {
  if (entry.kind === "standalone") return entry.enabled ? "On" : "Off";
  if (entry.installed === true) return "Installed";
  if (entry.installed === false) return "Available";
  return null;
}

/** A tracked change's state; `checking` until the first status read answers. */
export type TrackedChangeState = ExtensionChangeState | "checking";

/** A final state: polling stops, and an unknown outcome is never retried. */
export function isTerminalChange(state: TrackedChangeState): boolean {
  return state === "applied" || state === "refused" || state === "outcome_unknown";
}

export function changeStateText(state: TrackedChangeState, harness: string): string {
  switch (state) {
    case "checking":
      return "Checking…";
    case "staged":
      return "Waiting for running tasks";
    case "applied":
      return "Applied";
    case "refused":
      return "Refused";
    case "outcome_unknown":
      return `Outcome unknown: check ${harness}'s settings before trying again`;
  }
}

/** How often an unfinished change is read again while the window is visible. */
export const CHANGE_POLL_MS = 2_000;
