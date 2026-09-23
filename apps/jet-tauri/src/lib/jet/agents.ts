import { invoke } from "@tauri-apps/api/core";

import type { PublicError } from "./bridge";
import type { PlaneId } from "./planes";
import type { PlaneState, SettingsReview } from "./settings";

/** A Harness by identifier and product name ("Codex", "Claude Code"). */
export type HarnessView = { id: string; name: string };

export type CraftView = { craftId: string; version: string; harnesses: HarnessView[] };

export type CredentialStoreView = {
  state: "available" | "locked" | "unavailable";
  label: string;
};

export type CredentialSourceKind = "platform_store" | "external_helper" | "harness_native" | "session_only";

export type AccountView = {
  id: string;
  label: string;
  provider: string;
  providerAccount: string | null;
  credentialSource: CredentialSourceKind;
  state: "ready" | "resolved_at_use" | "locked" | "unavailable" | "invalidated";
  stateLabel: string;
  createdAtUnixMs: string;
};

/** A Harness-native sign-in this Plane can bind. */
export type BindOption = { provider: string; harness: string };

/** Token counts are decimal strings (64-bit on the wire). */
export type TokenCounts = { input: string; cachedInput: string; output: string; reasoning: string };

export type UsageSummary = { cursor: string; tokens: TokenCounts; measurements: string; estimated: string };

export type AgentsIssue = { section: "capabilities" | "accounts" | "usage"; error: PublicError };

export type AgentsView = {
  planeState: PlaneState;
  crafts: CraftView[];
  harnesses: HarnessView[];
  credentialStore: CredentialStoreView | null;
  degraded: string[];
  accounts: AccountView[];
  bindOptions: BindOption[];
  usage: UsageSummary | null;
  /** The Account binding list's cursor. */
  cursor: string | null;
  issues: AgentsIssue[];
};

export type QuotaWindowView = {
  provider: string;
  window: string;
  scope: { kind: "account" } | { kind: "model"; model: string };
  unit: "tokens" | "requests" | "credits" | "share";
  used: string;
  limit: string | null;
  windowSeconds: string | null;
  resetsAtUnixMs: string | null;
  estimation: "measured" | "estimated";
  finality: "interim" | "final";
  observedAtUnixMs: string;
  /** The Provider's reason for `unreachable` never reaches the app. */
  freshness: "fresh" | "stale" | "unreachable";
};

/** A policy as the Plane reports it. `message: null` can't be displayed. */
export type AutoContinuePolicyView =
  | { mode: "off" }
  | { mode: "retry"; delayMs: number; maxDelayMs: number; maxRetries: number; message: string | null };

export type AutoContinueRetryView = {
  status: "disabled" | "deferred" | "pending" | "dispatched" | "canceled" | "exhausted";
  retryCount: number;
  dueAtUnixMs: string;
};

export type AccountDetail = {
  bindingId: string;
  quotaWindows: QuotaWindowView[];
  autoContinue: { policy: AutoContinuePolicyView; retry: AutoContinueRetryView | null } | null;
  /** The Auto-continue snapshot's cursor. */
  cursor: string | null;
  issues: Array<{ section: "quota" | "auto_continue"; error: PublicError }>;
};

export type HistoryDays = 1 | 7 | 30 | 90;

export type UsagePoint = { startUnixMs: string; tokens: TokenCounts; measurements: string; estimated: string };

export type UsageHistoryView = {
  cursor: string;
  /** The answer's resolution, which may be coarser than asked. */
  resolution: "hour" | "day";
  truncated: boolean;
  series: Array<{ model: string | null; points: UsagePoint[] }>;
};

/** Auto-continue input. Field names inside the input are snake_case. */
export type AutoContinuePolicyInput =
  | { mode: "off" }
  | { mode: "retry"; delay_ms: number; max_delay_ms: number; max_retries: number; message: string };

/** The native bounds (`docs/auto-continue.md`), mirrored for input hints only. */
export const AUTO_CONTINUE_LIMITS = {
  minDelayMs: 1,
  maxDelayMs: 86_400_000,
  minRetries: 1,
  maxRetries: 100,
  maxMessageBytes: 8_192,
} as const;

/** A release, or a token for files picked natively. Never a path. */
export type CraftSourceInput =
  | { type: "github_release"; repository: string; tag: string }
  | { type: "local"; source_token: string };

/** Files picked in the native dialog: basenames only. */
export type LocalCraftSource = { sourceToken: string; specificationName: string; artifactName: string };

export type CraftPreview = {
  craftId: string;
  version: string;
  source: "github_release" | "local";
  enabledFeatures: string[];
  repository: string;
  publisherClaim: string;
  commit: string;
  artifactSha256: string;
  brokerPermissions: Array<"artifact_read" | "artifact_write" | "remote_tools">;
  hostAccess: Array<{ kind: "executable" | "filesystem" | "environment" | "network"; value: string }>;
  trust: "same_user_executable" | "developer_source";
};

export type CraftDisableMode = "wait" | "force";

/** Status, capabilities, Account bindings and Plane usage in one connection. */
export const loadAgents = (planeId: PlaneId, freshCredentials: boolean) =>
  invoke<AgentsView>("load_agents", { planeId, freshCredentials });

/** Reviews a Harness-native binding. Apply it with `applySettingsChange`. */
export const prepareAccountBind = (planeId: PlaneId, provider: string) =>
  invoke<SettingsReview>("prepare_account_bind", { planeId, provider });

export const loadAccountDetail = (planeId: PlaneId, bindingId: string) =>
  invoke<AccountDetail>("load_account_detail", { planeId, bindingId });

export const loadUsageHistory = (planeId: PlaneId, bindingId: string | null, days: HistoryDays) =>
  invoke<UsageHistoryView>("load_usage_history", { planeId, bindingId, days });

export const prepareAutoContinue = (planeId: PlaneId, bindingId: string, policy: AutoContinuePolicyInput) =>
  invoke<SettingsReview>("prepare_auto_continue", { planeId, bindingId, policy });

export const prepareAccountUnbind = (planeId: PlaneId, bindingId: string) =>
  invoke<SettingsReview>("prepare_account_unbind", { planeId, bindingId });

export const prepareCraftDisable = (planeId: PlaneId, craftId: string, mode: CraftDisableMode) =>
  invoke<SettingsReview>("prepare_craft_disable", { planeId, craftId, mode });

/**
 * Opens the native file dialogs (owned by the shell, as a child of the
 * Settings window). `null` when the user cancelled.
 */
export const pickLocalCraftSource = (planeId: PlaneId) =>
  invoke<LocalCraftSource | null>("pick_local_craft_source", { planeId });

/** Verifies a source and reviews its installation. */
export const discoverCraft = (planeId: PlaneId, source: CraftSourceInput) =>
  invoke<SettingsReview>("discover_craft", { planeId, source });
