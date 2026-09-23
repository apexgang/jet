import { describe, expect, it } from "vitest";

import {
  RECENT_CODES_LIMIT,
  diagnosticSummary,
  isStableCode,
  withRecentCode,
} from "../src/lib/features/system/model";
import type { SystemHealth } from "../src/lib/jet/system";

const PLANE = "0000000a-0000-4000-8000-000000000002";

function health(overrides: Partial<SystemHealth> = {}): SystemHealth {
  return {
    planeId: PLANE,
    planeLabel: "Alex's build box",
    service: { coreVersion: "1.43.0", daemonStarts: "3", startedAtUnixMs: "1700000000000" },
    app: { version: "0.1.0", supportedProtocol: "1.43" },
    protocol: { exact: null, atLeast: 38, atMost: null },
    platform: "linux · x86_64",
    tools: [
      { tool: "git", label: "Git", version: "git version 2.45.0 (/usr/bin/git)" },
      { tool: "git_lfs", label: "Git LFS", version: null },
    ],
    crafts: [{ id: "/home/alex/crafts/private-craft", version: "1.0.0", harnesses: ["codex"] }],
    credentialStore: { state: "locked", kind: "secret_service" },
    degraded: [{ kind: "credential_store_locked", label: "Secure storage is locked", target: null }],
    recovery: { kind: "serving", snapshotCount: 4, ledger: { kind: "verified", deletions: "12" } },
    security: { kind: "degraded", breach: "record_altered", breachSequence: "41", epoch: "2" },
    storage: { disposableMiB: 2048 },
    retention: { graceDays: 30 },
    issues: [],
    ...overrides,
  };
}

describe("diagnostic summary", () => {
  it("is built from allowlisted fields only", () => {
    const summary = diagnosticSummary(health(), ["storage.disk_pressure", "recovery.read_only"]);
    expect(summary.split("\n")).toEqual([
      "Jet for Linux diagnostic summary",
      "App version: 0.1.0",
      "Supported protocol: 1.43",
      "Negotiated protocol: at least 1.38",
      "Jet service version: 1.43.0",
      "Jet service starts: 3",
      "Platform: linux x86_64",
      "Tools: git present, git_lfs missing",
      "Crafts installed: 1",
      "Secure storage: locked",
      "Degraded: credential_store_locked",
      "Store: serving, 4 recovery snapshots",
      "Deletion ledger: verified, 12 deletions",
      "Security audit: degraded (record_altered, epoch 2)",
      "Temporary file budget: 2048 MiB",
      "Jet Trash grace period: 30 days",
      "Recent error codes, oldest first: storage.disk_pressure, recovery.read_only",
    ]);
  });

  it("leaves out identifiers, paths, labels and native text", () => {
    const summary = diagnosticSummary(
      health({
        planeLabel: "/home/alex/secret-plane",
        service: { coreVersion: "1.43.0 built in /home/alex/src", daemonStarts: "3", startedAtUnixMs: "1" },
        platform: "linux /etc/os-release · x86_64",
        protocol: { exact: 43, atLeast: 43, atMost: 43 },
        recovery: { kind: "read_only", reason: "integrity_check_failed", snapshotCount: 0, ledger: { kind: "corrupt" } },
        issues: [
          {
            section: "capabilities",
            error: {
              category: "internal",
              code: `plane ${PLANE} failed`,
              message: "at /home/alex/.jet/jetd.sock",
              retryable: false,
              recoveryActions: [],
              restart: null,
              revisionConflict: null,
              protocolLimit: null,
              planeId: PLANE,
            },
          },
        ],
      }),
      ["transport.offline", `/home/alex/${PLANE}`, "Could not open the file"],
    );
    for (const secret of [PLANE, "/home/alex", "secret-plane", "private-craft", "/usr/bin/git", "2.45.0", "os-release", "jetd.sock", "Alex", "Could not"]) {
      expect(summary, secret).not.toContain(secret);
    }
    expect(summary).toContain("Jet service version: unrecognized");
    expect(summary).toContain("Platform: unrecognized");
    expect(summary).toContain("Negotiated protocol: 1.43");
    expect(summary).toContain("Store: read-only (integrity_check_failed), 0 recovery snapshots");
    expect(summary).toContain("Deletion ledger: corrupt");
    expect(summary).toContain("Recent error codes, oldest first: transport.offline");
  });

  it("names a section that did not load by its code only", () => {
    const summary = diagnosticSummary(
      health({
        platform: null,
        tools: [],
        credentialStore: null,
        security: { kind: "absent" },
        recovery: { kind: "unsupported" },
        storage: { disposableMiB: null },
        issues: [
          {
            section: "capabilities",
            error: {
              category: "unavailable",
              code: "capability.observation_failed",
              message: "Daemon text",
              retryable: true,
              recoveryActions: [],
              restart: null,
              revisionConflict: null,
              protocolLimit: null,
              planeId: PLANE,
            },
          },
        ],
      }),
      [],
    );
    expect(summary).toContain("Platform: not reported");
    expect(summary).toContain("Secure storage: not reported");
    expect(summary).toContain("Security audit: not reported");
    expect(summary).toContain("Store: recovery not reported");
    expect(summary).toContain("Temporary file budget: not read");
    expect(summary).toContain("Not loaded: capabilities (capability.observation_failed)");
    expect(summary).not.toContain("Daemon text");
    expect(summary).toContain("Recent error codes, oldest first: none");
  });

  it("still gives app-side facts before health loads", () => {
    expect(diagnosticSummary(null, ["transport.offline"])).toBe(
      "Jet for Linux diagnostic summary\nPlane health: not loaded\nRecent error codes, oldest first: transport.offline",
    );
  });

  it("keeps the newest 20 stable codes, oldest first", () => {
    let codes: string[] = [];
    for (let index = 0; index < 25; index++) codes = withRecentCode(codes, { code: `test.code_${index}` });
    codes = withRecentCode(codes, { code: "Not a code" });
    expect(codes).toHaveLength(RECENT_CODES_LIMIT);
    expect(codes[0]).toBe("test.code_5");
    expect(codes.at(-1)).toBe("test.code_24");
    expect(isStableCode("storage.disk_pressure")).toBe(true);
    expect(isStableCode("storage.disk pressure")).toBe(false);
    expect(isStableCode(`plane.${"x".repeat(64)}`)).toBe(false);
  });
});
