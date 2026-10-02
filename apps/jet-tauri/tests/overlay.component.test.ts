// @vitest-environment happy-dom
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { flushSync, tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import AppShell from "../src/lib/features/shell/AppShell.svelte";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";
import { ENABLED_SHORTCUTS } from "../src/lib/features/shell/shortcuts";
import { withActiveRun } from "./support/session";

type HappyWindow = { happyDOM: { setViewport(viewport: { width: number; height: number }): void } };

function viewport(width: number) {
  (window as unknown as HappyWindow).happyDOM.setViewport({ width, height: 800 });
  window.dispatchEvent(new Event("resize"));
}

let stopKeys: () => void = () => undefined;

beforeEach(() => viewport(1000));
afterEach(() => {
  stopKeys();
  cleanup();
});

function renderShell(session = new DesktopSession()) {
  // Inspector tests use an open task. New-task setup deliberately has no inspector toggle.
  session.selectedConversationId ??= "l1";
  const result = render(AppShell, { session });
  const onKey = (event: KeyboardEvent) =>
    session.handleShortcut(event, {
      platform: "other",
      modalOpen: document.querySelector("dialog[open]") !== null,
      enabled: ENABLED_SHORTCUTS,
    });
  window.addEventListener("keydown", onKey);
  stopKeys = () => window.removeEventListener("keydown", onKey);
  flushSync();
  return { ...result, session };
}

const sidebar = () => document.querySelector<HTMLElement>("aside.sidebar")!;
const main = () => document.querySelector<HTMLElement>(".main-region")!;
const panel = () => document.querySelector<HTMLElement>(".work-panel")!;
const toggle = () => document.querySelector<HTMLButtonElement>(".work-panel-toggle")!;

async function settle() {
  flushSync();
  await tick();
}

describe("compact work panel overlay", () => {
  it("starts closed in a narrow window, whatever the column preference (D12)", () => {
    const { session } = renderShell();
    expect(document.querySelector(".app-shell")?.getAttribute("data-layout")).toBe("compact");
    // The task view is the window's main landmark (adaptation audit, axe landmark-one-main).
    expect(main().tagName).toBe("MAIN");
    expect(session.workPanelPresented).toBe(false);
    expect(panel().classList.contains("hidden")).toBe(true);
    expect(document.querySelector(".work-panel-scrim")).toBeNull();
    expect(document.querySelector('[role="separator"]')).toBeNull();
  });

  it("opens as a modal dialog, makes the page behind it inert, and moves focus in", async () => {
    renderShell();
    toggle().focus();
    await fireEvent.click(toggle());
    await settle();

    expect(panel().getAttribute("role")).toBe("dialog");
    expect(panel().getAttribute("aria-modal")).toBe("true");
    expect(panel().getAttribute("aria-label")).toBe("Work panel");
    expect(sidebar().hasAttribute("inert")).toBe(true);
    expect(main().hasAttribute("inert")).toBe(true);
    expect(panel().hasAttribute("inert")).toBe(false);
    expect(document.querySelector(".work-panel-scrim")?.getAttribute("aria-hidden")).toBe("true");
    expect(document.activeElement?.id).toBe("work-tab-run");
  });

  it("Escape closes it and returns focus to the toggle", async () => {
    renderShell();
    toggle().focus();
    await fireEvent.click(toggle());
    await settle();

    await fireEvent.keyDown(document.activeElement!, { key: "Escape", code: "Escape" });
    await settle();
    expect(panel().classList.contains("hidden")).toBe(true);
    expect(panel().getAttribute("role")).toBe("complementary");
    expect(panel().hasAttribute("aria-modal")).toBe(false);
    expect(sidebar().hasAttribute("inert")).toBe(false);
    expect(main().hasAttribute("inert")).toBe(false);
    expect(document.activeElement).toBe(toggle());
  });

  it("closing an overlay a shortcut opened outside the task view focuses the Sidebar button", async () => {
    const { session } = renderShell();
    session.select("planes");
    await settle();
    (document.activeElement as HTMLElement | null)?.blur();
    await fireEvent.keyDown(document.body, { key: "0", code: "Digit0", ctrlKey: true, altKey: true });
    await settle();
    expect(session.workPanelOverlay).toBe(true);
    expect(toggle()).toBeNull();

    await fireEvent.keyDown(document.activeElement!, { key: "Escape", code: "Escape" });
    await settle();
    expect(session.workPanelOverlay).toBe(false);
    expect(document.activeElement).toBe(main().querySelector(".sidebar-toggle"));
  });

  it("the scrim and Hide close it too", async () => {
    renderShell();
    await fireEvent.click(toggle());
    await settle();
    await fireEvent.click(document.querySelector(".work-panel-scrim")!);
    await settle();
    expect(document.querySelector(".work-panel-scrim")).toBeNull();
    expect(document.activeElement).toBe(toggle());

    await fireEvent.click(toggle());
    await settle();
    const hide = panel().querySelector<HTMLButtonElement>(".panel-close")!;
    await fireEvent.click(hide);
    await settle();
    expect(panel().classList.contains("hidden")).toBe(true);
    expect(document.activeElement).toBe(toggle());
  });

  it("an approval card's Interrupt Turn… opens it on Run with Cancel focused; Escape then goes back to the card", async () => {
    const session = new DesktopSession();
    withActiveRun(session);
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
    renderShell(session);
    const card = document.querySelector('[aria-label="Approval request for Shell"]')!;
    const interrupt = [...card.querySelectorAll("button")].find((button) => button.textContent?.trim() === "Interrupt Turn…")!;
    interrupt.focus();
    await fireEvent.click(interrupt);
    await settle();

    expect(session.panel.presentation).toEqual({ kind: "overlay", origin: "run-control" });
    expect(document.activeElement?.textContent?.trim()).toBe("Cancel");
    expect(panel().contains(document.activeElement)).toBe(true);

    await fireEvent.keyDown(document.activeElement!, { key: "Escape", code: "Escape" });
    await settle();
    expect(session.workPanelOverlay).toBe(false);
    expect(document.activeElement).toBe(interrupt);
  });

  it("task status changes are still spoken while the overlay makes the page inert (D8)", async () => {
    const session = new DesktopSession();
    withActiveRun(session);
    session.connectionState = "online";
    session.conversationFreshness = "live";
    const approval = (tool: string) => ({
      id: `approval-${tool}`,
      kind: "approval" as const,
      text: "Run the tests",
      sequence: "5",
      rawCount: 0,
      approval: {
        requestId: `request-${tool}`,
        reviewId: null,
        runId: "run-1",
        tool,
        action: "bun test",
        target: "Workspace",
        scope: "Once",
        consequence: "Runs a command",
        rationale: null,
        state: "requested" as const,
        canAuthorizeRetry: false,
      },
    });
    session.timeline = [approval("Shell")];
    renderShell(session);
    await fireEvent.click(toggle());
    await settle();
    expect(main().inert).toBe(true);

    session.timeline = [approval("Shell"), approval("Git")];
    await settle();
    const regions = [...document.querySelectorAll<HTMLElement>('[role="status"]')].filter(
      (region) => region.textContent === "Approval needed: Git",
    );
    expect(regions).toHaveLength(1);
    expect(regions[0].closest("[inert]")).toBeNull();
    expect(main().querySelector(".task-status")).toBeNull();

    // Outside the task view the region is silent.
    session.select("planes");
    await settle();
    expect(document.querySelector(".task-status")?.textContent).toBe("");
  });

  it("never remounts the work panel: the same element before hide, after show, and in the overlay", async () => {
    viewport(1280);
    const { session } = renderShell();
    session.toggleWorkPanel("toggle");
    await settle();
    const root = panel();
    expect(root.getAttribute("role")).toBe("complementary");
    expect(root.classList.contains("hidden")).toBe(false);

    await fireEvent.click(toggle());
    await settle();
    expect(panel()).toBe(root);
    expect(root.classList.contains("hidden")).toBe(true);

    await fireEvent.click(toggle());
    await settle();
    expect(panel()).toBe(root);

    viewport(1000);
    await settle();
    expect(session.layoutMode).toBe("compact");
    session.toggleWorkPanel("toggle", null);
    await settle();
    expect(panel()).toBe(root);
    expect(root.getAttribute("role")).toBe("dialog");

    viewport(1280);
    await settle();
    expect(panel()).toBe(root);
    expect(root.getAttribute("role")).toBe("complementary");
    expect(root.classList.contains("hidden")).toBe(false);
  });
});

describe("regular layout", () => {
  it("shows both column resizers and sets widths through CSSOM", async () => {
    viewport(1280);
    const { session } = renderShell();
    session.toggleWorkPanel("toggle");
    await settle();
    const shell = document.querySelector<HTMLElement>(".app-shell")!;
    expect(shell.getAttribute("data-layout")).toBe("regular");
    expect(shell.style.getPropertyValue("--sidebar-width")).toBe("244px");
    expect(shell.style.getPropertyValue("--work-panel-width")).toBe("340px");
    const separators = [...document.querySelectorAll<HTMLElement>('[role="separator"]')];
    expect(separators.map((item) => item.getAttribute("aria-label"))).toEqual(["Resize sidebar", "Resize work panel"]);
    // Tab reaches each resizer next to its column, not after the whole work panel.
    expect(sidebar().nextElementSibling).toBe(separators[0]);
    expect(panel().previousElementSibling).toBe(separators[1]);

    await fireEvent.keyDown(separators[1], { key: "ArrowLeft", shiftKey: true });
    await settle();
    expect(session.workPanelWidth).toBe(372);
    expect(shell.style.getPropertyValue("--work-panel-width")).toBe("372px");

    session.toggleSidebar();
    await settle();
    expect(shell.style.getPropertyValue("--sidebar-width")).toBe("0px");
    expect(document.querySelectorAll('[role="separator"]')).toHaveLength(1);
  });

  it("hiding the work panel column that has focus moves focus to the Work panel button", async () => {
    viewport(1280);
    const { session } = renderShell();
    session.toggleWorkPanel("toggle");
    await settle();
    expect(session.panel.presentation).toEqual({ kind: "column" });
    const tab = document.querySelector<HTMLElement>("#work-tab-run")!;
    tab.focus();
    await fireEvent.keyDown(tab, { key: "0", code: "Digit0", ctrlKey: true, altKey: true });
    await settle();
    expect(session.workPanelPresented).toBe(false);
    expect(document.activeElement).toBe(toggle());

    // The panel's own Hide button does the same.
    await fireEvent.click(toggle());
    await settle();
    const hide = panel().querySelector<HTMLButtonElement>(".panel-close")!;
    hide.focus();
    await fireEvent.click(hide);
    await settle();
    expect(document.activeElement).toBe(toggle());
  });

  it("F9 on a sidebar control moves focus to the header's Sidebar button", async () => {
    viewport(1280);
    const { session } = renderShell();
    await settle();
    const control = sidebar().querySelector<HTMLButtonElement>("button")!;
    control.focus();
    await fireEvent.keyDown(control, { key: "F9", code: "F9" });
    await settle();
    expect(session.sidebarPresented).toBe(false);
    expect(document.activeElement).toBe(main().querySelector(".sidebar-toggle"));
  });

  it("an arrow on a narrowed work panel keeps the requested width", async () => {
    viewport(1101);
    const { session } = renderShell();
    session.toggleWorkPanel("toggle");
    session.setColumnWidth("sidebar", 300);
    session.setColumnWidth("work-panel", 440);
    await settle();
    const resizer = document.querySelector<HTMLElement>('[aria-label="Resize work panel"]')!;
    expect(resizer.getAttribute("aria-valuenow")).toBe("381");
    await fireEvent.keyDown(resizer, { key: "ArrowLeft" });
    await settle();
    expect(session.workPanelWidth).toBe(440);
    await fireEvent.keyDown(resizer, { key: "ArrowRight" });
    await settle();
    expect(session.workPanelWidth).toBe(373);
    expect(resizer.getAttribute("aria-valuenow")).toBe("373");
  });

  it("offers the sidebar toggle in every main-window destination", async () => {
    viewport(1280);
    const { session } = renderShell();
    for (const destination of ["conversation", "project", "schedules", "planes", "trash"] as const) {
      session.select(destination);
      await settle();
      const toggles = main().querySelectorAll<HTMLButtonElement>(".sidebar-toggle");
      expect(toggles, destination).toHaveLength(1);
      expect(toggles[0].getAttribute("aria-label")).toBe("Hide sidebar");
      expect(toggles[0].title).toBe("Hide sidebar (F9)");
      expect(toggles[0].getAttribute("aria-keyshortcuts")).toBe("F9");
    }
    await fireEvent.click(main().querySelector(".sidebar-toggle")!);
    await settle();
    expect(session.sidebarPresented).toBe(false);
    expect(main().querySelector(".sidebar-toggle")?.getAttribute("aria-label")).toBe("Show sidebar");
  });
});
