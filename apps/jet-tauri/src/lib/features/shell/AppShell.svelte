<script lang="ts">
  import { untrack } from "svelte";
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
  import WorkPanel from "./WorkPanel.svelte";

  let { session }: { session: DesktopSession } = $props();

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

  // The overlay closed: focus goes back to what opened it, or else to the
  // header's Work panel button. Effects run after the page behind the
  // overlay has lost `inert`, so the target can take focus.
  $effect(() => {
    if (!session.takeFocusRequest("work-panel-return") || session.returnWorkPanelFocus()) return;
    document.querySelector<HTMLElement>(".work-panel-toggle")?.focus();
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
  <!-- Shell status (full screen, window layout): outside the regions an overlay makes inert. -->
  <p class="visually-hidden" role="status">{session.shellStatus}</p>
  <Sidebar {session} inert={overlay} />
  <div class="main-region" inert={overlay}>
    <Conversation {session} />
  </div>
  <WorkPanel {session} mode={overlay ? "overlay" : "column"} />

  {#if mode === "regular" && widths.sidebar > 0}
    <ColumnResizer
      label="Resize sidebar"
      edge="start"
      value={widths.sidebar}
      min={SIDEBAR_WIDTH.min}
      max={SIDEBAR_WIDTH.max}
      ideal={SIDEBAR_WIDTH.ideal}
      onchange={(width) => session.setColumnWidth("sidebar", width)}
    />
  {/if}
  {#if mode === "regular" && widths.panel > 0}
    <ColumnResizer
      label="Resize work panel"
      edge="end"
      value={widths.panel}
      min={WORK_PANEL_WIDTH.min}
      max={WORK_PANEL_WIDTH.max}
      ideal={WORK_PANEL_WIDTH.ideal}
      onchange={(width) => session.setColumnWidth("work-panel", width)}
    />
  {/if}
  {#if overlay}
    <!-- A pointer target only: the keyboard closes the overlay with Escape or Hide. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="work-panel-scrim" aria-hidden="true" onclick={() => session.hideWorkPanel()}></div>
  {/if}
</div>
