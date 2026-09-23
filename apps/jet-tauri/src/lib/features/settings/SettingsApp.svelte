<script lang="ts">
  import { onMount, tick } from "svelte";

  import {
    closeSettings,
    rememberSettingsPane,
    watchSettingsNavigation,
    type SettingsNavigation,
    type SettingsPane,
  } from "$lib/jet/settings-window";
  import ConnectionsPane from "./ConnectionsPane.svelte";
  import GeneralPane from "./GeneralPane.svelte";
  import { landedPanes, paneTitle, resolveTarget, sectionHeadingId } from "./model";

  const panes = landedPanes();

  type Navigation =
    | { kind: "starting" }
    | { kind: "ready"; pane: SettingsPane }
    | { kind: "failed"; pane: SettingsPane };

  let navigation = $state<Navigation>({ kind: "starting" });
  let nav = $state<HTMLElement>();
  let mounted = false;
  /** The newest native watcher generation; older messages are ignored. */
  let generation = 0;

  const pane = $derived(navigation.kind === "starting" ? null : navigation.pane);

  function reducedMotion(): boolean {
    return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
  }

  async function show(message: SettingsNavigation) {
    const next = Number(message.generation);
    if (!Number.isSafeInteger(next) || next < generation) return;
    generation = next;
    const target = resolveTarget(message.target);
    navigation = { kind: "ready", pane: target.pane };
    if (!target.section) return;
    await tick();
    if (!mounted) return;
    const heading = document.getElementById(sectionHeadingId(target.section));
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
    if (event.key.toLowerCase() === "w") {
      event.preventDefault();
      closeSettings().catch(() => undefined);
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
    outline: 2px solid var(--accent);
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
