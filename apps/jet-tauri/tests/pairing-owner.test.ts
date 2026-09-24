import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { PublicError } from "../src/lib/jet/bridge";
import {
  confirmPairingRequest,
  executePairedClientChange,
  loadPairing,
  openPairingOffer,
  preparePairedClientChange,
  setPairingGate,
  type ClientChangeReview,
  type PairedClient,
  type PairingView,
  type PendingPairing,
} from "../src/lib/jet/pairing";
import type { Plane, PlanesSnapshot } from "../src/lib/jet/planes";
import {
  countdownText,
  formatFingerprint,
  normalizeAuthString,
  pairingPausedCopy,
} from "../src/lib/features/planes/model";
import { OwnerPairing } from "../src/lib/features/planes/pairing.svelte";
import { PlanesSession, type FeedHandler } from "../src/lib/features/planes/session.svelte";

const REMOTE = "0000000a-0000-4000-8000-000000000002";
const THIS_CLIENT = "00000000-0000-4000-8000-00000000000c";
const OTHER_CLIENT = "00000000-0000-4000-8000-0000000000aa";
const OFFER = "00000000-0000-4000-8000-0000000000f1";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

type Call = { command: string; args: Record<string, unknown> };
type Handler = (command: string, args: Record<string, unknown>, calls: Call[]) => unknown;

function ipc(handler: Handler): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC(async (command, args) => {
    const record = (args ?? {}) as Record<string, unknown>;
    calls.push({ command, args: record });
    return handler(command, record, calls);
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

function client(clientId: string, overrides: Partial<PairedClient> = {}): PairedClient {
  return {
    clientId,
    fingerprint: "6668 7aad f862 bd77",
    access: "enabled",
    pairedAtUnixMs: "1700000000000",
    pairingProtocol: "jet.pairing.v1",
    isThisComputer: clientId === THIS_CLIENT,
    ...overrides,
  };
}

function pending(progress: PendingPairing["progress"], overrides: Partial<PendingPairing> = {}): PendingPairing {
  return {
    offerId: OFFER,
    method: "manual_code",
    progress,
    attemptsRemaining: 5,
    openedAtUnixMs: String(Date.now()),
    expiresAtUnixMs: String(Date.now() + 120_000),
    ...overrides,
  };
}

function view(overrides: Partial<PairingView> = {}): PairingView {
  return {
    planeId: "local",
    cursor: "40",
    gate: "closed",
    pending: null,
    clients: [client(THIS_CLIENT)],
    thisClientId: THIS_CLIENT,
    mutations: { allowed: true, reason: null },
    ...overrides,
  };
}

const disclosure = {
  offerId: OFFER,
  code: "1234-5678",
  alreadyDisclosed: false,
  expiresAtUnixMs: String(Date.now() + 120_000),
  attemptsRemaining: 5,
};

function review(overrides: Partial<ClientChangeReview> = {}): ClientChangeReview {
  return {
    reviewId: "00000000-0000-4000-8000-0000000000e1",
    planeId: "local",
    planeLabel: "This computer",
    planeIdentity: null,
    clientId: OTHER_CLIENT,
    fingerprint: "1111 2222 3333 4444",
    change: "revoke",
    isThisComputer: false,
    viaThisPlaneConnection: false,
    ...overrides,
  };
}

async function settle(): Promise<void> {
  for (let index = 0; index < 10; index += 1) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

async function shown(pairing: OwnerPairing, planeId = "local"): Promise<void> {
  pairing.show(planeId);
  await settle();
}

function commands(calls: Call[]): string[] {
  return calls.map((call) => call.command);
}

describe("owner Pairing adapter", () => {
  it("sends the exact owner commands and camelCase arguments", async () => {
    const calls = ipc(() => null);
    await loadPairing(REMOTE);
    await setPairingGate(REMOTE, "open");
    await openPairingOffer(REMOTE);
    await confirmPairingRequest(REMOTE, OFFER, "482-913");
    await preparePairedClientChange(REMOTE, OTHER_CLIENT, "disable");
    await executePairedClientChange("review-1");
    expect(calls).toEqual([
      { command: "load_pairing", args: { planeId: REMOTE } },
      { command: "set_pairing_gate", args: { planeId: REMOTE, gate: "open" } },
      { command: "open_pairing_offer", args: { planeId: REMOTE } },
      {
        command: "confirm_pairing_request",
        args: { planeId: REMOTE, offerId: OFFER, authenticationString: "482-913" },
      },
      {
        command: "prepare_paired_client_change",
        args: { planeId: REMOTE, clientId: OTHER_CLIENT, change: "disable" },
      },
      { command: "execute_paired_client_change", args: { reviewId: "review-1" } },
    ]);
  });
});

describe("owner Pairing offers", () => {
  it("Pair a computer opens the gate, then an offer, and shows the code", async () => {
    let current = view();
    const calls = ipc((command, args) => {
      switch (command) {
        case "load_pairing":
          return current;
        case "set_pairing_gate":
          current = view({ gate: args.gate as "open" | "closed" });
          return current;
        case "open_pairing_offer":
          current = view({ gate: "open", pending: pending({ kind: "offered" }) });
          return disclosure;
      }
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    await pairing.pairComputer();
    await settle();
    expect(commands(calls)).toEqual(["load_pairing", "set_pairing_gate", "open_pairing_offer", "load_pairing"]);
    expect(calls[1].args).toEqual({ planeId: "local", gate: "open" });
    expect(pairing.offer).toMatchObject({ kind: "shown", code: "1234-5678", offerId: OFFER });
    expect(pairing.openedGateForThisPairing).toBe(true);
    expect(pairing.holdsCode).toBe(true);
  });

  it("an offer disclosed before shows no code and only offers a new one", async () => {
    const calls = ipc((command) => {
      if (command === "load_pairing") return view({ gate: "open", pending: pending({ kind: "offered" }) });
      if (command === "open_pairing_offer") return { ...disclosure, code: null, alreadyDisclosed: true };
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    await pairing.pairComputer();
    await settle();
    // The gate was already open: nothing opened it here.
    expect(commands(calls)).not.toContain("set_pairing_gate");
    expect(pairing.offer).toEqual({ kind: "already_disclosed", offerId: OFFER });
    expect(JSON.stringify(pairing.offer)).not.toContain("1234");
    expect(pairing.openedGateForThisPairing).toBe(false);
  });

  it("Stop and Reject always close the gate, even when it was opened elsewhere", async () => {
    const calls = ipc((command, args) => {
      if (command === "load_pairing") {
        return view({
          gate: "open",
          pending: pending({ kind: "awaiting_confirmation", clientId: OTHER_CLIENT, clientIsThisComputer: false }),
        });
      }
      if (command === "set_pairing_gate") {
        expect(args).toEqual({ planeId: "local", gate: "closed" });
        return view({ gate: "closed", pending: pending({ kind: "ended", reason: "gate_closed" }) });
      }
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    expect(pairing.confirm).toEqual({ kind: "entering", offerId: OFFER, value: "" });
    // Reject is Stop: the only way to end an offer is to close the gate.
    await pairing.stop();
    expect(commands(calls)).toEqual(["load_pairing", "set_pairing_gate"]);
    expect(pairing.offer).toEqual({ kind: "ended", reason: "gate_closed" });
    expect(pairing.confirm).toEqual({ kind: "idle" });
  });

  it("a failed Stop keeps the code and says how long it stays valid", async () => {
    const expiresAtUnixMs = String(Date.now() + 90_000);
    ipc((command, args) => {
      if (command === "load_pairing") return view({ gate: "open", pending: pending({ kind: "offered" }) });
      if (command === "open_pairing_offer") return { ...disclosure, expiresAtUnixMs };
      if (command === "set_pairing_gate" && args.gate === "closed") throw failure("transport.offline", "offline", true);
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    await pairing.pairComputer();
    await pairing.stop();
    expect(pairing.stopFailure).toMatchObject({ validUntilUnixMs: expiresAtUnixMs });
    expect(pairing.stopFailure?.error.code).toBe("transport.offline");
    expect(pairing.offer).toMatchObject({ kind: "shown", code: "1234-5678" });
  });

  it("clears the code on an ended refresh, on expiry and when the section is left", async () => {
    let current = view({ gate: "open", pending: pending({ kind: "offered" }) });
    const calls = ipc((command, args) => {
      if (command === "load_pairing") return current;
      if (command === "open_pairing_offer") return disclosure;
      if (command === "set_pairing_gate") return view({ gate: args.gate as "open" | "closed" });
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);

    await pairing.pairComputer();
    await settle();
    current = view({ gate: "open", pending: pending({ kind: "ended", reason: "too_many_attempts" }) });
    await pairing.load();
    expect(pairing.offer).toEqual({ kind: "ended", reason: "too_many_attempts" });

    current = view({ gate: "open", pending: pending({ kind: "offered" }) });
    await pairing.pairComputer();
    await settle();
    pairing.expire(Number(disclosure.expiresAtUnixMs) + 1);
    expect(pairing.offer).toEqual({ kind: "ended", reason: "expired" });

    await pairing.pairComputer();
    await settle();
    expect(pairing.holdsCode).toBe(true);
    calls.length = 0;
    pairing.leave();
    await settle();
    expect(pairing.offer).toEqual({ kind: "none" });
    expect(calls).toEqual([{ command: "set_pairing_gate", args: { planeId: "local", gate: "closed" } }]);
  });

  it("switching Planes while a code is shown asks first, stops pairing, then switches", async () => {
    const calls = ipc((command, args) => {
      if (command === "load_pairing") {
        return view({ planeId: args.planeId as string, gate: "open", pending: pending({ kind: "offered" }) });
      }
      if (command === "open_pairing_offer") return disclosure;
      if (command === "set_pairing_gate") return view({ gate: args.gate as "open" | "closed" });
      if (command === "load_plane_detail" || command === "list_planes") throw failure("transport.offline", "offline", true);
      throw new Error(`Unexpected ${command}`);
    });
    const handler: FeedHandler = { receive: () => undefined, opened: () => undefined, openFailed: () => undefined };
    const planes = new PlanesSession(handler);
    planes.snapshot = snapshot([plane("local", "This computer"), plane(REMOTE, "build@host")]);
    planes.select("local");
    planes.pairing.show("local");
    await settle();
    await planes.pairing.pairComputer();
    await settle();

    planes.select(REMOTE);
    expect(planes.pendingSwitch).toEqual({ planeId: REMOTE, focus: null });
    expect(planes.selectedPlaneId).toBe("local");
    expect(planes.pairing.holdsCode).toBe(true);

    calls.length = 0;
    await planes.confirmSwitch();
    expect(calls[0]).toEqual({ command: "set_pairing_gate", args: { planeId: "local", gate: "closed" } });
    expect(planes.pendingSwitch).toBeNull();
    expect(planes.selectedPlaneId).toBe(REMOTE);
    expect(planes.pairing.holdsCode).toBe(false);
    expect(JSON.stringify(planes.pairing.offer)).not.toContain("1234");
  });
});

describe("owner Pairing confirmation", () => {
  it("a string that does not match moves to mismatch and keeps the claim", async () => {
    const calls = ipc((command) => {
      if (command === "load_pairing") {
        return view({
          gate: "open",
          pending: pending({ kind: "awaiting_confirmation", clientId: OTHER_CLIENT, clientIsThisComputer: false }),
        });
      }
      if (command === "confirm_pairing_request") throw failure("pairing.authentication_string_mismatch", "invalid_input");
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    pairing.setConfirmValue("48 29 13");
    expect(pairing.confirm).toEqual({ kind: "entering", offerId: OFFER, value: "482-913" });
    await pairing.submitConfirm();
    expect(calls.find((call) => call.command === "confirm_pairing_request")?.args).toEqual({
      planeId: "local",
      offerId: OFFER,
      authenticationString: "482-913",
    });
    await settle();
    expect(pairing.confirm).toEqual({ kind: "mismatch", offerId: OFFER });
    pairing.setConfirmValue("482914");
    expect(pairing.confirm).toEqual({ kind: "entering", offerId: OFFER, value: "482-914" });
  });

  it("closes the gate on completion only when this flow opened it", async () => {
    for (const openedHere of [true, false]) {
      let current = view({ gate: openedHere ? "closed" : "open" });
      const calls = ipc((command, args) => {
        if (command === "load_pairing") return current;
        if (command === "set_pairing_gate") {
          current = view({ gate: args.gate as "open" | "closed", pending: current.pending, clients: current.clients });
          return current;
        }
        if (command === "open_pairing_offer") {
          current = view({ gate: "open", pending: pending({ kind: "offered" }) });
          return disclosure;
        }
        if (command === "confirm_pairing_request") {
          current = view({ gate: "open", pending: pending({ kind: "confirmed", clientId: OTHER_CLIENT }) });
          return current.pending;
        }
        throw new Error(`Unexpected ${command}`);
      });
      const pairing = new OwnerPairing(() => "This computer");
      await shown(pairing);
      await pairing.pairComputer();
      await settle();

      // pairing.claimed refresh.
      current = view({
        gate: "open",
        pending: pending({ kind: "awaiting_confirmation", clientId: OTHER_CLIENT, clientIsThisComputer: false }),
      });
      pairing.pairingEvent("local");
      await settle();
      expect(pairing.offer).toEqual({ kind: "ended", reason: "claimed" });
      pairing.setConfirmValue("482913");
      await pairing.submitConfirm();
      await settle();
      expect(pairing.confirm).toEqual({ kind: "confirmed" });

      // pairing.completed refresh: a new client row.
      calls.length = 0;
      current = view({ gate: "open", clients: [client(THIS_CLIENT), client(OTHER_CLIENT, { fingerprint: "AAAA BBBB CCCC DDDD" })] });
      pairing.pairingEvent("local");
      await settle();
      const closes = calls.filter((call) => call.command === "set_pairing_gate");
      if (openedHere) {
        expect(closes).toEqual([{ command: "set_pairing_gate", args: { planeId: "local", gate: "closed" } }]);
        expect(pairing.notice).toBe("aaaa bbbb cccc dddd is now paired. Pairing is closed again.");
      } else {
        expect(closes).toEqual([]);
        expect(pairing.notice).toBe("aaaa bbbb cccc dddd is now paired.");
      }
      expect(pairing.confirm).toEqual({ kind: "idle" });
      clearMocks();
    }
  });

  it("completes a Pair again from a client that is already listed", async () => {
    let current = view({ gate: "closed", clients: [client(THIS_CLIENT), client(OTHER_CLIENT, { access: "disabled" })] });
    const calls = ipc((command, args) => {
      if (command === "load_pairing") return current;
      if (command === "set_pairing_gate") {
        current = view({ gate: args.gate as "open" | "closed", pending: current.pending, clients: current.clients });
        return current;
      }
      if (command === "open_pairing_offer") {
        current = view({ gate: "open", pending: pending({ kind: "offered" }), clients: current.clients });
        return disclosure;
      }
      if (command === "confirm_pairing_request") {
        current = view({
          gate: "open",
          pending: pending({ kind: "confirmed", clientId: OTHER_CLIENT }),
          clients: current.clients,
        });
        return current.pending;
      }
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    await pairing.pairComputer();
    await settle();

    current = view({
      gate: "open",
      pending: pending({ kind: "awaiting_confirmation", clientId: OTHER_CLIENT, clientIsThisComputer: false }),
      clients: current.clients,
    });
    pairing.pairingEvent("local");
    await settle();
    pairing.setConfirmValue("482913");
    await pairing.submitConfirm();
    await settle();
    expect(pairing.confirm).toEqual({ kind: "confirmed" });

    // pairing.completed: no new row, the same client ID with a new key.
    calls.length = 0;
    current = view({
      gate: "open",
      pending: null,
      clients: [client(THIS_CLIENT), client(OTHER_CLIENT, { fingerprint: "AAAA BBBB CCCC DDDD", pairedAtUnixMs: "1800000000000" })],
    });
    pairing.pairingEvent("local");
    await settle();
    expect(pairing.confirm).toEqual({ kind: "idle" });
    expect(pairing.notice).toBe("aaaa bbbb cccc dddd is paired again. Pairing is closed again.");
    expect(calls.filter((call) => call.command === "set_pairing_gate")).toEqual([
      { command: "set_pairing_gate", args: { planeId: "local", gate: "closed" } },
    ]);
  });

  it("a claim that disappears without a paired client returns to idle", async () => {
    let current = view({
      gate: "open",
      clients: [client(THIS_CLIENT), client(OTHER_CLIENT)],
      pending: pending({ kind: "awaiting_confirmation", clientId: OTHER_CLIENT, clientIsThisComputer: false }),
    });
    ipc((command) => {
      if (command === "load_pairing") return current;
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    expect(pairing.confirm.kind).toBe("entering");
    current = view({ gate: "open", clients: [client(THIS_CLIENT), client(OTHER_CLIENT)], pending: null });
    pairing.pairingEvent("local");
    await settle();
    expect(pairing.confirm).toEqual({ kind: "idle" });
    expect(pairing.notice).toBeNull();
  });

  it("paused mutations disable every pairing change with the reason", async () => {
    const calls = ipc((command) => {
      if (command === "load_pairing") {
        return view({
          mutations: { allowed: false, reason: "security_degraded" },
          pending: pending({ kind: "awaiting_confirmation", clientId: OTHER_CLIENT, clientIsThisComputer: false }),
          clients: [client(THIS_CLIENT), client(OTHER_CLIENT)],
        });
      }
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "build@host");
    await shown(pairing, REMOTE);
    expect(pairing.canMutate).toBe(false);
    expect(pairing.blockedReason).toBe(
      "build@host can't vouch for its security record, so pairing changes are paused. Existing paired computers keep working.",
    );
    await pairing.pairComputer();
    pairing.setConfirmValue("482913");
    await pairing.submitConfirm();
    await pairing.prepareChange(OTHER_CLIENT, "disable");
    await pairing.prepareChange(OTHER_CLIENT, "revoke");
    expect(commands(calls)).toEqual(["load_pairing"]);
    expect(pairingPausedCopy("store_read_only", "build@host")).toBe(
      "build@host is in read-only recovery, so pairing changes are paused until its data is restored.",
    );
  });

  it("a stale Pairing view keeps the last list and disables changes", async () => {
    let fail = false;
    ipc((command) => {
      if (command === "load_pairing") {
        if (fail) throw failure("transport.offline", "offline", true);
        return view({ clients: [client(THIS_CLIENT), client(OTHER_CLIENT)] });
      }
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    fail = true;
    await pairing.load();
    expect(pairing.state.kind).toBe("stale");
    expect(pairing.view?.clients).toHaveLength(2);
    expect(pairing.canMutate).toBe(false);
    expect(pairing.blockedReason).toContain("earlier check");
  });

  it("a late reply for another Plane is discarded", async () => {
    let release: (value: PairingView) => void = () => undefined;
    ipc((command, args) => {
      if (command === "load_pairing" && args.planeId === "local") {
        return new Promise<PairingView>((resolve) => (release = resolve));
      }
      if (command === "load_pairing") return view({ planeId: REMOTE, gate: "open" });
      throw new Error(`Unexpected ${command}`);
    });
    const pairing = new OwnerPairing(() => "Plane");
    pairing.show("local");
    await settle();
    pairing.show(REMOTE);
    await settle();
    release(view({ planeId: "local", gate: "closed" }));
    await settle();
    expect(pairing.view?.planeId).toBe(REMOTE);
    expect(pairing.view?.gate).toBe("open");
  });
});

describe("paired client changes", () => {
  function changes(receipts: Array<unknown>, prepared: ClientChangeReview) {
    return ipc((command) => {
      if (command === "load_pairing") return view({ clients: [client(THIS_CLIENT), client(OTHER_CLIENT)] });
      if (command === "prepare_paired_client_change") return prepared;
      if (command === "execute_paired_client_change") {
        const next = receipts.shift();
        if (next instanceof Error || (next && typeof next === "object" && "category" in next)) throw next;
        return next;
      }
      throw new Error(`Unexpected ${command}`);
    });
  }

  it("an uncertain change is retried with the same review", async () => {
    const calls = changes(
      [failure("transport.offline", "offline", true), { kind: "applied", client: null, revoked: true }],
      review(),
    );
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    await pairing.prepareChange(OTHER_CLIENT, "revoke");
    expect(pairing.change).toMatchObject({ kind: "review", review: { reviewId: review().reviewId } });
    await pairing.executeChange();
    expect(pairing.change).toMatchObject({ kind: "uncertain", error: { code: "transport.offline" } });
    // Another change cannot start while the outcome is unknown.
    await pairing.prepareChange(OTHER_CLIENT, "disable");
    await pairing.executeChange();
    await settle();
    const executed = calls.filter((call) => call.command === "execute_paired_client_change");
    expect(executed.map((call) => call.args.reviewId)).toEqual([review().reviewId, review().reviewId]);
    expect(calls.filter((call) => call.command === "prepare_paired_client_change")).toHaveLength(1);
    expect(pairing.change).toMatchObject({ kind: "done", receipt: { kind: "applied", revoked: true } });
    expect(pairing.notice).toBe("1111 2222 3333 4444 can no longer control This computer.");
  });

  it("a refused receipt returns to no change and shows the refusal", async () => {
    changes([{ kind: "refused", error: failure("security.audit_degraded") }], review({ change: "disable" }));
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    await pairing.prepareChange(OTHER_CLIENT, "disable");
    await pairing.executeChange();
    expect(pairing.change).toEqual({ kind: "none" });
    expect(pairing.actionError?.code).toBe("security.audit_degraded");
  });

  it("enable runs right after it is prepared", async () => {
    const calls = changes(
      [{ kind: "applied", client: client(OTHER_CLIENT), revoked: false }],
      review({ change: "enable" }),
    );
    const pairing = new OwnerPairing(() => "This computer");
    await shown(pairing);
    await pairing.prepareChange(OTHER_CLIENT, "enable");
    expect(commands(calls).slice(1, 3)).toEqual(["prepare_paired_client_change", "execute_paired_client_change"]);
    expect(pairing.change).toMatchObject({ kind: "done", receipt: { kind: "applied" } });
  });

  it("an applied-unverified self change shows its outcome and offers no retry", async () => {
    const calls = changes(
      [{ kind: "applied_unverified", change: "revoke" }],
      review({ planeId: REMOTE, planeLabel: "build@host", isThisComputer: true, viaThisPlaneConnection: true }),
    );
    const pairing = new OwnerPairing(() => "build@host");
    await shown(pairing, REMOTE);
    await pairing.prepareChange(THIS_CLIENT, "revoke");
    await pairing.executeChange();
    expect(pairing.change).toMatchObject({ kind: "done", receipt: { kind: "applied_unverified", change: "revoke" } });
    // Retrying is not possible from a settled receipt.
    await pairing.executeChange();
    expect(calls.filter((call) => call.command === "execute_paired_client_change")).toHaveLength(1);
    expect(pairing.changeInFlight).toBe(false);
  });
});

describe("owner Pairing copy helpers", () => {
  it("groups typed strings and fingerprints for display only", () => {
    expect(normalizeAuthString("48a2 91-3x")).toBe("482-913");
    expect(normalizeAuthString("48")).toBe("48");
    expect(normalizeAuthString("4829134")).toBe("482-913");
    expect(formatFingerprint("6668-7AAD-F862-BD77")).toBe("6668 7aad f862 bd77");
  });

  it("counts down in coarse text", () => {
    expect(countdownText("120000", 0)).toBe("About 2 minutes left");
    expect(countdownText("75000", 0)).toBe("About a minute left");
    expect(countdownText("25000", 0)).toBe("About 30 seconds left");
    expect(countdownText("1000", 5000)).toBe("Expired");
  });
});

function plane(planeId: string, label: string): Plane {
  return {
    planeId,
    kind: planeId === "local" ? "local" : "remote",
    label,
    planeIdentity: null,
    connection: { state: "online" },
    coreVersion: "0.2.0",
    credential: planeId === "local" ? null : "durable",
    security: "trusted",
    store: "serving",
    features: [],
    protocol: { exact: null, atLeast: 37, atMost: null },
  };
}

function snapshot(planes: Plane[]): PlanesSnapshot {
  return {
    planes,
    identity: { clientId: THIS_CLIENT, key: "unknown", fingerprint: null, notice: null },
    restoredSelection: null,
    notice: null,
    maximumRemotePlanes: 16,
  };
}
