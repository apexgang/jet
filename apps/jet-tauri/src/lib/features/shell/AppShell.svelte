<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { MediaQuery } from "svelte/reactivity";

  import ColumnResizer from "./ColumnResizer.svelte";
  import Conversation from "./Conversation.svelte";
  import {
    MINIMUM_WINDOW,
    OVERLAY_BREAKPOINT,
    SIDEBAR_WIDTH,
    WORK_PANEL_WIDTH,
    clampWidth,
    columnWidths,
  } from "./layout";
  import type { DesktopSession } from "./session.svelte";
  import Sidebar from "./Sidebar.svelte";
  import { markShellInteractive } from "./timing";
  import WorkPanel from "./WorkPanel.svelte";

  let { session }: { session: DesktopSession } = $props();

  // The sidebar and the task view's composer mount first, whatever startup
  // restores later; the release journey times launch to this mark.
  onMount(() => markShellInteractive());

  const compact = new MediaQuery(`max-width: ${OVERLAY_BREAKPOINT}px`);
  let innerWidth = $state<number>(MINIMUM_WINDOW.width);

  const mode = $derived(compact.current ? "compact" : "regular");
  const overlay = $derived(session.workPanelOverlay);
  const widths = $derived(
    columnWidths(
      innerWidth,
      { sidebar: session.sidebarWidth, panel: session.workPanelWidth },
      { sidebar: session.sidebarPresented, panel: session.panel.presentation.kind === "column" },
    ),
  );

  // Crossing the breakpoint turns the column into a closed overlay, and back.
  $effect.pre(() => {
    const next = mode;
    untrack(() => session.setLayoutMode(next));
  });

  /**
   * Focuses the first header control that exists: the task view's Work
   * panel button, else the Sidebar button every destination header carries.
   */
  function focusHeaderControl(selectors: readonly string[]): void {
    for (const selector of selectors) {
      const control = document.querySelector<HTMLElement>(`.main-region ${selector}`);
      if (control) {
        control.focus();
        return;
      }
    }
  }

  const WORK_PANEL_FALLBACK = [".work-panel-toggle", ".sidebar-toggle"] as const;

  /** A column hid under the focused element: focus moves to a header control. */
  function rescueFocus(column: string, selectors: readonly string[]): void {
    const active = document.activeElement;
    if (active && document.querySelector(column)?.contains(active)) focusHeaderControl(selectors);
  }

  // The overlay closed: focus goes back to what opened it, or else to a
  // header control. Effects run after the page behind the overlay has lost
  // `inert`, so the target can take focus.
  $effect(() => {
    if (!session.takeFocusRequest("work-panel-return") || session.returnWorkPanelFocus()) return;
    focusHeaderControl(WORK_PANEL_FALLBACK);
  });

  // F9, Ctrl+Alt+0 or Hide hid the column that had focus.
  $effect(() => {
    if (!session.sidebarPresented) untrack(() => rescueFocus("aside.sidebar", [".sidebar-toggle"]));
  });
  $effect(() => {
    if (!session.workPanelPresented) untrack(() => rescueFocus(".work-panel", WORK_PANEL_FALLBACK));
  });
</script>

<svelte:window bind:innerWidth />

<!-- Widths are CSS custom properties set through CSSOM: the production CSP blocks style attributes. -->
<div
  class="app-shell"
  data-layout={mode}
  style:--sidebar-width={`${widths.sidebar}px`}
  style:--work-panel-width={`${widths.panel}px`}
  style:--work-panel-overlay-width={`${clampWidth(session.workPanelWidth, WORK_PANEL_WIDTH)}px`}
>
  <!-- Shell and task status: outside the regions an overlay makes inert, so they are still spoken. -->
  <p class="visually-hidden" role="status">{session.shellStatus}</p>
  <p class="visually-hidden task-status" role="status">{session.taskStatus}</p>
  <Sidebar {session} inert={overlay} />
  <!-- Each resizer sits next to its column in the DOM, so Tab reaches it there; it is absolutely positioned. -->
  {#if mode === "regular" && widths.sidebar > 0}
    <ColumnResizer
      label="Resize sidebar"
      edge="start"
      value={widths.sidebar}
      requested={session.sidebarWidth}
      min={SIDEBAR_WIDTH.min}
      max={SIDEBAR_WIDTH.max}
      ideal={SIDEBAR_WIDTH.ideal}
      onchange={(width) => session.setColumnWidth("sidebar", width)}
    />
  {/if}
  <main class="main-region" inert={overlay}>
    <Conversation {session} />
  </main>
  {#if mode === "regular" && widths.panel > 0}
    <ColumnResizer
      label="Resize work panel"
      edge="end"
      value={widths.panel}
      requested={session.workPanelWidth}
      min={WORK_PANEL_WIDTH.min}
      max={WORK_PANEL_WIDTH.max}
      ideal={WORK_PANEL_WIDTH.ideal}
      onchange={(width) => session.setColumnWidth("work-panel", width)}
    />
  {/if}
  <WorkPanel {session} mode={overlay ? "overlay" : "column"} />
  {#if overlay}
    <!-- A pointer target only: the keyboard closes the overlay with Escape or Hide. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="work-panel-scrim" aria-hidden="true" onclick={() => session.hideWorkPanel()}></div>
  {/if}
</div>
