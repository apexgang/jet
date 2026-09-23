// @vitest-environment happy-dom
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { tick } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import Conversation from "../src/lib/features/shell/Conversation.svelte";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";
import { ENABLED_SHORTCUTS } from "../src/lib/features/shell/shortcuts";
import WorkPanel from "../src/lib/features/shell/WorkPanel.svelte";
import { withActiveRun } from "./support/session";

afterEach(() => {
  cleanup();
  clearMocks();
});

function sessionWithApproval(): DesktopSession {
  const session = new DesktopSession();
  withActiveRun(session);
  session.connectionState = "online";
  session.conversationFreshness = "live";
  session.timeline = [
    {
      id: "approval-1",
      kind: "approval",
      text: "Run the tests",
      sequence: "5",
      rawCount: 0,
      approval: {
        requestId: "request-1",
        reviewId: null,
        runId: "run-1",
        tool: "Shell",
        action: "bun test",
        target: "Workspace",
        scope: "Once",
        consequence: "Runs a command",
        rationale: null,
        state: "requested",
        canAuthorizeRetry: false,
      },
    },
  ];
  return session;
}

function renderShell(session: DesktopSession) {
  render(Conversation, { session });
  render(WorkPanel, { session });
  const onKey = (event: KeyboardEvent) =>
    session.handleShortcut(event, {
      platform: "other",
      modalOpen: document.querySelector("dialog[open]") !== null,
      enabled: ENABLED_SHORTCUTS,
    });
  window.addEventListener("keydown", onKey);
  return () => window.removeEventListener("keydown", onKey);
}

function button(name: string, root: ParentNode = document): HTMLButtonElement {
  const match = [...root.querySelectorAll("button")].find((item) => item.textContent?.trim() === name);
  if (!match) throw new Error(`No button ${name}`);
  return match as HTMLButtonElement;
}

describe("Run controls from the approval card (D6)", () => {
  it("Interrupt Turn… opens the Run tab, focuses Cancel, and Escape returns focus", async () => {
    const session = sessionWithApproval();
    session.hideWorkPanel();
    session.selectedWorkPanel = "changes";
    const stop = renderShell(session);
    try {
      const card = document.querySelector('[aria-label="Approval request for Shell"]')!;
      const interrupt = button("Interrupt Turn…", card);
      interrupt.focus();
      await fireEvent.click(interrupt);

      expect(session.workPanelPresented).toBe(true);
      expect(session.selectedWorkPanel).toBe("run");
      expect(document.querySelector("#work-tab-run")?.getAttribute("aria-selected")).toBe("true");
      const confirmation = document.querySelector(".control-confirmation")!;
      expect(confirmation.textContent).toContain("Interrupt this Turn?");
      expect(document.activeElement).toBe(button("Cancel", confirmation));

      await fireEvent.keyDown(document.activeElement!, { key: "Escape", code: "Escape" });
      expect(document.querySelector(".control-confirmation")).toBeNull();
      expect(document.activeElement).toBe(interrupt);
    } finally {
      stop();
    }
  });

  it("Cancel returns focus to the Run tab's own Stop Run… button", async () => {
    const session = sessionWithApproval();
    const stop = renderShell(session);
    try {
      const controls = document.querySelector('[aria-label="Run controls"]')!;
      const stopRun = button("Stop Run…", controls);
      stopRun.focus();
      await fireEvent.click(stopRun);
      const confirmation = document.querySelector(".control-confirmation")!;
      expect(confirmation.textContent).toContain("Stop this Run?");
      expect(document.activeElement).toBe(button("Cancel", confirmation));

      await fireEvent.click(button("Cancel", confirmation));
      expect(document.querySelector(".control-confirmation")).toBeNull();
      expect(document.activeElement).toBe(stopRun);
    } finally {
      stop();
    }
  });

  it("confirming Stop Run moves focus to the Run tab, not the page body", async () => {
    const commands: string[] = [];
    mockIPC((command) => {
      commands.push(command);
      return command === "stop_run" ? { message: "Stopping the Run" } : null;
    });
    const session = sessionWithApproval();
    const stop = renderShell(session);
    try {
      const controls = document.querySelector('[aria-label="Run controls"]')!;
      const stopRun = button("Stop Run…", controls);
      stopRun.focus();
      await fireEvent.click(stopRun);
      const confirmation = document.querySelector(".control-confirmation")!;
      const confirm = button("Stop Run", confirmation);
      confirm.focus();
      await fireEvent.click(confirm);
      await tick();

      expect(commands).toContain("stop_run");
      expect(document.querySelector(".control-confirmation")).toBeNull();
      expect(document.activeElement).toBe(document.querySelector("#work-tab-run"));
    } finally {
      stop();
    }
  });

  it("confirming Interrupt Turn inside the overlay keeps focus in the overlay", async () => {
    mockIPC((command) => (command === "interrupt_turn" ? { message: "Interrupting" } : null));
    const session = sessionWithApproval();
    session.setLayoutMode("compact");
    const stop = renderShell(session);
    try {
      const card = document.querySelector('[aria-label="Approval request for Shell"]')!;
      await fireEvent.click(button("Interrupt Turn…", card));
      expect(session.workPanelOverlay).toBe(true);
      const confirmation = document.querySelector(".control-confirmation")!;
      await fireEvent.click(button("Interrupt Turn", confirmation));
      await tick();

      expect(document.activeElement).toBe(document.querySelector("#work-tab-run"));
    } finally {
      stop();
    }
  });

  it("announces status in a dedicated region, not the whole timeline (D8)", () => {
    const session = sessionWithApproval();
    const stop = renderShell(session);
    try {
      expect(document.querySelector(".timeline")?.hasAttribute("aria-live")).toBe(false);
      // AppShell speaks it, outside the regions the overlay makes inert.
      expect(session.taskStatus).toBe("Approval needed: Shell");
    } finally {
      stop();
    }
  });
});
