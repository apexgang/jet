import type { PublicError } from "$lib/jet/bridge";
import type { AutodeleteChange, AutodeleteRule, PendingRuleChange } from "$lib/jet/retention";
import { formatDate } from "$lib/features/trash/model";

/** The Plane's prompt bound, in UTF-8 bytes (`utility.input_limit`). */
export const PROMPT_LIMIT = 4096;
/** Whole days of inactivity a rule may name. */
export const MIN_DAYS = 1;
export const MAX_DAYS = 36_500;
/** A compiling rule is read again after this long… */
export const POLL_DELAY_MS = 2_000;
/** …at most this many times before the view asks for a manual refresh. */
export const POLL_LIMIT = 5;
/** The Plane lists at most this many candidates per rule. */
export const CANDIDATES_LIMIT = 32;

const encoder = new TextEncoder();

export function promptBytes(text: string): number {
  return encoder.encode(text).length;
}

/** "{n} / 4,096 bytes" under the Rule field. */
export function byteCounter(text: string): string {
  return `${promptBytes(text).toLocaleString("en-US")} / ${PROMPT_LIMIT.toLocaleString("en-US")} bytes`;
}

/**
 * Whether the wording can be sent: 1 to 4,096 bytes, and line breaks and
 * tabs are the only control characters. The shell checks the same bound.
 */
export function promptFits(text: string): boolean {
  const bytes = promptBytes(text);
  if (bytes === 0 || bytes > PROMPT_LIMIT) return false;
  // eslint-disable-next-line no-control-regex
  return !/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/.test(text);
}

/** The typed number of days, or null when it isn't a whole number in range. */
export function parseDays(text: string): number | null {
  if (!/^\d{1,5}$/.test(text.trim())) return null;
  const days = Number(text.trim());
  return days >= MIN_DAYS && days <= MAX_DAYS ? days : null;
}

function daysText(days: number): string {
  return days === 1 ? "1 day" : `${days.toLocaleString("en-US")} days`;
}

export function introText(graceDays: number | null): string {
  const wait = graceDays === null ? "wait in Jet Trash first" : `wait in Jet Trash for ${daysText(graceDays)} first`;
  return `Describe which tasks to clean up. Jet drafts a rule you review. Nothing is deleted until you approve it, and matches ${wait}.`;
}

/** The Provider disclosure above "Create draft" (ADR-0099). */
export function draftingText(enabled: boolean | null): string {
  if (enabled === true) {
    return "Jet sends this rule's text to the Utility Provider selected in Settings › Agents to draft it. Task contents aren't sent.";
  }
  if (enabled === false) {
    return "Rule drafting is off. Jet records the rule as not drafted, and you set the number of days yourself.";
  }
  return "Jet couldn't check whether rule drafting is on. If it's off, you set the number of days yourself.";
}

/** Drafting is on but no Utility account is chosen, so drafting will be refused. */
export const NO_BINDING_TEXT =
  "No Utility account is selected for this Plane, so Jet can't draft rules. You can still set the number of days yourself.";

/** Why the Plane couldn't turn the wording into a rule. */
export function refusalCopy(reason: string): { text: string; drafting: boolean } {
  if (reason === "utility.disabled") {
    return {
      text: "Rule drafting is off. Turn it on in Settings › Agents, or set the number of days yourself.",
      drafting: true,
    };
  }
  return {
    text: "Jet couldn't turn this into a rule. Edit the wording or set the number of days yourself.",
    drafting: false,
  };
}

export const COMPILING_TEXT = "Preparing a draft…";
export const STILL_PREPARING_TEXT = "Still preparing. Refresh to check again.";
export const PENDING_TEXT = "Jet couldn't confirm your last change. Try again to resend it.";
export const DELETE_TEXT = "Delete this rule? Tasks it already moved stay in Jet Trash until their dates.";
export const CHANGED_TEXT = "This rule changed on another device. Review the new version before approving.";

export function draftText(days: number): string {
  return `Matches tasks with no activity for ${daysText(days)}.`;
}

export function approvedText(rule: Pick<AutodeleteRule, "scope">, approvedAtUnixMs: string, now: number): string {
  const since = formatDate(approvedAtUnixMs, now).absolute;
  return rule.scope === "everywhere"
    ? `Active since ${since}. Matching tasks move to Jet Trash, and Jet records a request to delete their Harness history after the grace period.`
    : `Active since ${since}. Matching tasks move to Jet Trash (forget).`;
}

export function attributionText(attribution: AutodeleteRule["attribution"]): string | null {
  return attribution ? `Drafted by ${attribution.model} via ${attribution.provider}` : null;
}

export function approveText(days: number, planeLabel: string): string {
  return `From now on, tasks on ${planeLabel} with no activity for ${daysText(days)} move to Jet Trash. You can restore them there until their dates.`;
}

export function everywhereText(planeLabel: string): string {
  return `Also request deletion of Harness history for tasks this rule matches on ${planeLabel}? Jet records the request when each task's grace period ends. No installed Harness supports this yet.`;
}

/** "{count} tasks match today", noting when the Plane listed only its first 32. */
export function candidatesHeading(rule: Pick<AutodeleteRule, "candidates" | "candidatesCapped">): string {
  const count = rule.candidates.length;
  const matches = count === 1 ? "1 task matches today" : `${count} tasks match today`;
  return rule.candidatesCapped ? `${matches} (showing the first ${CANDIDATES_LIMIT})` : matches;
}

/** A refused or local change, in plain words. */
export function changeRefusalText(error: Pick<PublicError, "code" | "message">): string {
  switch (error.code) {
    case "autodelete.interpretation_changed":
    case "autodelete.token_expired":
      return CHANGED_TEXT;
    case "autodelete.retry_mismatch":
      return "Jet is still confirming your previous change to this rule. Try that one again first.";
    case "autodelete.compiling":
      return "This rule is still being drafted. Wait for the draft, then approve it.";
    case "autodelete.refused":
      return "This rule has no draft to approve. Set the number of days first.";
    case "autodelete.already_approved":
      return "This rule is already approved.";
    case "autodelete.not_approved":
      return "Approve the rule before allowing delete everywhere.";
    case "autodelete.already_everywhere":
      return "This rule already requests deletion everywhere.";
    case "autodelete.not_found":
    case "autodelete.rule_unknown":
      return "This rule no longer exists. Refresh to see the current rules.";
    case "autodelete.prompt_invalid":
    case "utility.input_limit":
      return "Describe the rule in 1 to 4,096 bytes of text.";
    case "autodelete.days_invalid":
    case "autodelete.inactive_days_out_of_range":
      return "Choose from 1 to 36,500 days.";
    case "security.audit_degraded":
      return "Rule changes are paused until the security audit is checked (Safety › Audit).";
    case "recovery.read_only":
      return "This Plane is in read-only recovery. Rule changes wait until it's restored.";
    default:
      return error.message;
  }
}

/** Refusals after which the rules are read again. */
export function refusalReloads(error: Pick<PublicError, "code">): boolean {
  return [
    "autodelete.interpretation_changed",
    "autodelete.token_expired",
    "autodelete.not_found",
    "autodelete.rule_unknown",
    "autodelete.compiling",
    "autodelete.refused",
    "autodelete.already_approved",
    "autodelete.not_approved",
    "autodelete.already_everywhere",
  ].includes(error.code);
}

/** The slot a change occupies: one per rule, one for a new rule. */
export type RuleSlot = string;
export const NEW_RULE: RuleSlot = "new";

export function slotOf(ruleId: string | null): RuleSlot {
  return ruleId ?? NEW_RULE;
}

/** Unconfirmed changes by slot, as the Plane view lists them. */
export function pendingBySlot(pending: readonly PendingRuleChange[]): Record<RuleSlot, AutodeleteChange> {
  const bySlot: Record<RuleSlot, AutodeleteChange> = {};
  for (const entry of pending) bySlot[slotOf(entry.ruleId)] = entry.change;
  return bySlot;
}

/** What an unconfirmed change was, for "Try again". */
export function pendingLabel(change: AutodeleteChange): string {
  switch (change.kind) {
    case "compile":
      return change.rule_id === null ? "Create draft" : "Save wording";
    case "set_inactive_days":
      return `Set to ${daysText(change.inactive_days)}`;
    case "approve":
      return "Approve rule";
    case "authorize_everywhere":
      return "Allow delete everywhere";
    case "delete":
      return "Delete rule";
  }
}
