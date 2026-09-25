import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";

import type { PublicError } from "../src/lib/jet/bridge";
import {
  executeLocalServiceRollback,
  isProvisioning,
  loadLocalService,
  prepareLocalServiceRollback,
  repairLocalService,
  watchLocalService,
  type LocalServiceView,
} from "../src/lib/jet/local-service";
import { LocalServiceSession } from "../src/lib/features/system/local-service.svelte";
import {
  actionText,
  channelText,
  managerText,
  phaseText,
  provisioningText,
  rollbackLines,
  rollbackRefusedNote,
  serviceErrorText,
  serviceProblem,
} from "../src/lib/features/system/service-model";
import { serviceView } from "./support/service";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function failure(code: string, category = "unavailable"): PublicError {
  return {
    category,
    code,
    message: `${code} message`,
    retryable: true,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: null,
  };
}

async function flush() {
  for (let tick = 0; tick < 20; tick += 1) await Promise.resolve();
}

type Handler = (command: string, args: Record<string, unknown>) => unknown;

function ipc(handler: Handler) {
  const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
  let channel: Channel<LocalServiceView> | null = null;
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC(async (command, args) => {
    const { onChange, ...plain } = (args ?? {}) as Record<string, unknown>;
    calls.push({ command, args: plain });
    if (command === "watch_local_service") channel = onChange as Channel<LocalServiceView>;
    return handler(command, plain);
  });
  return { calls, push: (view: LocalServiceView) => channel?.onmessage(view) };
}

describe("local service adapter", () => {
  it("names each command and passes only a review ID", async () => {
    const { calls } = ipc((command) => {
      if (command === "prepare_local_service_rollback") {
        return { reviewId: "r1", currentVersion: "0.2.0", previousVersion: "0.1.0" };
      }
      return serviceView();
    });
    await loadLocalService();
    await repairLocalService();
    expect(await prepareLocalServiceRollback()).toEqual({
      reviewId: "r1",
      currentVersion: "0.2.0",
      previousVersion: "0.1.0",
    });
    await executeLocalServiceRollback("r1");
    expect(calls).toEqual([
      { command: "load_local_service", args: {} },
      { command: "repair_local_service", args: {} },
      { command: "prepare_local_service_rollback", args: {} },
      { command: "execute_local_service_rollback", args: { reviewId: "r1" } },
    ]);
  });

  it("watches through a channel and resolves with the view now", async () => {
    const { push } = ipc(() => serviceView({ phase: "checking" }));
    const seen: LocalServiceView[] = [];
    const initial = await watchLocalService((view) => seen.push(view));
    expect(initial.phase).toBe("checking");
    push(serviceView({ phase: "installing" }));
    expect(seen.map((view) => view.phase)).toEqual(["installing"]);
  });

  it("knows which phases are provisioning", () => {
    expect(["checking", "installing", "updating", "starting"].every((phase) => isProvisioning(phase as never))).toBe(
      true,
    );
    expect(["running", "stopped", "not_installed", "failed"].some((phase) => isProvisioning(phase as never))).toBe(
      false,
    );
  });
});

describe("LocalServiceSession", () => {
  it("follows the watcher; a pushed view is never replaced by the older initial one", async () => {
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => (release = resolve));
    const { push } = ipc(async (command) => {
      if (command === "watch_local_service") {
        await held;
        return serviceView({ revision: 1, phase: "checking" });
      }
      throw new Error(command);
    });
    const changes: Array<[string, string | null]> = [];
    const session = new LocalServiceSession((view, previous) => changes.push([view.phase, previous?.phase ?? null]));
    const started = session.start();
    await flush();
    push(serviceView({ revision: 2, phase: "installing" }));
    release();
    await started;
    expect(session.view?.phase).toBe("installing");
    expect(session.provisioning).toBe(true);
    push(serviceView({ revision: 3, phase: "running", lastAction: "installed" }));
    expect(session.view?.phase).toBe("running");
    expect(session.provisioning).toBe(false);
    expect(changes).toEqual([
      ["installing", null],
      ["running", "installing"],
    ]);

    // Registered once per window.
    await session.start();
    session.dispose();
    push(serviceView({ revision: 4, phase: "failed" }));
    expect(session.view?.phase).toBe("running");
  });

  it("drops a Repair reply older than a view the watcher already pushed", async () => {
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => (release = resolve));
    const { push } = ipc(async (command) => {
      if (command === "watch_local_service") return serviceView({ revision: 1, phase: "failed", canRepair: true });
      if (command === "repair_local_service") {
        await held;
        // The pass ended at revision 3; the reply crossed a newer push.
        return serviceView({ revision: 3, lastAction: "started" });
      }
      throw new Error(command);
    });
    const session = new LocalServiceSession();
    await session.start();
    const repairing = session.repair();
    await flush();
    push(serviceView({ revision: 3, lastAction: "started" }));
    // Another window's pass began after this one ended.
    push(serviceView({ revision: 4, phase: "checking" }));
    release();
    await repairing;
    expect(session.view).toMatchObject({ revision: 4, phase: "checking" });
    // A later view still applies.
    push(serviceView({ revision: 5, phase: "running" }));
    expect(session.view?.phase).toBe("running");
  });

  it("keeps a watcher failure as a stable error", async () => {
    ipc(() => {
      throw failure("client.state_unavailable", "internal");
    });
    const session = new LocalServiceSession();
    await session.start();
    expect(session.view).toBeNull();
    expect(session.error?.code).toBe("client.state_unavailable");
  });

  it("repairs once at a time and shows a refused repair", async () => {
    let answer: () => unknown = () => serviceView({ lastAction: "started" });
    const { calls } = ipc((command) => {
      if (command === "watch_local_service") return serviceView({ phase: "failed", canRepair: true });
      if (command === "repair_local_service") return answer();
      throw new Error(command);
    });
    const session = new LocalServiceSession();
    await session.start();
    const first = session.repair();
    const second = session.repair();
    expect(session.repairing).toBe(true);
    await Promise.all([first, second]);
    expect(calls.filter((call) => call.command === "repair_local_service")).toHaveLength(1);
    expect(session.view?.lastAction).toBe("started");
    expect(session.repairing).toBe(false);

    answer = () => {
      throw failure("service.busy");
    };
    await session.repair();
    expect(session.repairError?.code).toBe("service.busy");
  });

  it("reviews a rollback, sends it once, and reports what happened", async () => {
    let execute: () => unknown = () => serviceView({ currentVersion: "0.1.0", previousVersion: "0.2.0", lastAction: "rolled_back" });
    const { calls } = ipc((command) => {
      switch (command) {
        case "watch_local_service":
          return serviceView({ previousVersion: "0.1.0", canRollback: true });
        case "prepare_local_service_rollback":
          return { reviewId: "r1", currentVersion: "0.2.0", previousVersion: "0.1.0" };
        case "execute_local_service_rollback":
          return execute();
        default:
          throw new Error(command);
      }
    });
    const session = new LocalServiceSession();
    await session.start();
    await session.prepareRollback();
    expect(session.rollback).toEqual({
      kind: "review",
      review: { reviewId: "r1", currentVersion: "0.2.0", previousVersion: "0.1.0" },
    });
    const sending = session.confirmRollback();
    expect(session.rollback.kind).toBe("sending");
    // Closing while it is sent does nothing: the outcome is owed.
    session.closeRollback();
    expect(session.rollback.kind).toBe("sending");
    await sending;
    expect(session.rollback.kind).toBe("done");
    expect(session.view?.currentVersion).toBe("0.1.0");
    session.closeRollback();
    expect(session.rollback.kind).toBe("closed");

    execute = () => {
      throw failure("service.review_stale", "conflict");
    };
    await session.prepareRollback();
    await session.confirmRollback();
    expect(session.rollback).toMatchObject({ kind: "refused", error: { code: "service.review_stale" } });
    expect(calls.filter((call) => call.command === "execute_local_service_rollback")).toHaveLength(2);
  });

  it("drops a review that arrives after the dialog closed", async () => {
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => (release = resolve));
    ipc(async (command) => {
      if (command === "watch_local_service") return serviceView({ canRollback: true, previousVersion: "0.1.0" });
      await held;
      return { reviewId: "r1", currentVersion: "0.2.0", previousVersion: "0.1.0" };
    });
    const session = new LocalServiceSession();
    await session.start();
    const preparing = session.prepareRollback();
    expect(session.rollback.kind).toBe("preparing");
    session.closeRollback();
    release();
    await preparing;
    expect(session.rollback.kind).toBe("closed");
  });
});

describe("local service copy", () => {
  it("says what the shell is doing while it provisions", () => {
    expect(provisioningText("installing")).toBe("Setting up the Jet service on this computer…");
    expect(provisioningText("starting")).toBe("Starting the Jet service…");
    expect(provisioningText("updating")).toBe("Updating the Jet service on this computer…");
    expect(provisioningText("checking")).toBe("Checking the Jet service on this computer…");
    expect(provisioningText("running")).toBeNull();
    expect(phaseText("not_installed")).toBe("Not installed");
  });

  it("names who manages the service", () => {
    expect(channelText("gui")).toBe("Managed by this app");
    expect(channelText("homebrew")).toBe("Managed by Homebrew");
    expect(channelText("development")).toBe("Development build");
    expect(channelText(null)).toBe("Not identified");
    expect(managerText("systemd")).toMatch(/systemd/);
    expect(managerText(null)).toBeNull();
  });

  it("gives Setup service-aware failure copy", () => {
    expect(serviceProblem(serviceView())).toBeNull();
    expect(serviceProblem(serviceView({ phase: "installing" }))).toBeNull();
    const failed = serviceProblem(serviceView({ phase: "failed", error: failure("service.start_timeout") }));
    expect(failed).toEqual({
      title: "The Jet service needs attention",
      detail: "The Jet service was started but didn't answer in time.",
      code: "service.start_timeout",
    });
    expect(serviceProblem(serviceView({ phase: "stopped" }))?.title).toBe("The Jet service isn't running");
    expect(serviceProblem(serviceView({ phase: "stopped", channel: "homebrew" }))?.detail).toMatch(/Homebrew/);
    expect(serviceProblem(serviceView({ phase: "not_installed", channel: null }))?.title).toBe(
      "The Jet service isn't installed",
    );
    // The formula's full name: homebrew/core's unrelated `jet` owns the bare one.
    expect(serviceErrorText(failure("service.homebrew_start_failed"))).toContain(
      "brew services start apexgang/tap/jet in a terminal",
    );
    // A drain that timed out was asked to stop: only the version is kept.
    expect(serviceErrorText(failure("service.drain_timeout"))).not.toMatch(/nothing was changed/i);
    expect(rollbackRefusedNote(failure("service.drain_timeout"))).toBe("The version wasn't changed.");
    expect(rollbackRefusedNote(failure("service.review_stale"))).toBe("Nothing was changed.");
    // An unknown code falls back to the shell's own sentence.
    expect(serviceErrorText(failure("service.future_code"))).toBe("service.future_code message");
  });

  it("confirms what the last pass did", () => {
    expect(actionText(serviceView({ lastAction: "installed" }))).toBe("The Jet service 0.2.0 is set up on this computer.");
    expect(actionText(serviceView({ lastAction: "updated" }))).toBe("The Jet service was updated to 0.2.0.");
    expect(actionText(serviceView({ lastAction: "rolled_back", currentVersion: "0.1.0" }))).toBe(
      "The Jet service went back to 0.1.0.",
    );
    expect(actionText(serviceView())).toBeNull();
    expect(rollbackLines("0.2.0", "0.1.0")[0]).toBe("The Jet service stops, then starts again with version 0.1.0.");
  });
});
