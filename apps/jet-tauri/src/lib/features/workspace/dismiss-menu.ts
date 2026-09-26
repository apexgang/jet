/** Disclosure actions use normal Tab navigation, with native-menu dismissal. */
export function dismissMenu(node: HTMLDetailsElement) {
  const modal = () => document.querySelector("dialog[open]") !== null;
  const outside = (event: PointerEvent) => {
    if (node.open && !modal() && event.target instanceof Node && !node.contains(event.target)) node.open = false;
  };
  const key = (event: KeyboardEvent) => {
    if (event.key === "Escape" && node.open && !modal()) {
      event.preventDefault(); event.stopPropagation(); node.open = false;
      node.querySelector("summary")?.focus();
    }
  };
  const blur = (event: FocusEvent) => {
    if (node.open && !modal() && event.relatedTarget instanceof Node && !node.contains(event.relatedTarget)) node.open = false;
  };
  document.addEventListener("pointerdown", outside);
  document.addEventListener("keydown", key);
  node.addEventListener("focusout", blur);
  return { destroy() { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", key); node.removeEventListener("focusout", blur); } };
}
