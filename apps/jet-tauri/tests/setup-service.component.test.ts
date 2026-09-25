// @vitest-environment happy-dom
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { flushSync, tick } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { PublicError } from "../src/lib/jet/bridge";
import SetupPanel from "../src/lib/features/setup/SetupPanel.svelte";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";
import { serviceView } from "./support/service";

afterEach(() => {
  cleanup();
  clearMocks();
});

function offline(): PublicError {
  return {
    category: "offline",
    code: "transport.offline",
    message: "Jet could not reach this Plane.",
    retryable: true,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: "local",
  };
}

function failedSetup(): DesktopSession {
  const session = new DesktopSession();
  session.setup = { kind: "failed", error: offline() };
  return session;
}

async function settle() {
  flushSync();
  await tick();
}

const status = () => document.querySelector<HTMLElement>(".setup-provisioning");
const alert = () => document.querySelector<HTMLElement>(".setup-failure");
const button = (name: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((candidate) => candidate.textContent?.trim() === name);

describe("Setup while the local Jet service is provisioned (wave 4 §A)", () => {
  it("announces each provisioning phase in a polite live region instead of the offline copy", async () => {
    const session = failedSetup();
    session.service.view = serviceView({ phase: "installing" });
    render(SetupPanel, { session });
    await settle();
    expect(alert()).toBeNull();
    expect(status()?.getAttribute("role")).toBe("status");
    expect(status()?.getAttribute("aria-live")).toBe("polite");
    expect(status()?.textContent).toContain("Setting up the Jet service on this computer…");

    session.service.view = serviceView({ phase: "starting" });
    await settle();
    expect(status()?.textContent).toContain("Starting the Jet service…");
    expect(document.body.textContent).not.toContain("Local Plane unavailable");
  });

  it("shows service-aware failure copy with a keyboard-reachable Repair", async () => {
    const calls: string[] = [];
    mockIPC((command) => {
      calls.push(command);
      if (command === "repair_local_service") return serviceView({ lastAction: "started" });
      return null;
    });
    const session = failedSetup();
    session.service.view = serviceView({
      phase: "failed",
      canRepair: true,
      error: { ...offline(), category: "unavailable", code: "service.start_timeout", planeId: null },
    });
    render(SetupPanel, { session });
    await settle();
    expect(status()).toBeNull();
    expect(alert()?.getAttribute("role")).toBe("alert");
    expect(alert()?.textContent).toContain("The Jet service needs attention");
    expect(alert()?.textContent).toContain("service.start_timeout");
    const repair = button("Repair");
    expect(repair?.tagName).toBe("BUTTON");
    expect(repair?.disabled).toBe(false);
    expect(button("Try again")).toBeDefined();

    await fireEvent.click(repair!);
    await settle();
    expect(calls).toContain("repair_local_service");
  });

  it("explains a build without the service and checks again instead of repairing", async () => {
    const session = failedSetup();
    session.service.view = serviceView({
      phase: "not_installed",
      channel: null,
      manager: null,
      bundledVersion: null,
      currentVersion: null,
      runningVersion: null,
    });
    render(SetupPanel, { session });
    await settle();
    expect(alert()?.textContent).toContain("The Jet service isn't installed");
    expect(button("Repair")).toBeUndefined();
    expect(button("Check again")).toBeDefined();
  });

  it("offers Repair for an app-managed service that stopped answering", async () => {
    const session = failedSetup();
    session.service.view = serviceView();
    render(SetupPanel, { session });
    await settle();
    expect(alert()?.textContent).toContain("Local Plane unavailable");
    expect(button("Repair")).toBeDefined();
  });
});
