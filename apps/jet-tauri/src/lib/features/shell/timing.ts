/**
 * User Timing marks for the desktop release journey (Wave 4 §F,
 * `tests/e2e/`). The journey reads them through WebDriver and turns each
 * into wall-clock time with `performance.timeOrigin`. Nothing in the app
 * reads them; they carry no user data, and a page without the Performance
 * API skips them.
 */

/** The sidebar and the composer are rendered with their handlers and painted. */
export const SHELL_INTERACTIVE = "shell-interactive";
/**
 * This computer's Plane feed came online: it opened online, or it was down
 * and then said `connected` or `resumed`. Marked every time.
 */
export const LOCAL_PLANE_CONNECTED = "local-plane-connected";

/** Marked when this window's local-service watcher first sees `phase`. */
export function localServiceMark(phase: string): string {
  return `local-service-${phase}`;
}

export function mark(name: string): void {
  try {
    globalThis.performance?.mark(name);
  } catch {
    // Diagnostics only: a failed mark never affects the shell.
  }
}

function marked(name: string): boolean {
  try {
    return (globalThis.performance?.getEntriesByName(name, "mark").length ?? 0) > 0;
  } catch {
    return false;
  }
}

/** Runs `callback` with the next frame, or at once where there are no frames. */
function nextFrame(callback: () => void): void {
  if (typeof requestAnimationFrame === "function") {
    requestAnimationFrame(() => callback());
  } else {
    callback();
  }
}

/**
 * Marks `shell-interactive` once per page, in the first frame after the
 * shell mounted: its controls are then painted and answer input.
 */
export function markShellInteractive(schedule: (callback: () => void) => void = nextFrame): void {
  if (marked(SHELL_INTERACTIVE)) return;
  schedule(() => {
    if (!marked(SHELL_INTERACTIVE)) mark(SHELL_INTERACTIVE);
  });
}
