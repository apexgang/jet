import type { PublicError } from "$lib/jet/bridge";
import type { AuditActorKind, AuditBreachKind, AuditEntry, AuditOutcome, AuditRisk } from "$lib/jet/system";
import { formatWhen, unixMs } from "./model";

/**
 * Plain labels for the decisions a Plane records (jet-core audit encoding).
 * A decision this app does not know is shown generically with its code
 * (ADR-0094), never as free text.
 */
const DECISIONS: Readonly<Record<string, string>> = {
  "account.bound": "Account connected",
  "account.unbound": "Account disconnected",
  "approval.reviewed": "Approval answered",
  "approval.retry_authorized": "Approval retry allowed",
  "audit.epoch_begun": "New audit period started",
  "autodelete.everywhere_authorized": "Auto-delete rule allowed to delete everywhere",
  "autodelete.rule_approved": "Auto-delete rule approved",
  "autodelete.rule_compiled": "Auto-delete rule drafted",
  "autodelete.rule_deleted": "Auto-delete rule deleted",
  "autodelete.rule_edited": "Auto-delete rule edited",
  "connection.authenticated": "Device connected",
  "conversation.deleted": "Task deleted",
  "conversation.deletion_authorized": "Task deletion everywhere requested",
  "conversation.forgotten": "Task forgotten",
  "conversation.restored": "Task restored from Jet Trash",
  "craft.developer_mode_cleared": "Developer mode setting cleared",
  "craft.developer_mode_disabled": "Developer mode turned off",
  "craft.developer_mode_enabled": "Developer mode turned on",
  "craft.disabled": "Harness package disabled",
  "craft.installation_approved": "Harness package installation approved",
  "execution.resolution_requested": "Retained process resolution requested",
  "extension.disable": "Extension disabled",
  "extension.install": "Extension installed",
  "extension.remove": "Extension removed",
  "extension.update": "Extension updated",
  "pairing.claimed": "Pairing code used",
  "pairing.client_disabled": "Paired device turned off",
  "pairing.client_enabled": "Paired device turned on",
  "pairing.client_revoked": "Paired device removed",
  "pairing.completed": "Pairing completed",
  "pairing.confirmed": "Pairing confirmed",
  "pairing.gate_closed": "Pairing closed",
  "pairing.gate_opened": "Pairing opened",
  "pairing.offer_invalidated": "Pairing code withdrawn",
  "pairing.offered": "Pairing code created",
  "policy.audit_retention_changed": "Audit retention changed",
  "policy.audit_retention_cleared": "Audit retention reset",
  "policy.auto_continue_changed": "Auto-continue changed",
  "policy.git_automation_cleared": "Git automation reset",
  "policy.git_automation_disabled": "Git automation turned off",
  "policy.git_automation_enabled": "Git automation turned on",
  "policy.review_changed": "Review policy changed",
  "policy.trash_grace_changed": "Jet Trash period changed",
  "policy.trash_grace_cleared": "Jet Trash period reset",
  "policy.utility_changed": "Utility work policy changed",
  "project.registered": "Project added",
  "project.removed": "Project removed",
  "recovery.import_authorized": "Recovery bundle import allowed",
  "recovery.snapshot_restored": "Recovery snapshot restored",
  "recovery.snapshots_purged": "Old recovery snapshots removed",
  "recovery.unencrypted_export_authorized": "Unencrypted recovery export allowed",
  "remote.reviewed": "Remote tool reviewed",
  "terminal.closed": "Terminal closed",
  "terminal.input": "Terminal input sent",
  "terminal.opened": "Terminal opened",
  "transfer.aborted": "Plane transfer canceled",
  "transfer.committed": "Plane transfer completed",
  "transfer.imported": "Task moved in from another Plane",
  "transfer.prepared": "Plane transfer prepared",
  "transfer.relinquished": "Task moved to another Plane",
};

export function decisionLabel(decision: string): string {
  return Object.hasOwn(DECISIONS, decision) ? DECISIONS[decision] : `Security decision (${decision})`;
}

const ACTORS: Record<AuditActorKind, string> = {
  this_device: "This computer",
  other_client: "Another device",
  craft_revocation: "Harness package revocation",
  retention: "Jet Trash cleanup",
};

export function actorLabel(kind: AuditActorKind): string {
  return ACTORS[kind];
}

const RISKS: Record<AuditRisk, string> = {
  routine: "Routine",
  elevated: "Widens access",
  destructive: "Destructive",
};

export function riskLabel(risk: AuditRisk): string {
  return RISKS[risk];
}

const OUTCOMES: Record<AuditOutcome, string> = {
  succeeded: "Done",
  denied: "Refused",
  failed: "Failed",
};

export function outcomeLabel(outcome: AuditOutcome): string {
  return OUTCOMES[outcome];
}

/** The record's time, or a plain fallback for a stamp that isn't a number. */
export function entryWhen(entry: Pick<AuditEntry, "recordedAtUnixMs">): string {
  const at = unixMs(entry.recordedAtUnixMs);
  return at === null ? "Unknown time" : formatWhen(at);
}

/** One row as a sentence: "{abs} · {decision} · {actor} · {risk} · {outcome}". */
export function entryLine(entry: AuditEntry): string {
  return [
    entryWhen(entry),
    decisionLabel(entry.decision),
    actorLabel(entry.actor.kind),
    riskLabel(entry.risk),
    outcomeLabel(entry.outcome),
  ].join(" · ");
}

/** What the audit's validation found, in plain words. */
export function breachExplanation(breach: AuditBreachKind): string {
  switch (breach) {
    case "head_missing":
      return "the note of how far the audit had reached is missing";
    case "head_not_in_store":
      return "its stored records end before the last record Jet noted";
    case "head_diverged":
      return "its stored history differs from what Jet noted";
    case "record_altered":
      return "a record was changed after it was written";
    case "target_altered":
      return "what a record is about was changed after it was written";
  }
}

export const AUDIT_INTRO_TAIL =
  "It never contains prompts, file contents, terminal output, or passwords. Oldest records first; newest last.";

export function auditIntro(planeLabel: string): string {
  return `A private record of security decisions on ${planeLabel}. ${AUDIT_INTRO_TAIL}`;
}

export function degradedText(planeLabel: string, breach: AuditBreachKind): string {
  return (
    `Jet can't verify the security audit on ${planeLabel} (${breachExplanation(breach)}). ` +
    "Reading and running tasks continue, but trust, policy, and deletion changes are paused. " +
    "Save the evidence, then start a new audit period."
  );
}

export function deniedText(planeLabel: string): string {
  return `Only the owner of ${planeLabel} can view its security audit.`;
}

export function unsupportedText(planeLabel: string): string {
  return `${planeLabel} doesn't provide a security audit to this app.`;
}

export function savedText(records: string, fileName: string): string {
  const count = records === "1" ? "1 record" : `${records} records`;
  return `Saved ${count} to ${fileName}.`;
}

/** Copy for a failed evidence export. */
export function exportFailureText(error: Pick<PublicError, "code" | "category" | "message">, planeLabel: string): string {
  switch (error.code) {
    case "audit.export_busy":
      return "Jet is already saving this Plane's audit evidence.";
    case "audit.export_failed":
      return "Jet couldn't write the file. Choose another folder and try again.";
    case "audit.export_too_large":
      return "This audit is too large to save from this app.";
  }
  if (error.category === "unauthorized") return deniedText(planeLabel);
  if (error.category === "offline") return `Jet can't reach ${planeLabel}. Try again when it reconnects.`;
  return error.message;
}

/**
 * Epoch review refusals that mean the view it was opened from is out of
 * date: reload and look again.
 */
const STALE_EPOCH_CODES: ReadonlySet<string> = new Set([
  "audit.export_required",
  "audit.not_degraded_local",
  "security.audit_trusted",
  "recovery.review_expired",
  "client.review_plane_mismatch",
  "plane.review_moved",
]);

export function isStaleEpochError(error: Pick<PublicError, "code">): boolean {
  return STALE_EPOCH_CODES.has(error.code);
}

/** Copy for a refused or stale new-audit-period request. */
export function epochRefusalText(error: Pick<PublicError, "code" | "message">, planeLabel: string): string {
  switch (error.code) {
    case "audit.export_required":
      return "Save the audit evidence for this audit period first.";
    case "audit.not_degraded_local":
    case "security.audit_trusted":
      return `The security audit on ${planeLabel} doesn't need a new period now. Reload to see its state.`;
    case "security.gap_unknown":
      return "Jet couldn't find where the audit stopped. Copy the diagnostic summary and contact support.";
    case "recovery.request_unresolved":
      return "Jet is still confirming an earlier request to start a new audit period. Try that one again first.";
    case "recovery.review_expired":
      return "This review expired. Review again.";
    case "client.review_plane_mismatch":
    case "plane.review_moved":
      return `${planeLabel} changed since you opened this review. Review again.`;
    default:
      return error.message;
  }
}

export function epochReviewText(planeLabel: string): string {
  return (
    `Start a new audit period on ${planeLabel}? The new period records that earlier records couldn't be verified. ` +
    "This can't be undone."
  );
}

export const EPOCH_UNCERTAIN_TEXT =
  "Jet couldn't confirm the request. Try again to resend the same request. It won't be applied twice.";
