/**
 * Keeps keyboard focus in a block whose controls change with native state
 * (Settings › Versions). When the focused control is removed, a browser
 * drops focus to the page body and a keyboard user loses their place. This
 * moves focus to the first control still in `node`, or to `fallback` (the
 * block's heading) when none is left.
 *
 * Focus that the user moved elsewhere, by Tab or by clicking another
 * control, is never taken back.
 */
export function keepFocus(node: HTMLElement, fallback: () => HTMLElement | null) {
  let held = false;

  const focusin = () => {
    held = true;
  };
  const focusout = (event: FocusEvent) => {
    const next = event.relatedTarget;
    if (next instanceof Node && node.contains(next)) return;
    if (next !== null) {
      held = false;
      return;
    }
    // Focus went nowhere: the user blurred the page, or the control was
    // removed. Only a control still in the page was blurred on purpose.
    const target = event.target;
    queueMicrotask(() => {
      if (target instanceof Node && target.isConnected) held = false;
    });
  };
  const rescue = () => {
    const active = document.activeElement;
    if (!held || (active !== null && active !== document.body)) return;
    const next =
      node.querySelector<HTMLElement>("button:not([disabled]), input:not([disabled]), [tabindex='-1']") ?? fallback();
    if (next === null) {
      held = false;
      return;
    }
    next.focus();
  };

  const observer = new MutationObserver(rescue);
  observer.observe(node, { childList: true, subtree: true, attributes: true, attributeFilter: ["disabled"] });
  node.addEventListener("focusin", focusin);
  node.addEventListener("focusout", focusout);
  return {
    destroy() {
      observer.disconnect();
      node.removeEventListener("focusin", focusin);
      node.removeEventListener("focusout", focusout);
    },
  };
}
