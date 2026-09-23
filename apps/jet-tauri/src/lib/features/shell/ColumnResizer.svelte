<script lang="ts">
  import { clampWidth, resizeStep, type ResizeEdge } from "./layout";

  let {
    label,
    value,
    min,
    max,
    ideal,
    edge,
    onchange,
  }: {
    label: string;
    /** The column's current width in CSS pixels. */
    value: number;
    min: number;
    max: number;
    /** Double-click resets to this width. */
    ideal: number;
    /** The side of the window the resized column is attached to. */
    edge: ResizeEdge;
    /** Receives the new, clamped width. */
    onchange: (width: number) => void;
  } = $props();

  /** The pointer drag in progress. The column follows it live, clamped. */
  let drag = $state<{ pointerId: number; startX: number; startWidth: number } | null>(null);

  function commit(width: number) {
    const next = clampWidth(width, { min, max });
    if (next !== value) onchange(next);
  }

  function handleKey(event: KeyboardEvent) {
    const step = resizeStep(event.key, event.shiftKey, edge);
    if (step === null || event.altKey || event.ctrlKey || event.metaKey) return;
    event.preventDefault();
    if (step === "min") commit(min);
    else if (step === "max") commit(max);
    else commit(value + step);
  }

  function widthAt(clientX: number, from: { startX: number; startWidth: number }): number {
    const moved = clientX - from.startX;
    return from.startWidth + (edge === "start" ? moved : -moved);
  }

  function handlePointerDown(event: PointerEvent & { currentTarget: HTMLDivElement }) {
    if (event.button !== 0) return;
    event.preventDefault();
    drag = { pointerId: event.pointerId, startX: event.clientX, startWidth: value };
    event.currentTarget.setPointerCapture?.(event.pointerId);
    event.currentTarget.focus();
  }

  function handlePointerMove(event: PointerEvent) {
    if (drag?.pointerId !== event.pointerId) return;
    commit(widthAt(event.clientX, drag));
  }

  function handlePointerUp(event: PointerEvent) {
    if (drag?.pointerId !== event.pointerId) return;
    const from = drag;
    drag = null;
    commit(widthAt(event.clientX, from));
  }

  /** A cancelled drag (the system took the pointer) puts the width back. */
  function handlePointerCancel(event: PointerEvent) {
    if (drag?.pointerId !== event.pointerId) return;
    const from = drag;
    drag = null;
    commit(from.startWidth);
  }
</script>

<!-- A focusable separator is an interactive widget (ARIA "window splitter"). -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="column-resizer"
  class:start={edge === "start"}
  class:end={edge === "end"}
  class:dragging={drag !== null}
  role="separator"
  aria-orientation="vertical"
  aria-label={label}
  aria-valuemin={min}
  aria-valuemax={max}
  aria-valuenow={value}
  tabindex="0"
  onkeydown={handleKey}
  onpointerdown={handlePointerDown}
  onpointermove={handlePointerMove}
  onpointerup={handlePointerUp}
  onpointercancel={handlePointerCancel}
  ondblclick={() => commit(ideal)}
></div>
