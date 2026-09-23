/** How long a closed dialog waits for its trigger to become usable again. */
const RETURN_FOCUS_MS = 30_000;

function usable(element: Element | null): element is HTMLElement {
  return element instanceof HTMLElement && element.isConnected && !element.matches(":disabled");
}

/**
 * Returns focus to a dialog's trigger once it can take it (§7.3: focus
 * returns to the trigger). The trigger is often disabled while its change
 * is in flight, or replaced once it settles, so the candidates are the
 * element that had focus when the dialog opened, then `fallbackIds` in
 * order. Stops as soon as focus is somewhere other than the document body:
 * focus the user moved is never taken back.
 */
export function returnFocusWhenReady(recorded: HTMLElement | null, fallbackIds: readonly string[] = []): void {
  if (typeof document === "undefined") return;
  const attempt = (): boolean => {
    const active = document.activeElement;
    if (active && active !== document.body) return true;
    const candidates: Array<Element | null> = [recorded, ...fallbackIds.map((id) => document.getElementById(id))];
    const target = candidates.find(usable);
    if (!target) return false;
    target.focus();
    return true;
  };
  queueMicrotask(() => {
    if (attempt()) return;
    const observer = new MutationObserver(() => {
      if (attempt()) stop();
    });
    const timer = setTimeout(stop, RETURN_FOCUS_MS);
    function stop(): void {
      observer.disconnect();
      clearTimeout(timer);
    }
    observer.observe(document.body, { subtree: true, childList: true, attributes: true, attributeFilter: ["disabled"] });
  });
}

/** The element that has focus now, unless that is only the document body. */
export function focusedElement(): HTMLElement | null {
  if (typeof document === "undefined") return null;
  const active = document.activeElement;
  return active instanceof HTMLElement && active !== document.body ? active : null;
}
