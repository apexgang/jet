import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { PublicError } from "../src/lib/jet/bridge";
import type { ClientChangeReview, PairingView } from "../src/lib/jet/pairing";
import {
  addRemotePlane,
  cancelRemotePairing,
  claimRemotePairing,
  completeRemotePairing,
  forgetRemotePlane,
  repairRemotePlane,
  type Enrollment,
  type Plane,
  type PlanesSnapshot,
} from "../src/lib/jet/planes";
import { PlaneEnrollment } from "../src/lib/features/planes/enrollment.svelte";
import {
  enrollmentFailureCopy,
  needsPairing,
  normalizeManualCode,
  onlyForget,
  planeStatus,
} from "../src/lib/features/planes/model";
import { PlanesSession, type FeedHandler } from "../src/lib/features/planes/session.svelte";

const REMOTE = "0000000a-0000-4000-8000-000000000002";
const DRAFT = "00000000-0000-4000-8000-0000000000d1";
const TICKET = "00000000-0000-4000-8000-0000000000e1";
const THIS_CLIENT = "00000000-0000-4000-8000-00000000000c";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

type Call = { command: string; args: Record<string, unknown> };
type Handler = (command: string, args: Record<string, unknown>) => unknown;

function ipc(handler: Handler): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC(async (command, args) => {
    const record = (args ?? {}) as Record<string, unknown>;
    calls.push({ command, args: record });
    return handler(command, record);
  });
  return calls;
}

function failure(code: string, category = "conflict", retryable = false): PublicError {
  return {
    category,
    code,
    message: "Jet could not use that request.",
    retryable,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: null,
  };
}

function plane(overrides: Partial<Plane> = {}): Plane {
  return {
    planeId: REMOTE,
    kind: "remote",
    label: "alice@build-box",
    planeIdentity: "0000abcd-0000-4000-8000-000000000001",
    connection: { state: "online" },
    coreVersion: "0.3.0",
    credential: "durable",
    security: "trusted",
    store: "serving",
    features: [],
    protocol: { exact: null, atLeast: 37, atMost: null },
    ...overrides,
  };
}

const LOCAL: Plane = plane({ planeId: "local", kind: "local", label: "This computer", credential: null });

function snapshot(planes: Plane[] = [LOCAL, plane()]): PlanesSnapshot {
  return {
    planes,
    identity: { clientId: THIS_CLIENT, key: "present", fingerprint: "6668 7aad f862 bd77" },
    restoredSelection: null,
    notice: null,
    maximumRemotePlanes: 16,
  };
}

const enrollment: Enrollment = {
  ticketId: TICKET,
  destination: "alice@build-box",
  planeIdentity: "0000abcd",
  authenticationString: "482-913",
  confirmByUnixMs: String(Date.now() + 120_000),
  sessionOnly: false,
};

async function settle(): Promise<void> {
  for (let index = 0; index < 10; index += 1) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

function commands(calls: Call[]): string[] {
  return calls.map((call) => call.command);
}

describe("enrollment adapter", () => {
  it("sends the exact enrollment commands and camelCase arguments", async () => {
    const calls = ipc(() => null);
    await addRemotePlane("alice@build-box");
    await addRemotePlane("alice@build-box", true);
    await repairRemotePlane(REMOTE);
    await claimRemotePairing(DRAFT, "1234-5678");
    await completeRemotePairing(TICKET);
    await cancelRemotePairing(DRAFT);
    await forgetRemotePlane(REMOTE);
    expect(calls).toEqual([
      { command: "add_remote_plane", args: { destination: "alice@build-box", sessionOnly: false } },
      { command: "add_remote_plane", args: { destination: "alice@build-box", sessionOnly: true } },
      { command: "repair_remote_plane", args: { planeId: REMOTE, sessionOnly: false } },
      { command: "claim_remote_pairing", args: { draftId: DRAFT, code: "1234-5678" } },
      { command: "complete_remote_pairing", args: { ticketId: TICKET } },
      { command: "cancel_remote_pairing", args: { id: DRAFT } },
      { command: "forget_remote_plane", args: { planeId: REMOTE } },
    ]);
  });
});

describe("Add a Plane wizard", () => {
  it("goes straight to done when this computer is already paired", async () => {
    ipc((command) => {
      if (command === "add_remote_plane") return { kind: "connected", plane: plane() };
      throw new Error(`Unexpected ${command}`);
    });
    const paired = vi.fn();
    const wizard = new PlaneEnrollment(paired);
    wizard.startAdd();
    expect(wizard.state).toEqual({ step: "destination", value: "", error: null });
    wizard.setDestination("  alice@build-box ");
    await wizard.submitDestination();
    expect(wizard.state).toMatchObject({ step: "done", repaired: false });
    expect(paired).toHaveBeenCalledWith(plane(), false);
  });

  it("claims the code, stays on confirm until the Plane confirms, then finishes", async () => {
    let completions = 0;
    const calls = ipc((command) => {
      switch (command) {
        case "add_remote_plane":
          return { kind: "pairing_required", draftId: DRAFT, destination: "alice@build-box" };
        case "claim_remote_pairing":
          return enrollment;
        case "complete_remote_pairing":
          completions += 1;
          if (completions === 1) throw failure("pairing.not_confirmed");
          return plane();
      }
      throw new Error(`Unexpected ${command}`);
    });
    const paired = vi.fn();
    const wizard = new PlaneEnrollment(paired);
    wizard.startAdd();
    wizard.setDestination("alice@build-box");
    await wizard.submitDestination();
    expect(wizard.state).toMatchObject({ step: "code", draftId: DRAFT, destination: "alice@build-box" });
    wizard.setCode(normalizeManualCode("1234 5678"));
    await wizard.submitCode();
    expect(wizard.state).toMatchObject({ step: "confirm", enrollment, error: null });

    await wizard.finish();
    expect(wizard.state).toMatchObject({ step: "confirm", error: { code: "pairing.not_confirmed" } });
    expect(paired).not.toHaveBeenCalled();

    await wizard.finish();
    expect(wizard.state).toMatchObject({ step: "done", plane: plane() });
    expect(paired).toHaveBeenCalledOnce();
    expect(commands(calls)).toEqual([
      "add_remote_plane",
      "claim_remote_pairing",
      "complete_remote_pairing",
      "complete_remote_pairing",
    ]);
    expect(calls[1].args).toEqual({ draftId: DRAFT, code: "1234-5678" });
    expect(calls[2].args).toEqual({ ticketId: TICKET });
  });

  it("shows secure-storage instructions and pairs for this session only on request", async () => {
    const calls = ipc((command, args) => {
      if (command === "add_remote_plane") {
        if (args.sessionOnly) return { kind: "pairing_required", draftId: DRAFT, destination: "build-box" };
        throw failure("identity.secret_store_unavailable", "unavailable", true);
      }
      throw new Error(`Unexpected ${command}`);
    });
    const wizard = new PlaneEnrollment(() => undefined);
    wizard.startAdd();
    wizard.setDestination("build-box");
    await wizard.submitDestination();
    expect(wizard.state).toMatchObject({
      step: "secure_storage",
      destination: "build-box",
      error: { code: "identity.secret_store_unavailable" },
    });
    await wizard.checkAgain();
    expect(wizard.state.step).toBe("secure_storage");
    await wizard.pairForSessionOnly();
    expect(wizard.state).toMatchObject({ step: "code", draftId: DRAFT });
    expect(calls.map((call) => call.args.sessionOnly)).toEqual([false, false, true]);
  });

  it("offers no session-only pairing for a locked keyring", async () => {
    const calls = ipc(() => {
      throw failure("identity.secret_store_locked", "unavailable", true);
    });
    const wizard = new PlaneEnrollment(() => undefined);
    wizard.startAdd();
    wizard.setDestination("build-box");
    await wizard.submitDestination();
    expect(wizard.state).toMatchObject({ step: "secure_storage", error: { code: "identity.secret_store_locked" } });
    await wizard.pairForSessionOnly();
    expect(calls).toHaveLength(1);
    expect(wizard.state.step).toBe("secure_storage");
  });

  it("Pair again opens at checking with the destination fixed", async () => {
    let resolve: (value: unknown) => void = () => undefined;
    const calls = ipc((command) => {
      if (command === "repair_remote_plane") return new Promise((done) => (resolve = done));
      throw new Error(`Unexpected ${command}`);
    });
    const wizard = new PlaneEnrollment(() => undefined);
    const repairing = wizard.startRepair(REMOTE, "alice@build-box");
    await settle();
    expect(wizard.state).toEqual({ step: "checking", destination: "alice@build-box", repairOf: REMOTE });
    resolve({ kind: "pairing_required", draftId: DRAFT, destination: "alice@build-box" });
    await repairing;
    expect(wizard.state).toMatchObject({ step: "code", repairOf: REMOTE, destination: "alice@build-box" });
    expect(calls[0]).toEqual({ command: "repair_remote_plane", args: { planeId: REMOTE, sessionOnly: false } });
  });

  it("cancel during claiming ignores the late claim and drops it natively", async () => {
    let resolve: (value: unknown) => void = () => undefined;
    const calls = ipc((command) => {
      switch (command) {
        case "add_remote_plane":
          return { kind: "pairing_required", draftId: DRAFT, destination: "build-box" };
        case "claim_remote_pairing":
          return new Promise((done) => (resolve = done));
        case "cancel_remote_pairing":
          return null;
      }
      throw new Error(`Unexpected ${command}`);
    });
    const wizard = new PlaneEnrollment(() => undefined);
    wizard.startAdd();
    wizard.setDestination("build-box");
    await wizard.submitDestination();
    wizard.setCode("12345678");
    const claiming = wizard.submitCode();
    await settle();
    expect(wizard.state.step).toBe("claiming");
    wizard.cancel();
    expect(wizard.state).toEqual({ step: "closed" });
    resolve(enrollment);
    await claiming;
    await settle();
    expect(wizard.state).toEqual({ step: "closed" });
    const cancelled = calls.filter((call) => call.command === "cancel_remote_pairing").map((call) => call.args.id);
    expect(cancelled).toEqual([DRAFT, TICKET]);
  });

  it("never ends the confirm step on the countdown; an expired offer returns to the code", async () => {
    vi.useFakeTimers();
    ipc((command) => {
      switch (command) {
        case "add_remote_plane":
          return { kind: "pairing_required", draftId: DRAFT, destination: "build-box" };
        case "claim_remote_pairing":
          return { ...enrollment, confirmByUnixMs: String(Date.now() + 1_000) };
        case "complete_remote_pairing":
          throw failure("pairing.offer_expired");
      }
      throw new Error(`Unexpected ${command}`);
    });
    const wizard = new PlaneEnrollment(() => undefined);
    wizard.startAdd();
    wizard.setDestination("build-box");
    await wizard.submitDestination();
    wizard.setCode("12345678");
    await wizard.submitCode();
    vi.advanceTimersByTime(10 * 60_000);
    expect(wizard.state.step).toBe("confirm");
    await wizard.finish();
    expect(wizard.state).toMatchObject({
      step: "code",
      draftId: DRAFT,
      value: "",
      error: { code: "pairing.offer_expired" },
    });
  });

  it("a wrong code stays on the code step and an unknown address stays on the first", async () => {
    ipc((command, args) => {
      if (command === "add_remote_plane") {
        if (args.destination === "-bad") throw failure("plane.destination_invalid", "invalid_input");
        return { kind: "pairing_required", draftId: DRAFT, destination: "build-box" };
      }
      if (command === "claim_remote_pairing") throw failure("pairing.secret_rejected", "invalid_input");
      throw new Error(`Unexpected ${command}`);
    });
    const wizard = new PlaneEnrollment(() => undefined);
    wizard.startAdd();
    wizard.setDestination("-bad");
    await wizard.submitDestination();
    expect(wizard.state).toMatchObject({ step: "destination", value: "-bad", error: { code: "plane.destination_invalid" } });
    wizard.setDestination("build-box");
    await wizard.submitDestination();
    wizard.setCode("12345678");
    await wizard.submitCode();
    expect(wizard.state).toMatchObject({ step: "code", error: { code: "pairing.secret_rejected" } });
  });

  it("maps each failure code to its copy", () => {
    const copy = (code: string, category = "conflict") => enrollmentFailureCopy(failure(code, category), "box");
    expect(copy("ssh.connection_failed")).toContain("`ssh box` works in a terminal");
    expect(copy("ssh.client_missing")).toBe("OpenSSH isn't installed on this computer.");
    expect(copy("plane.jetd_missing")).toContain("Non-interactive SSH sessions often skip your shell profile.");
    expect(copy("plane.jetd_unavailable")).toBe("Jet isn't running on box.");
    expect(copy("plane.handshake_refused")).toContain("Nothing was sent.");
    expect(copy("pairing.secret_rejected")).toContain("That code is wrong.");
    for (const code of ["pairing.offer_expired", "pairing.offer_ended", "enrollment.ticket_expired"]) {
      expect(copy(code)).toBe("This pairing request expired. Get a new code on box.");
    }
    for (const code of ["pairing.none_offered", "pairing.gate_closed"]) {
      expect(copy(code)).toBe("That code is no longer valid. Get a new one on box.");
    }
    expect(copy("enrollment.transcript_invalid")).toContain("reply didn't check out");
    expect(copy("plane.already_registered")).toContain("same Plane");
    expect(copy("plane.identity_changed")).toContain("now reaches a different Plane");
    expect(copy("connection.limit", "unavailable")).toContain("too many connections");
    expect(copy("security.audit_degraded")).toContain("security record needs attention");
    expect(copy("recovery.read_only", "unavailable")).toContain("read-only recovery");
    expect(copy("protocol.remote_auth_required", "incompatible")).toContain("Update Jet there.");
    expect(copy("pairing.not_confirmed")).toBe("box hasn't confirmed yet.");
  });

  it("classifies which failures Pair again or only Forget can fix", () => {
    expect(needsPairing(failure("connection.unauthorized", "unauthorized"))).toBe(true);
    expect(needsPairing(failure("identity.key_missing"))).toBe(true);
    expect(needsPairing(failure("identity.session_ended"))).toBe(true);
    expect(needsPairing(failure("transport.offline", "offline", true))).toBe(false);
    expect(onlyForget(failure("plane.duplicates_local"))).toBe(true);
    expect(onlyForget(failure("plane.identity_changed"))).toBe(true);
    expect(planeStatus({ ...plane(), connection: { state: "failed", error: failure("identity.key_missing") } }).text).toBe(
      "Needs pairing",
    );
    expect(normalizeManualCode("12a34-5678 9")).toBe("1234-5678");
  });
});

describe("revoke, then forget", () => {
  const review: ClientChangeReview = {
    reviewId: "00000000-0000-4000-8000-0000000000f7",
    planeId: REMOTE,
    planeLabel: "alice@build-box",
    planeIdentity: "0000abcd-0000-4000-8000-000000000001",
    clientId: THIS_CLIENT,
    fingerprint: "6668 7aad f862 bd77",
    change: "revoke",
    isThisComputer: true,
    viaThisPlaneConnection: true,
  };

  const pairingView: PairingView = {
    planeId: REMOTE,
    cursor: "4",
    gate: "closed",
    pending: null,
    clients: [
      {
        clientId: THIS_CLIENT,
        fingerprint: "6668 7aad f862 bd77",
        access: "enabled",
        pairedAtUnixMs: "1700000000000",
        pairingProtocol: "jet.pairing.v1",
        isThisComputer: true,
      },
    ],
    thisClientId: THIS_CLIENT,
    mutations: { allowed: true, reason: null },
  };

  async function session(execute: () => unknown): Promise<{ planes: PlanesSession; calls: Call[]; forgotten: string[] }> {
    const forgotten: string[] = [];
    const calls = ipc((command) => {
      switch (command) {
        case "list_planes":
          return snapshot();
        case "load_pairing":
          return pairingView;
        case "load_plane_detail":
          return { plane: plane(), platform: null, harnesses: [], crafts: [], degraded: [], missingTools: [], issues: [] };
        case "prepare_paired_client_change":
          return review;
        case "execute_paired_client_change":
          return execute();
        case "forget_remote_plane":
          return snapshot([LOCAL]);
      }
      throw new Error(`Unexpected ${command}`);
    });
    const handler: FeedHandler = {
      receive: () => undefined,
      opened: () => undefined,
      openFailed: () => undefined,
      planeForgotten: (planeId) => forgotten.push(planeId),
    };
    const planes = new PlanesSession(handler);
    await planes.refresh();
    planes.select(REMOTE);
    planes.pairing.show(REMOTE);
    await settle();
    return { planes, calls, forgotten };
  }

  it("prepares, executes the revoke, then forgets the Plane", async () => {
    const { planes, calls, forgotten } = await session(() => ({ kind: "applied_unverified", change: "revoke" }));
    await planes.revokeThenForget(REMOTE);
    expect(planes.pairing.change).toMatchObject({ kind: "review", review });
    await planes.pairing.executeChange();
    await settle();
    const flow = commands(calls).filter((command) =>
      ["prepare_paired_client_change", "execute_paired_client_change", "forget_remote_plane"].includes(command),
    );
    expect(flow).toEqual(["prepare_paired_client_change", "execute_paired_client_change", "forget_remote_plane"]);
    expect(calls.find((call) => call.command === "prepare_paired_client_change")?.args).toEqual({
      planeId: REMOTE,
      clientId: THIS_CLIENT,
      change: "revoke",
    });
    expect(forgotten).toEqual([REMOTE]);
    expect(planes.planes.map((candidate) => candidate.planeId)).toEqual(["local"]);
    expect(planes.selectedPlaneId).toBe("local");
  });

  it("stops before forgetting when the revoke is refused", async () => {
    const { planes, calls, forgotten } = await session(() => ({
      kind: "refused",
      error: failure("security.audit_degraded"),
    }));
    await planes.revokeThenForget(REMOTE);
    await planes.pairing.executeChange();
    await settle();
    expect(commands(calls)).not.toContain("forget_remote_plane");
    expect(forgotten).toEqual([]);
    expect(planes.pairing.actionError?.code).toBe("security.audit_degraded");
    // Plain Forget stays available.
    await planes.forget(REMOTE);
    expect(forgotten).toEqual([REMOTE]);
  });

  it("stops before forgetting when the outcome is uncertain", async () => {
    const { planes, calls } = await session(() => {
      throw failure("transport.offline", "offline", true);
    });
    await planes.revokeThenForget(REMOTE);
    await planes.pairing.executeChange();
    await settle();
    expect(commands(calls)).not.toContain("forget_remote_plane");
    expect(planes.pairing.change.kind).toBe("uncertain");
  });
});
