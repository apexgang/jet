import { describe, expect, it } from "vitest";

import {
  COMPACT_SIDEBAR_WIDTH,
  CONVERSATION_MINIMUM,
  INITIAL_PANEL,
  SIDEBAR_WIDTH,
  WORK_PANEL_WIDTH,
  clampWidth,
  columnWidths,
  layoutMode,
  reducePanel,
  resizeStep,
  type PanelIntent,
  type PanelState,
} from "../src/lib/features/shell/layout";

const regular = (columnPreference = true): PanelState => ({
  mode: "regular",
  columnPreference,
  presentation: columnPreference ? { kind: "column" } : { kind: "hidden" },
});
const compact = (columnPreference = true): PanelState => ({
  mode: "compact",
  columnPreference,
  presentation: { kind: "hidden" },
});

function run(state: PanelState, ...intents: PanelIntent[]): PanelState {
  return intents.reduce(reducePanel, state);
}

describe("layoutMode", () => {
  it("is compact up to the 1100 px breakpoint and regular above it", () => {
    expect(layoutMode(900)).toBe("compact");
    expect(layoutMode(1100)).toBe("compact");
    expect(layoutMode(1101)).toBe("regular");
    expect(layoutMode(1280)).toBe("regular");
    expect(layoutMode(2560)).toBe("regular");
  });
});

describe("reducePanel", () => {
  it("starts with work details tucked away", () => {
    expect(INITIAL_PANEL).toEqual(regular(false));
  });

  it("regular mode: navigation respects the panel preference", () => {
    expect(run(regular(false), { kind: "auto-open" })).toEqual(regular(false));
    expect(run(regular(true), { kind: "auto-open" })).toEqual(regular(true));
    expect(run(regular(false), { kind: "user-open", origin: "toggle" })).toEqual(regular(true));
    expect(run(regular(true), { kind: "user-close" })).toEqual(regular(false));
    expect(run(regular(true), { kind: "auto-close" })).toEqual(regular(false));
    expect(run(regular(true), { kind: "restore", presented: false })).toEqual(regular(false));
  });

  it("regular mode: returning to a task leaves a closed panel closed", () => {
    expect(run(regular(true), { kind: "auto-close" }, { kind: "auto-open" })).toEqual(regular(false));
  });

  it("compact mode: auto-open leaves the panel closed (D12)", () => {
    expect(run(compact(true), { kind: "auto-open" })).toEqual(compact(true));
    expect(run(compact(false), { kind: "auto-open" })).toEqual(compact(false));
  });

  it("compact mode: a user request opens the overlay with its origin", () => {
    expect(run(compact(), { kind: "user-open", origin: "toggle" }).presentation).toEqual({
      kind: "overlay",
      origin: "toggle",
    });
    expect(run(compact(), { kind: "user-open", origin: "run-control" }).presentation).toEqual({
      kind: "overlay",
      origin: "run-control",
    });
  });

  it("compact mode: Needs attention opens the overlay", () => {
    const next = run(compact(false), { kind: "user-open", origin: "attention" });
    expect(next.presentation).toEqual({ kind: "overlay", origin: "attention" });
    expect(next.columnPreference).toBe(false);
  });

  it("compact mode: closing keeps the column preference", () => {
    const opened = run(compact(true), { kind: "user-open", origin: "toggle" });
    expect(run(opened, { kind: "user-close" })).toEqual(compact(true));
    expect(run(opened, { kind: "auto-close" })).toEqual(compact(true));
    const closedFromHidden = run(compact(false), { kind: "user-open", origin: "shortcut" }, { kind: "user-close" });
    expect(closedFromHidden).toEqual(compact(false));
  });

  it("compact mode: restore sets the preference and stays closed", () => {
    expect(run(compact(false), { kind: "restore", presented: true })).toEqual(compact(true));
    const opened = run(compact(true), { kind: "user-open", origin: "toggle" });
    expect(run(opened, { kind: "restore", presented: false })).toEqual(compact(false));
  });

  it("regular to compact collapses the column; compact to regular brings it back", () => {
    expect(run(regular(true), { kind: "mode", mode: "compact" })).toEqual(compact(true));
    expect(run(compact(true), { kind: "mode", mode: "regular" })).toEqual(regular(true));
    expect(run(compact(false), { kind: "mode", mode: "regular" })).toEqual(regular(false));
    const overlay = run(compact(false), { kind: "user-open", origin: "toggle" });
    expect(run(overlay, { kind: "mode", mode: "regular" })).toEqual(regular(false));
  });

  it("returns the same state when a mode intent changes nothing", () => {
    const state = regular(true);
    expect(reducePanel(state, { kind: "mode", mode: "regular" })).toBe(state);
  });
});

describe("columnWidths", () => {
  const all = { sidebar: true, panel: true };

  it("uses the ideal widths at the default 1280 px window", () => {
    expect(columnWidths(1280, { sidebar: SIDEBAR_WIDTH.ideal, panel: WORK_PANEL_WIDTH.ideal }, all)).toEqual({
      sidebar: 244,
      panel: 340,
      conversation: 696,
    });
  });

  it("shrinks the work panel first to keep the conversation at 420 px", () => {
    const widths = columnWidths(1101, { sidebar: 300, panel: 440 }, all);
    expect(widths.conversation).toBe(CONVERSATION_MINIMUM);
    expect(widths.sidebar).toBe(300);
    expect(widths.panel).toBe(1101 - 300 - 420);
  });

  it("gives a hidden column no width", () => {
    expect(columnWidths(1280, { sidebar: 244, panel: 340 }, { sidebar: false, panel: true })).toEqual({
      sidebar: 0,
      panel: 340,
      conversation: 940,
    });
    expect(columnWidths(1280, { sidebar: 244, panel: 340 }, { sidebar: true, panel: false }).panel).toBe(0);
  });

  it("grows the conversation on wide screens with maximum columns", () => {
    expect(columnWidths(2560, { sidebar: 300, panel: 440 }, all)).toEqual({
      sidebar: 300,
      panel: 440,
      conversation: 1820,
    });
  });

  it("clamps requested widths into the Swift ranges", () => {
    expect(columnWidths(1920, { sidebar: 90, panel: 9000 }, all)).toMatchObject({ sidebar: 210, panel: 440 });
  });

  it("uses the compact sidebar and no panel column in compact mode", () => {
    expect(columnWidths(900, { sidebar: 300, panel: 440 }, all)).toEqual({
      sidebar: COMPACT_SIDEBAR_WIDTH,
      panel: 0,
      conversation: 900 - COMPACT_SIDEBAR_WIDTH,
    });
    expect(columnWidths(1000, { sidebar: 300, panel: 440 }, { sidebar: false, panel: true })).toEqual({
      sidebar: 0,
      panel: 0,
      conversation: 1000,
    });
  });

  it("always adds up to the viewport and keeps the conversation readable", () => {
    for (let viewport = 900; viewport <= 2560; viewport += 7) {
      for (const requested of [
        { sidebar: 210, panel: 280 },
        { sidebar: 244, panel: 340 },
        { sidebar: 300, panel: 440 },
      ]) {
        for (const visible of [all, { sidebar: false, panel: true }, { sidebar: true, panel: false }]) {
          const widths = columnWidths(viewport, requested, visible);
          expect(widths.sidebar + widths.panel + widths.conversation).toBe(viewport);
          expect(widths.conversation).toBeGreaterThanOrEqual(CONVERSATION_MINIMUM);
        }
      }
    }
  });

  it("treats an unreadable viewport as the minimum window", () => {
    expect(columnWidths(Number.NaN, { sidebar: 244, panel: 340 }, all).conversation).toBe(900 - COMPACT_SIDEBAR_WIDTH);
  });
});

describe("clampWidth", () => {
  it("rounds into the range and turns unreadable values into the minimum", () => {
    expect(clampWidth(100, SIDEBAR_WIDTH)).toBe(210);
    expect(clampWidth(500, SIDEBAR_WIDTH)).toBe(300);
    expect(clampWidth(250.6, SIDEBAR_WIDTH)).toBe(251);
    expect(clampWidth(Number.NaN, WORK_PANEL_WIDTH)).toBe(280);
    expect(clampWidth(Number.POSITIVE_INFINITY, WORK_PANEL_WIDTH)).toBe(280);
  });
});

describe("resizeStep", () => {
  it("widens the sidebar (start edge) with ArrowRight and ArrowUp", () => {
    expect(resizeStep("ArrowRight", false, "start")).toBe(8);
    expect(resizeStep("ArrowLeft", false, "start")).toBe(-8);
    expect(resizeStep("ArrowUp", false, "start")).toBe(8);
    expect(resizeStep("ArrowDown", false, "start")).toBe(-8);
  });

  it("widens the work panel (end edge) with ArrowLeft and ArrowUp", () => {
    expect(resizeStep("ArrowLeft", false, "end")).toBe(8);
    expect(resizeStep("ArrowRight", false, "end")).toBe(-8);
    expect(resizeStep("ArrowUp", false, "end")).toBe(8);
    expect(resizeStep("ArrowDown", false, "end")).toBe(-8);
  });

  it("takes 32 px steps with Shift", () => {
    expect(resizeStep("ArrowRight", true, "start")).toBe(32);
    expect(resizeStep("ArrowLeft", true, "end")).toBe(32);
    expect(resizeStep("ArrowDown", true, "end")).toBe(-32);
  });

  it("jumps to the ends with Home and End", () => {
    for (const edge of ["start", "end"] as const) {
      expect(resizeStep("Home", false, edge)).toBe("min");
      expect(resizeStep("End", false, edge)).toBe("max");
    }
  });

  it("ignores other keys", () => {
    for (const key of ["Enter", " ", "a", "Tab", "PageUp", "Escape"]) {
      expect(resizeStep(key, false, "start")).toBeNull();
      expect(resizeStep(key, true, "end")).toBeNull();
    }
  });
});
