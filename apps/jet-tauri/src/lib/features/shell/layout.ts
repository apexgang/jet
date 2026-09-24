/**
 * The main window's column layout as a pure model: when the work panel is a
 * column or a focused overlay, how wide each column is, and how the column
 * resizers move. No DOM access, so it runs in node.
 *
 * Column ranges mirror the Swift client (`DesktopShellView.swift:17,29`).
 * Narrow windows never compress the conversation: below the overlay
 * breakpoint the work panel leaves the grid and opens only on request
 * (design-language l.50, l.96).
 */

export const MINIMUM_WINDOW = { width: 900, height: 600 } as const;
/** Compact when the viewport is at most this wide. */
export const OVERLAY_BREAKPOINT = 1100;
export const CONVERSATION_MINIMUM = 420;
export const SIDEBAR_WIDTH = { min: 210, ideal: 244, max: 300 } as const;
export const WORK_PANEL_WIDTH = { min: 280, ideal: 340, max: 440 } as const;
/** The sidebar's fixed width in compact mode. */
export const COMPACT_SIDEBAR_WIDTH = 224;

export type LayoutMode = "compact" | "regular";
/** What opened the work panel; decides where focus goes. */
export type PanelOrigin = "toggle" | "tab" | "run-control" | "attention" | "shortcut";

export type WorkPanelPresentation =
  | { kind: "hidden" }
  | { kind: "column" }
  | { kind: "overlay"; origin: PanelOrigin };

export type PanelIntent =
  /** Code paths that show the panel alongside a task: never an overlay. */
  | { kind: "auto-open" }
  /** The user asked for the panel: a toggle, a tab, a Run control, Needs attention, a shortcut. */
  | { kind: "user-open"; origin: PanelOrigin }
  /** The user closed it: Hide, Escape, the scrim, a toggle, a shortcut. */
  | { kind: "user-close" }
  /** A destination without a task: New task, Search, Setup and the others. */
  | { kind: "auto-close" }
  /** The saved presentation at startup. */
  | { kind: "restore"; presented: boolean }
  /** The window crossed the overlay breakpoint. */
  | { kind: "mode"; mode: LayoutMode };

export type PanelState = {
  mode: LayoutMode;
  /** Whether the work panel is a visible column in regular mode (the persisted preference). */
  columnPreference: boolean;
  presentation: WorkPanelPresentation;
};

export type ColumnRange = { readonly min: number; readonly max: number };

export const INITIAL_PANEL: PanelState = {
  mode: "regular",
  columnPreference: true,
  presentation: { kind: "column" },
};

export function layoutMode(viewportWidth: number): LayoutMode {
  return viewportWidth <= OVERLAY_BREAKPOINT ? "compact" : "regular";
}

function column(preference: boolean): WorkPanelPresentation {
  return preference ? { kind: "column" } : { kind: "hidden" };
}

/**
 * The next panel state. In regular mode every open and close is the
 * column preference. In compact mode only a user request opens the panel,
 * as an overlay, and nothing but a restore changes the column preference.
 */
export function reducePanel(state: PanelState, intent: PanelIntent): PanelState {
  if (intent.kind === "mode") {
    if (intent.mode === state.mode) return state;
    return {
      mode: intent.mode,
      columnPreference: state.columnPreference,
      presentation: intent.mode === "compact" ? { kind: "hidden" } : column(state.columnPreference),
    };
  }

  if (state.mode === "regular") {
    switch (intent.kind) {
      case "auto-open":
      case "user-open":
        return { ...state, columnPreference: true, presentation: { kind: "column" } };
      case "user-close":
      case "auto-close":
        return { ...state, columnPreference: false, presentation: { kind: "hidden" } };
      case "restore":
        return { ...state, columnPreference: intent.presented, presentation: column(intent.presented) };
    }
  }

  switch (intent.kind) {
    case "auto-open":
      // Never cover the conversation on its own (D12).
      return state;
    case "user-open":
      return { ...state, presentation: { kind: "overlay", origin: intent.origin } };
    case "user-close":
    case "auto-close":
      return state.presentation.kind === "hidden" ? state : { ...state, presentation: { kind: "hidden" } };
    case "restore":
      return { ...state, columnPreference: intent.presented, presentation: { kind: "hidden" } };
  }
}

/** A whole pixel width inside `range`; anything unreadable is the minimum. */
export function clampWidth(value: number, range: ColumnRange): number {
  if (!Number.isFinite(value)) return range.min;
  return Math.min(range.max, Math.max(range.min, Math.round(value)));
}

/**
 * Pixel widths of the three columns for a viewport. Hidden columns are 0.
 * In regular mode the conversation keeps at least `CONVERSATION_MINIMUM`:
 * the work panel gives way first (down to its minimum), then the sidebar.
 * In compact mode the sidebar is fixed and the panel takes no column.
 * The three widths always add up to the viewport.
 */
export function columnWidths(
  viewportWidth: number,
  requested: { sidebar: number; panel: number },
  visible: { sidebar: boolean; panel: boolean },
): { sidebar: number; panel: number; conversation: number } {
  const viewport = Number.isFinite(viewportWidth) && viewportWidth > 0
    ? Math.round(viewportWidth)
    : MINIMUM_WINDOW.width;

  if (layoutMode(viewport) === "compact") {
    const sidebar = visible.sidebar ? Math.min(COMPACT_SIDEBAR_WIDTH, viewport) : 0;
    return { sidebar, panel: 0, conversation: viewport - sidebar };
  }

  let sidebar = visible.sidebar ? clampWidth(requested.sidebar, SIDEBAR_WIDTH) : 0;
  let panel = visible.panel ? clampWidth(requested.panel, WORK_PANEL_WIDTH) : 0;
  let shortfall = CONVERSATION_MINIMUM - (viewport - sidebar - panel);
  if (shortfall > 0 && panel > 0) {
    const give = Math.min(shortfall, panel - WORK_PANEL_WIDTH.min);
    panel -= give;
    shortfall -= give;
  }
  if (shortfall > 0 && sidebar > 0) {
    const give = Math.min(shortfall, sidebar - SIDEBAR_WIDTH.min);
    sidebar -= give;
  }
  return { sidebar, panel, conversation: viewport - sidebar - panel };
}

/** The side of the window a resized column is attached to. */
export type ResizeEdge = "start" | "end";

export const RESIZE_STEP = 8;
export const RESIZE_STEP_LARGE = 32;

/**
 * How a key press on a column resizer changes the column's width: a signed
 * pixel step, a jump to the minimum or maximum, or null for other keys.
 * Arrows move the separator the way they point, so for the sidebar (start
 * edge) ArrowRight widens and for the work panel (end edge) ArrowLeft
 * widens. ArrowUp always widens and ArrowDown always narrows.
 */
export function resizeStep(key: string, shift: boolean, edge: ResizeEdge): number | "min" | "max" | null {
  const step = shift ? RESIZE_STEP_LARGE : RESIZE_STEP;
  switch (key) {
    case "ArrowUp":
      return step;
    case "ArrowDown":
      return -step;
    case "ArrowRight":
      return edge === "start" ? step : -step;
    case "ArrowLeft":
      return edge === "start" ? -step : step;
    case "Home":
      return "min";
    case "End":
      return "max";
    default:
      return null;
  }
}
