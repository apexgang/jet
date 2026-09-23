// @vitest-environment happy-dom
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { flushSync } from "svelte";
import { afterEach, describe, expect, it } from "vitest";

import { DesktopSession } from "../src/lib/features/shell/session.svelte";
import WorkPanel from "../src/lib/features/shell/WorkPanel.svelte";
import { withActiveRun } from "./support/session";

afterEach(() => cleanup());

function tabs(container: HTMLElement): HTMLButtonElement[] {
  return [...container.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
}

function selected(container: HTMLElement): string | undefined {
  return tabs(container).find((tab) => tab.getAttribute("aria-selected") === "true")?.id;
}

describe("work panel tablist", () => {
  it("uses a roving tabindex that follows the selected tab", () => {
    const session = new DesktopSession();
    const { container } = render(WorkPanel, { session });
    expect(tabs(container).map((tab) => tab.id)).toEqual([
      "work-tab-changes",
      "work-tab-files",
      "work-tab-terminal",
      "work-tab-run",
      "work-tab-delivery",
    ]);
    expect(tabs(container).map((tab) => tab.tabIndex)).toEqual([-1, -1, -1, 0, -1]);
    session.showPanel("files");
    flushSync();
    expect(tabs(container).map((tab) => tab.tabIndex)).toEqual([-1, 0, -1, -1, -1]);
  });

  it("moves focus and selection with the arrow keys, Home and End", async () => {
    const session = new DesktopSession();
    const { container } = render(WorkPanel, { session });
    const run = container.querySelector<HTMLButtonElement>("#work-tab-run")!;
    run.focus();

    await fireEvent.keyDown(run, { key: "ArrowRight" });
    expect(selected(container)).toBe("work-tab-delivery");
    expect(document.activeElement?.id).toBe("work-tab-delivery");

    await fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
    expect(selected(container)).toBe("work-tab-changes");
    expect(document.activeElement?.id).toBe("work-tab-changes");

    await fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" });
    expect(selected(container)).toBe("work-tab-delivery");

    await fireEvent.keyDown(document.activeElement!, { key: "Home" });
    expect(selected(container)).toBe("work-tab-changes");
    expect(document.activeElement?.id).toBe("work-tab-changes");

    await fireEvent.keyDown(document.activeElement!, { key: "End" });
    expect(selected(container)).toBe("work-tab-delivery");
    expect(document.activeElement?.id).toBe("work-tab-delivery");
    expect(session.selectedWorkPanel).toBe("delivery");
  });

  it("points a tab at its panel only while the panel exists", () => {
    const session = new DesktopSession();
    const { container } = render(WorkPanel, { session });
    const controls = () => tabs(container).map((tab) => tab.getAttribute("aria-controls"));
    // No Run selected: only Deliver has a panel.
    expect(controls()).toEqual([null, null, null, null, "work-delivery"]);

    withActiveRun(session);
    flushSync();
    expect(controls()).toEqual(["work-changes", "work-files", "work-terminal", "work-run", "work-delivery"]);
    for (const id of controls()) expect(container.querySelector(`#${id}`)).not.toBeNull();
  });

  it("labels each panel by its tab", () => {
    const session = new DesktopSession();
    withActiveRun(session);
    const { container } = render(WorkPanel, { session });
    const panels = [...container.querySelectorAll('[role="tabpanel"]')];
    expect(panels.length).toBe(5);
    for (const panel of panels) {
      const tab = container.querySelector(`#${panel.getAttribute("aria-labelledby")}`);
      expect(tab?.getAttribute("role")).toBe("tab");
      expect(tab?.getAttribute("aria-controls")).toBe(panel.id);
      expect(panel.hasAttribute("aria-label")).toBe(false);
    }
  });
});
