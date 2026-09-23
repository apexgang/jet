import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import {
  actorLabel,
  breachExplanation,
  decisionLabel,
  entryLine,
  epochRefusalText,
  exportFailureText,
  isStaleEpochError,
  savedText,
} from "../src/lib/features/system/audit-model";
import { restoreReviewLines } from "../src/lib/features/system/model";
import { SystemSession } from "../src/lib/features/system/session.svelte";
import type { PublicError } from "../src/lib/jet/bridge";
import {
  exportSecurityAudit,
  loadSecurityAudit,
  prepareRecoveryAction,
  type AuditBreachKind,
  type AuditEntry,
  type AuditPage,
  type RecoveryReview,
  type SecurityView,
  type SystemHealth,
} from "../src/lib/jet/system";

const REMOTE = "0000000a-0000-4000-8000-000000000002";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function failure(
  code: string,
  category = "invalid_input",
  extra: Partial<PublicError> = {},
): PublicError {
  return {
    category,
    code,
    message: "Message",
    retryable: false,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: REMOTE,
    ...extra,
  };
}

function health(security: SecurityView, daemonStarts = "3"): SystemHealth {
  return {
    planeId: REMOTE,
    planeLabel: "Build box",
    service: { coreVersion: "1.43.0", daemonStarts, startedAtUnixMs: "1700000000000" },
    app: { version: "0.1.0", supportedProtocol: "1.43" },
    protocol: { exact: null, atLeast: 38, atMost: null },
    platform: "linux · x86_64",
    tools: [],
    crafts: [],
    credentialStore: { state: "available", kind: "secret_service" },
    degraded: [],
    recovery: { kind: "serving", snapshotCount: 0, snapshots: [], ledger: { kind: "verified", deletions: "0" } },
    security,
    storage: { disposableMiB: 2048 },
    retention: { graceDays: 30 },
    issues: [],
  };
}

function degraded(exported: boolean): SecurityView {
  return { kind: "degraded", breach: "head_not_in_store", breachSequence: null, epoch: "2", exported };
}

function entry(sequence: number): AuditEntry {
  return {
    sequence: String(sequence),
    epoch: "2",
    recordedAtUnixMs: "1700000000000",
    actor: { kind: "this_device", clientId: null },
    target: { kind: "account_binding", identity: null, reference: null },
    decision: "account.bound",
    risk: "elevated",
    outcome: "succeeded",
  };
}

function page(cursor: number, sequences: number[]): AuditPage {
  return { cursor: String(cursor), complete: sequences.length === 0 || sequences.at(-1) === cursor, entries: sequences.map(entry) };
}

function epochReview(reviewId = "review-1"): Extract<RecoveryReview, { kind: "begin_audit_epoch" }> {
  return {
    kind: "begin_audit_epoch",
    reviewId,
    planeLabel: "Build box",
    degradedEpoch: "2",
    breach: "head_not_in_store",
    exportedThrough: "40",
  };
}

async function settle(): Promise<void> {
  for (let turn = 0; turn < 8; turn++) await new Promise((resolve) => setTimeout(resolve, 0));
}

type Call = { command: string; args: Record<string, unknown> };
type Answer = (args: Record<string, unknown>) => unknown;

/** A fake shell: each command answers from `answers`. */
class Fake {
  calls: Call[] = [];
  answers = new Map<string, Answer>();

  install(): void {
    vi.stubGlobal("window", { crypto: globalThis.crypto });
    mockIPC((command, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      this.calls.push({ command, args });
      const handler = this.answers.get(command);
      if (!handler) throw failure("client.state_unavailable", "internal");
      return handler(args);
    });
  }

  of(command: string): Call[] {
    return this.calls.filter((call) => call.command === command);
  }
}

/** A Settings-window system session showing the remote Plane with its audit loaded. */
async function shown(fake: Fake, security: SecurityView = { kind: "trusted" }): Promise<SystemSession> {
  fake.answers.set("load_system_health", () => health(security));
  const system = new SystemSession();
  system.select(REMOTE);
  await system.ensureLoaded();
  await system.audit.ensureLoaded();
  return system;
}

describe("audit adapter", () => {
  it("sends the exact command names and arguments", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", () => page(0, []));
    fake.answers.set("export_security_audit", () => ({ kind: "canceled" }));
    fake.answers.set("prepare_recovery_action", () => epochReview());
    await loadSecurityAudit(REMOTE, null, false);
    await loadSecurityAudit("local", "42", true);
    await exportSecurityAudit(REMOTE);
    await prepareRecoveryAction(REMOTE, { kind: "begin_audit_epoch" });
    expect(fake.calls).toEqual([
      { command: "load_security_audit", args: { planeId: REMOTE, after: null, reveal: false } },
      { command: "load_security_audit", args: { planeId: "local", after: "42", reveal: true } },
      { command: "export_security_audit", args: { planeId: REMOTE } },
      { command: "prepare_recovery_action", args: { planeId: REMOTE, action: { kind: "begin_audit_epoch" } } },
    ]);
  });
});

describe("audit viewer paging", () => {
  it("loads oldest first and appends newer pages until complete", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", (args) =>
      args.after === null ? page(5, [1, 2, 3]) : page(5, [4, 5]),
    );
    const system = await shown(fake);
    const { audit } = system;
    expect(audit.state).toMatchObject({ kind: "ready", next: "3", complete: false });
    expect(audit.canLoadNewer).toBe(true);

    await audit.loadNewer();
    expect(fake.of("load_security_audit").map((call) => call.args.after)).toEqual([null, "3"]);
    expect(audit.state.kind === "ready" && audit.state.entries.map((row) => row.sequence)).toEqual(["1", "2", "3", "4", "5"]);
    expect(audit.state).toMatchObject({ complete: true, next: "5" });
    expect(audit.canLoadNewer).toBe(false);
    await audit.loadNewer();
    expect(fake.of("load_security_audit")).toHaveLength(2);

    // Loading once per Plane selection; another ensureLoaded reads nothing.
    await audit.ensureLoaded();
    expect(fake.of("load_security_audit")).toHaveLength(2);
  });

  it("identifiers are off by default and turning them on reads again from the first page", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", (args) => (args.after === null ? page(9, [1, 2]) : page(9, [3])));
    const system = await shown(fake);
    await system.audit.loadNewer();
    await system.audit.setReveal(true);
    expect(fake.of("load_security_audit").map((call) => call.args)).toEqual([
      { planeId: REMOTE, after: null, reveal: false },
      { planeId: REMOTE, after: "2", reveal: false },
      { planeId: REMOTE, after: null, reveal: true },
    ]);
    expect(system.audit.state.kind === "ready" && system.audit.state.entries).toHaveLength(2);

    // A Plane switch turns them off again.
    system.select("local");
    expect(system.audit.reveal).toBe(false);
    expect(system.audit.state).toEqual({ kind: "idle" });
  });

  it("starts again from the first page once when the pages went stale", async () => {
    const fake = new Fake();
    fake.install();
    let stale = true;
    fake.answers.set("load_security_audit", (args) => {
      if (args.after === "2" && stale) {
        stale = false;
        throw failure("audit.pagination_stale", "conflict", {
          restart: { reason: "pagination_stale", currentSnapshotRevision: "7" },
        });
      }
      return args.after === null ? page(9, [1, 2]) : page(9, [3]);
    });
    const system = await shown(fake);
    await system.audit.loadNewer();
    expect(fake.of("load_security_audit").map((call) => call.args.after)).toEqual([null, "2", null]);
    expect(system.audit.state).toMatchObject({ kind: "ready", next: "2" });
  });

  it("classifies denied, unsupported and offline, and keeps loaded records while offline", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", () => {
      throw failure("unauthorized", "unauthorized");
    });
    const denied = await shown(fake);
    expect(denied.audit.state.kind).toBe("denied");
    expect(denied.recentCodes).toContain("unauthorized");

    fake.answers.set("load_security_audit", () => {
      throw failure("protocol.feature_unavailable", "incompatible", {
        protocolLimit: { requiredMinor: 5, negotiatedMinor: 4 },
      });
    });
    const unsupported = await shown(fake);
    expect(unsupported.audit.state.kind).toBe("unsupported");

    fake.answers.set("load_security_audit", () => page(9, [1, 2]));
    const offline = await shown(fake);
    fake.answers.set("load_security_audit", () => {
      throw failure("transport.offline", "offline", { retryable: true });
    });
    await offline.audit.loadNewer();
    expect(offline.audit.state.kind).toBe("offline");
    expect(offline.audit.state.kind === "offline" && offline.audit.state.entries).toHaveLength(2);
    await offline.audit.refresh();
    expect(offline.audit.state.kind === "offline" && offline.audit.state.entries).toHaveLength(2);
  });

  it("is offered when the Plane reports no Security state", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", () => page(1, [1]));
    const system = await shown(fake, { kind: "absent" });
    expect(system.audit.state.kind).toBe("ready");
  });

  it("drops pages read before the Plane's Jet service started again", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", () => page(9, [1, 2]));
    const system = await shown(fake);
    fake.answers.set("load_system_health", () => health({ kind: "trusted" }, "4"));
    fake.answers.set("load_security_audit", () => page(1, [1]));
    await system.load();
    await settle();
    expect(system.audit.state.kind === "ready" && system.audit.state.entries.map((row) => row.sequence)).toEqual(["1"]);
    expect(fake.of("load_security_audit").map((call) => call.args.after)).toEqual([null, null]);
  });
});

describe("evidence export and a new audit period", () => {
  it("saving the evidence enables the review, which starts a new period and reloads", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", () => page(40, [39, 40]));
    const system = await shown(fake, degraded(false));
    const { audit } = system;

    fake.answers.set("export_security_audit", () => ({ kind: "saved", records: "40", fileName: "jet-audit-build-box.jsonl" }));
    fake.answers.set("load_system_health", () => health(degraded(true)));
    await audit.exportEvidence();
    await settle();
    expect(audit.exporting).toEqual({ kind: "saved", records: "40", fileName: "jet-audit-build-box.jsonl" });
    // The health read now shows the native export mark.
    expect(system.health.kind === "ready" && system.health.data.security).toEqual(degraded(true));

    fake.answers.set("prepare_recovery_action", () => epochReview());
    await audit.prepareEpoch();
    expect(audit.epoch).toEqual({ kind: "review", review: epochReview() });

    fake.answers.set("execute_recovery_action", () => ({ kind: "epoch_begun", epoch: "3" }));
    fake.answers.set("load_system_health", () => health({ kind: "trusted" }));
    const healthReads = fake.of("load_system_health").length;
    const auditReads = fake.of("load_security_audit").length;
    await audit.confirmEpoch();
    await settle();
    expect(audit.epoch).toEqual({ kind: "done", epoch: "3", planeLabel: "Build box" });
    expect(fake.of("execute_recovery_action").map((call) => call.args)).toEqual([{ planeId: REMOTE, reviewId: "review-1" }]);
    expect(fake.of("load_system_health").length).toBe(healthReads + 1);
    expect(fake.of("load_security_audit").length).toBe(auditReads + 1);
    expect(system.health.kind === "ready" && system.health.data.security.kind).toBe("trusted");
  });

  it("a canceled dialog saves nothing and a failed export says why", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", () => page(0, []));
    const system = await shown(fake, degraded(false));
    fake.answers.set("export_security_audit", () => ({ kind: "canceled" }));
    await system.audit.exportEvidence();
    expect(system.audit.exporting).toEqual({ kind: "idle" });
    expect(fake.of("load_system_health")).toHaveLength(1);

    fake.answers.set("export_security_audit", () => {
      throw failure("audit.export_failed", "internal", { retryable: true });
    });
    await system.audit.exportEvidence();
    expect(system.audit.exporting.kind).toBe("failed");
    expect(system.recentCodes).toContain("audit.export_failed");
  });

  it("an uncertain new period resends the same review, also after the dialog closes", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", () => page(0, []));
    const system = await shown(fake, degraded(true));
    const { audit } = system;
    fake.answers.set("prepare_recovery_action", () => epochReview());
    await audit.prepareEpoch();
    fake.answers.set("execute_recovery_action", () => {
      throw failure("transport.offline", "offline", { retryable: true });
    });
    await audit.confirmEpoch();
    expect(audit.epoch).toMatchObject({ kind: "retry_epoch", review: epochReview() });

    audit.closeEpoch();
    await audit.prepareEpoch();
    // Reopened on the request still waiting; no new review is prepared.
    expect(audit.epoch.kind).toBe("retry_epoch");
    expect(fake.of("prepare_recovery_action")).toHaveLength(1);

    fake.answers.set("execute_recovery_action", () => ({ kind: "epoch_begun", epoch: "3" }));
    await audit.confirmEpoch();
    expect(fake.of("execute_recovery_action").map((call) => call.args.reviewId)).toEqual(["review-1", "review-1"]);
    expect(audit.epoch.kind).toBe("done");

    // Resolved: the next request is a new review.
    audit.closeEpoch();
    await audit.prepareEpoch();
    expect(fake.of("prepare_recovery_action")).toHaveLength(2);
  });

  it("a local precondition is stale with Reload; other refusals are refused", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", () => page(0, []));
    const system = await shown(fake, degraded(true));
    const { audit } = system;
    fake.answers.set("prepare_recovery_action", () => {
      throw failure("audit.export_required", "conflict");
    });
    await audit.prepareEpoch();
    expect(audit.epoch.kind).toBe("stale");
    const reads = fake.of("load_system_health").length;
    await audit.reloadEpoch();
    await settle();
    expect(audit.epoch.kind).toBe("closed");
    expect(fake.of("load_system_health").length).toBe(reads + 1);

    fake.answers.set("prepare_recovery_action", () => epochReview("review-2"));
    await audit.prepareEpoch();
    fake.answers.set("execute_recovery_action", () => ({
      kind: "refused",
      error: failure("security.gap_unknown", "internal"),
    }));
    await audit.confirmEpoch();
    expect(audit.epoch).toMatchObject({ kind: "refused", error: { code: "security.gap_unknown" } });
  });

  it("a review open when the Plane's Jet service starts again must be made again", async () => {
    const fake = new Fake();
    fake.install();
    fake.answers.set("load_security_audit", () => page(0, []));
    const system = await shown(fake, degraded(true));
    fake.answers.set("prepare_recovery_action", () => epochReview());
    await system.audit.prepareEpoch();
    fake.answers.set("load_system_health", () => health(degraded(true), "4"));
    await system.load();
    expect(system.audit.epoch.kind).toBe("restarted");
  });
});

describe("audit copy", () => {
  it("labels decisions from an allowlist and unknown ones by code", () => {
    expect(decisionLabel("account.bound")).toBe("Account connected");
    expect(decisionLabel("conversation.forgotten")).toBe("Task forgotten");
    expect(decisionLabel("audit.epoch_begun")).toBe("New audit period started");
    expect(decisionLabel("future.decision")).toBe("Security decision (future.decision)");
    expect(decisionLabel("toString")).toBe("Security decision (toString)");
    expect(actorLabel("this_device")).toBe("This computer");
    expect(entryLine(entry(1))).toMatch(/ · Account connected · This computer · Widens access · Done$/);
  });

  it("explains every breach and each refusal without native text", () => {
    const breaches: AuditBreachKind[] = ["head_missing", "head_not_in_store", "head_diverged", "record_altered", "target_altered"];
    for (const breach of breaches) expect(breachExplanation(breach)).toMatch(/^[a-z]/);
    expect(isStaleEpochError({ code: "audit.export_required" })).toBe(true);
    expect(isStaleEpochError({ code: "security.gap_unknown" })).toBe(false);
    expect(epochRefusalText(failure("security.gap_unknown"), "Build box")).toContain("diagnostic summary");
    expect(exportFailureText(failure("audit.export_busy", "conflict"), "Build box")).toBe(
      "Jet is already saving this Plane's audit evidence.",
    );
    expect(exportFailureText(failure("unauthorized", "unauthorized"), "Build box")).toBe(
      "Only the owner of Build box can view its security audit.",
    );
    expect(savedText("1", "a.jsonl")).toBe("Saved 1 record to a.jsonl.");
  });

  it("a restore review points to the security audit now that it can be reviewed", () => {
    const lines = restoreReviewLines({
      kind: "restore_snapshot",
      reviewId: "r",
      planeLabel: "Build box",
      takenAtUnixMs: "1700000000000",
      reason: "daily",
      bytes: "4096",
    });
    expect(lines.at(-1)).toContain("review the security audit afterwards");
  });
});
