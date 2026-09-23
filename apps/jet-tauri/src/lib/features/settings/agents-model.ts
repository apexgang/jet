import {
  AUTO_CONTINUE_LIMITS,
  type AutoContinuePolicyInput,
  type AutoContinuePolicyView,
  type CraftPreview,
  type CraftView,
  type CredentialSourceKind,
  type QuotaWindowView,
  type TokenCounts,
  type UsageHistoryView,
} from "$lib/jet/agents";
import type { AppliedDetail } from "$lib/jet/settings";
import { utf8Bytes } from "./model";

/** A decimal string from the shell (64-bit on the wire), grouped for reading. */
export function formatCount(decimal: string): string {
  try {
    return BigInt(decimal).toLocaleString("en-US");
  } catch {
    return decimal;
  }
}

/** A Craft's row title: its Harnesses by product name ("Codex and Claude Code"). */
export function craftTitle(craft: CraftView): string {
  const names = [...new Set(craft.harnesses.map((harness) => harness.name))];
  if (names.length === 0) return craft.craftId;
  if (names.length === 1) return names[0];
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

function timeText(unixMs: string, now: Date): string {
  const at = new Date(Number(unixMs));
  if (Number.isNaN(at.getTime())) return "later";
  const sameDay = at.toDateString() === now.toDateString();
  return sameDay
    ? at.toLocaleTimeString("en-US", { hour: "2-digit", minute: "2-digit" })
    : at.toLocaleString("en-US", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

/** One quota window in words: "5h: 62% used, resets 14:00". */
export function quotaText(window: QuotaWindowView, now: Date = new Date()): string {
  const scope = window.scope.kind === "model" ? ` (${window.scope.model})` : "";
  const name = `${window.window}${scope}`;
  if (window.freshness === "unreachable") return `${name}: Provider didn't answer`;
  let amount: string;
  const used = Number(window.used);
  const limit = window.limit === null ? null : Number(window.limit);
  if (limit !== null && limit > 0 && Number.isFinite(used)) {
    amount = `${Math.min(999, Math.round((used / limit) * 100))}% used`;
  } else if (window.unit === "share") {
    amount = `${formatCount(window.used)}% used`;
  } else {
    amount = `${formatCount(window.used)} ${window.unit} used`;
  }
  const resets = window.resetsAtUnixMs ? `, resets ${timeText(window.resetsAtUnixMs, now)}` : "";
  const notes = [
    window.freshness === "stale" ? "Stale" : null,
    window.estimation === "estimated" ? "estimated" : null,
  ].filter((note): note is string => note !== null);
  return `${name}: ${amount}${resets}${notes.length ? ` (${notes.join(", ")})` : ""}`;
}

/** How a binding's credential is resolved, in plain words. */
export function credentialSourceText(source: CredentialSourceKind): string {
  switch (source) {
    case "harness_native":
      return "Uses the Harness's own sign-in";
    case "platform_store":
      return "Uses your system keyring";
    case "external_helper":
      return "Uses a credential helper";
    case "session_only":
      return "Signed in until Jet restarts";
  }
}

/** What an applied Agents change did, for the pane's status line. */
export function receiptText(detail: AppliedDetail): string {
  switch (detail.kind) {
    case "account_bound":
      return "Account connected.";
    case "auto_continue":
      return "Saved what happens when a usage limit is reached.";
    case "account_unbound":
      switch (detail.cleanup) {
        case "keyring_item_remains":
          return "Jet no longer uses this sign-in. Remove it from your system keyring if you don't need it.";
        case "helper":
          return "Jet no longer uses this sign-in. Its credential helper still has it.";
        case "session":
          return "Jet no longer uses this sign-in.";
        case "none":
          return "Jet no longer uses this sign-in. The Harness keeps its own sign-in.";
      }
      break;
    case "craft_disabled":
      return "Disabled. Jet can't turn it back on for this Plane.";
    case "craft_install_queued":
      return `Installing ${detail.craftId} ${detail.version}. It appears here when it's ready.`;
    case "setting":
      return "Saved.";
  }
  return "Saved.";
}

// ---------------------------------------------------------------------------
// Auto-continue form
// ---------------------------------------------------------------------------

/** The Auto-continue form. Delays are edited in seconds. */
export type AutoContinueDraft =
  | { mode: "off" }
  | { mode: "retry"; delaySeconds: string; maxDelaySeconds: string; maxRetries: string; message: string };

export const DEFAULT_RETRY_MESSAGE = "Continue where you left off.";

/** The form for a Plane's policy. A message that can't be shown starts empty. */
export function draftFor(policy: AutoContinuePolicyView | null): AutoContinueDraft {
  if (!policy || policy.mode === "off") return { mode: "off" };
  return {
    mode: "retry",
    delaySeconds: String(policy.delayMs / 1000),
    maxDelaySeconds: String(policy.maxDelayMs / 1000),
    maxRetries: String(policy.maxRetries),
    message: policy.message ?? "",
  };
}

/** A default retry policy for switching the form to "Continue automatically". */
export function retryDraft(): AutoContinueDraft {
  return { mode: "retry", delaySeconds: "300", maxDelaySeconds: "3600", maxRetries: "3", message: DEFAULT_RETRY_MESSAGE };
}

function seconds(value: string): number | null {
  if (value.trim() === "") return null;
  const number = Number(value);
  if (!Number.isFinite(number)) return null;
  const ms = Math.round(number * 1000);
  return Number.isInteger(ms) ? ms : null;
}

/**
 * The policy a draft describes, or the problem with it. Mirrors the native
 * bounds as a hint; the shell and `jetd` check them again.
 */
export function policyFromDraft(draft: AutoContinueDraft): { policy: AutoContinuePolicyInput } | { problem: string } {
  if (draft.mode === "off") return { policy: { mode: "off" } };
  const delay = seconds(draft.delaySeconds);
  const maxDelay = seconds(draft.maxDelaySeconds);
  const retries = draft.maxRetries.trim() === "" ? Number.NaN : Number(draft.maxRetries);
  const { minDelayMs, maxDelayMs, minRetries, maxRetries, maxMessageBytes } = AUTO_CONTINUE_LIMITS;
  const inRange = (value: number | null) => value !== null && value >= minDelayMs && value <= maxDelayMs;
  if (!inRange(delay) || !inRange(maxDelay)) {
    return { problem: "Use waits from 0.001 seconds to 24 hours (86,400 seconds)." };
  }
  if ((delay as number) > (maxDelay as number)) {
    return { problem: "The first wait can't be longer than the longest wait." };
  }
  if (!Number.isInteger(retries) || retries < minRetries || retries > maxRetries) {
    return { problem: `Allow from ${minRetries} to ${maxRetries} retries.` };
  }
  if (draft.message.trim() === "") return { problem: "Write the message Jet sends to continue." };
  if (utf8Bytes(draft.message) > maxMessageBytes) {
    return { problem: `Keep the message to ${maxMessageBytes.toLocaleString("en-US")} bytes.` };
  }
  for (const character of draft.message) {
    const code = character.codePointAt(0) ?? 0;
    const control = code < 0x20 || (code >= 0x7f && code <= 0x9f);
    if (control && character !== "\n" && character !== "\t") {
      return { problem: "Remove control characters from the message." };
    }
  }
  return {
    policy: {
      mode: "retry",
      delay_ms: delay as number,
      max_delay_ms: maxDelay as number,
      max_retries: retries,
      message: draft.message,
    },
  };
}

/** A policy in words, for the review dialog. */
export function policyText(policy: AutoContinuePolicyView): string {
  if (policy.mode === "off") return "Wait for me";
  const wait = (ms: number) => `${(ms / 1000).toLocaleString("en-US")} s`;
  return `Continue automatically: first wait ${wait(policy.delayMs)}, at most ${wait(policy.maxDelayMs)}, up to ${policy.maxRetries} ${policy.maxRetries === 1 ? "retry" : "retries"}`;
}

// ---------------------------------------------------------------------------
// Usage history
// ---------------------------------------------------------------------------

export type HistoryRow = {
  startUnixMs: string;
  tokens: TokenCounts;
  measurements: string;
  estimated: string;
};

function add(left: string, right: string): string {
  try {
    return (BigInt(left) + BigInt(right)).toString();
  } catch {
    return left;
  }
}

/** One row per bucket, summed over every model's series, newest first. */
export function historyRows(view: UsageHistoryView): HistoryRow[] {
  const rows = new Map<string, HistoryRow>();
  for (const series of view.series) {
    for (const point of series.points) {
      const row = rows.get(point.startUnixMs);
      if (!row) {
        rows.set(point.startUnixMs, { ...point, tokens: { ...point.tokens } });
        continue;
      }
      row.tokens = {
        input: add(row.tokens.input, point.tokens.input),
        cachedInput: add(row.tokens.cachedInput, point.tokens.cachedInput),
        output: add(row.tokens.output, point.tokens.output),
        reasoning: add(row.tokens.reasoning, point.tokens.reasoning),
      };
      row.measurements = add(row.measurements, point.measurements);
      row.estimated = add(row.estimated, point.estimated);
    }
  }
  return [...rows.values()].sort((left, right) => {
    const a = BigInt(left.startUnixMs);
    const b = BigInt(right.startUnixMs);
    return a === b ? 0 : a > b ? -1 : 1;
  });
}

/** A bucket's label at the answer's resolution. */
export function bucketLabel(startUnixMs: string, resolution: "hour" | "day"): string {
  const at = new Date(Number(startUnixMs));
  if (Number.isNaN(at.getTime())) return startUnixMs;
  return resolution === "hour"
    ? at.toLocaleString("en-US", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })
    : at.toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" });
}

// ---------------------------------------------------------------------------
// Craft installation
// ---------------------------------------------------------------------------

/** `owner/name`: the same shape the shell accepts. */
export function releaseProblem(repository: string, tag: string): string | null {
  if (!/^[A-Za-z0-9._-]{1,100}\/[A-Za-z0-9._-]{1,100}$/.test(repository.trim())) {
    return "Enter the repository as owner/name.";
  }
  if (!/^[\x21-\x7e]{1,128}$/.test(tag.trim())) return "Enter a release tag without spaces.";
  return null;
}

export function brokerPermissionText(permission: CraftPreview["brokerPermissions"][number]): string {
  switch (permission) {
    case "artifact_read":
      return "Read task artifacts";
    case "artifact_write":
      return "Write task artifacts";
    case "remote_tools":
      return "Use remote tools";
  }
}

export function hostAccessText(access: CraftPreview["hostAccess"][number]): string {
  switch (access.kind) {
    case "executable":
      return `Run ${access.value}`;
    case "filesystem":
      return `Use files at ${access.value}`;
    case "environment":
      return `Read the ${access.value} environment variable`;
    case "network":
      return `Connect to ${access.value}`;
  }
}
