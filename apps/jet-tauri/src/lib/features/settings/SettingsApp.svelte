<script lang="ts">
  import { onMount, tick, untrack } from "svelte";

  import { quitJet } from "$lib/jet/presentation";
  import {
    closeSettings,
    rememberSettingsPane,
    watchSettingsNavigation,
    type SettingsNavigation,
    type SettingsPane,
    type SettingsSection,
  } from "$lib/jet/settings-window";
  import type { PlaneId } from "$lib/jet/planes";
  import AgentsPane from "./AgentsPane.svelte";
  import ConnectionsPane from "./ConnectionsPane.svelte";
  import GeneralPane from "./GeneralPane.svelte";
  import PlaneBanners from "./PlaneBanners.svelte";
  import PlanePicker from "./PlanePicker.svelte";
  import SafetyPane from "./SafetyPane.svelte";
  import { SettingsSession } from "./session.svelte";
  import WorkPane from "./WorkPane.svelte";
  import {
    LAST_WRITER_WINS,
    PLANE_PANES,
    landedPanes,
    paneOf,
    paneTitle,
    resolveTarget,
    sectionHeadingId,
  } from "./model";

  const panes = landedPanes();

  type Navigation =
    | { kind: "starting" }
    | { kind: "ready"; pane: SettingsPane }
    | { kind: "failed"; pane: SettingsPane };

  let navigation = $state<Navigation>({ kind: "starting" });
  let nav = $state<HTMLElement>();
  let mounted = false;
  /** The newest native navigation number; it and older ones are ignored. */
  let generation = 0;

  const pane = $derived(navigation.kind === "starting" ? null : navigation.pane);
  /** Plane settings for the Agents, Work and Safety panes. */
  const session = new SettingsSession();
  /** The Plane a deep link asked for; the picker changes it afterwards. */
  let requestedPlane = $state<PlaneId | null>(null);

  // A Plane pane starts (or switches) the session; General and Connections never connect.
  $effect(() => {
    const current = pane;
    const plane = requestedPlane;
    if (current === null || !PLANE_PANES.has(current)) return;
    untrack(() => session.select(plane ?? session.planeId));
  });

  function reducedMotion(): boolean {
    return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
  }

  async function show(message: SettingsNavigation) {
    const next = Number(message.generation);
    if (!Number.isSafeInteger(next) || next <= generation) return;
    generation = next;
    const target = resolveTarget(message.target);
    if (message.target.plane_id && PLANE_PANES.has(target.pane)) requestedPlane = message.target.plane_id;
    await go(target.pane, target.section);
  }

  /** Shows a pane and moves focus to one of its section headings. */
  async function go(next: SettingsPane, section: SettingsSection | null) {
    navigation = { kind: "ready", pane: next };
    if (!section) return;
    await tick();
    if (!mounted) return;
    const heading = document.getElementById(sectionHeadingId(section));
    heading?.focus({ preventScroll: true });
    heading?.scrollIntoView({ block: "start", behavior: reducedMotion() ? "auto" : "smooth" });
  }

  function choose(next: SettingsPane) {
    navigation = { kind: "ready", pane: next };
    // Remembering the pane is a convenience; a failed write keeps the old one.
    rememberSettingsPane({ pane: next }).catch(() => undefined);
  }

  function focusNavigation() {
    const current = nav?.querySelector<HTMLButtonElement>('button[aria-current="page"]');
    (current ?? nav?.querySelector<HTMLButtonElement>("button"))?.focus();
  }

  function handleKey(event: KeyboardEvent) {
    if (!event.ctrlKey || event.altKey || event.metaKey) return;
    const key = event.key.toLowerCase();
    if (key === "w") {
      event.preventDefault();
      closeSettings().catch(() => undefined);
    } else if (key === "q" && !event.shiftKey && !event.repeat && !event.isComposing) {
      // Ctrl+Q quits Jet from either window; Runs keep going on their Planes.
      event.preventDefault();
      quitJet().catch(() => undefined);
    } else if (event.key === ",") {
      event.preventDefault();
      focusNavigation();
    }
  }

  onMount(() => {
    mounted = true;
    window.addEventListener("keydown", handleKey);
    watchSettingsNavigation((message) => void show(message))
      .then((initial) => {
        if (mounted) void show(initial);
      })
      .catch(() => {
        // Without a watcher, deep links cannot arrive; the panes still work.
        if (mounted) navigation = { kind: "failed", pane: "general" };
      });
    return () => {
      mounted = false;
      window.removeEventListener("keydown", handleKey);
      session.dispose();
    };
  });
</script>

<svelte:head>
  <title>Jet Settings</title>
</svelte:head>

<div class="settings-app">
  <nav bind:this={nav} aria-label="Settings">
    {#each panes as entry (entry.id)}
      <button
        aria-current={pane === entry.id ? "page" : undefined}
        disabled={pane === null}
        onclick={() => choose(entry.id)}
      >
        {entry.title}
      </button>
    {/each}
  </nav>

  <main aria-busy={pane === null}>
    {#if pane !== null}
      <h1>{paneTitle(pane)}</h1>
      {#if navigation.kind === "failed"}
        <p class="notice" role="status">
          Links from the main window can't open a specific setting right now. Close and reopen Settings to try again.
        </p>
      {/if}
      {#if pane === "general"}
        <GeneralPane />
      {:else if pane === "connections"}
        <ConnectionsPane />
      {:else if pane === "agents" || pane === "work" || pane === "safety"}
        <div class="plane-context">
          <PlanePicker
            planeId={session.planeId}
            onselect={(planeId, label) => {
              requestedPlane = planeId;
              session.select(planeId, label);
            }}
          />
          <PlaneBanners {session} onopenaudit={() => void go("safety", "audit")} />
        </div>
        {#if pane === "agents"}
          <AgentsPane {session} onopenpermissions={() => void go("safety", "permissions")} />
        {:else if pane === "work"}
          <WorkPane {session} onopen={(section) => void go(paneOf(section), section)} />
        {:else}
          <SafetyPane {session} onopen={(section) => void go(paneOf(section), section)} />
        {/if}
        <p class="disclosure">{LAST_WRITER_WINS}</p>
      {/if}
    {:else}
      <span class="visually-hidden">Loading Settings</span>
    {/if}
  </main>
</div>

<style>
  .settings-app {
    display: grid;
    grid-template-columns: 200px minmax(0, 1fr);
    height: 100vh;
    min-height: 0;
    background: var(--background);
    color: var(--text);
  }

  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 20px 10px;
    border-right: 1px solid var(--border);
    background: var(--sidebar);
  }

  nav button {
    min-height: 32px;
    padding: 6px 10px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }

  nav button:hover:not(:disabled) {
    background: var(--hover);
  }

  nav button[aria-current="page"] {
    background: var(--selected);
    font-weight: 620;
  }

  main {
    display: grid;
    align-content: start;
    gap: 24px;
    min-width: 0;
    min-height: 0;
    padding: 28px 32px 40px;
    overflow: auto;
    line-height: 1.6;
  }

  .plane-context {
    display: grid;
    gap: 12px;
  }

  .disclosure {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }

  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 680;
    letter-spacing: -0.02em;
  }

  main > :global(*) {
    max-width: 720px;
  }

  main :global(.settings-section) {
    display: grid;
    gap: 10px;
  }

  main :global(.settings-section h2) {
    margin: 0;
    font-size: 16px;
    font-weight: 650;
    scroll-margin-top: 16px;
  }

  main :global(.settings-section h2:focus-visible) {
    outline: 2px solid var(--focus);
    outline-offset: 4px;
  }

  main :global(.settings-section h3) {
    margin: 8px 0 0;
    font-size: 13px;
    font-weight: 650;
  }

  main :global(.settings-section p) {
    margin: 0;
    color: var(--muted);
  }

  main :global(.settings-section .toggle) {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text);
  }

  main :global(.settings-section input[type="checkbox"]) {
    width: 18px;
    height: 18px;
    accent-color: var(--accent);
  }

  @media (max-width: 760px) {
    .settings-app {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto minmax(0, 1fr);
    }

    nav {
      flex-direction: row;
      flex-wrap: wrap;
      padding: 10px 16px;
      border-right: 0;
      border-bottom: 1px solid var(--border);
    }

    main {
      padding: 20px 20px 32px;
    }
  }
</style>
