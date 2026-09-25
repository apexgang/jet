<script lang="ts">
  import { onMount } from "svelte";

  import SettingsDialog from "$lib/features/settings/SettingsDialog.svelte";
  import type { AppUpdateSession } from "./updates.svelte";
  import { keepFocus } from "./keep-focus";
  import { downloadPercent, updateStatusText } from "./service-model";

  let { updates }: { updates: AppUpdateSession } = $props();

  const HEADING_ID = "app-updates-heading";

  onMount(() => void updates.start());

  const update = $derived(updates.update);
  const state = $derived(update?.state ?? null);
  const busy = $derived(updates.busy !== null);
  const percent = $derived(state?.kind === "downloading" ? downloadPercent(state.downloaded, state.total) : null);

  /**
   * The one main action. It stays the same button while the state moves on
   * (Check → Checking… → Install → Installing… → Restart), so focus stays on
   * it; while its work is under way it is `aria-disabled`, not `disabled`,
   * which would drop focus to the page.
   */
  const primary = $derived.by(() => {
    switch (state?.kind) {
      case "available":
        return { label: `Install Jet ${state.version}`, waiting: busy, run: () => void updates.install() };
      case "downloading":
        return { label: "Installing…", waiting: true, run: () => undefined };
      case "ready":
        return { label: "Restart Jet…", waiting: busy, run: () => updates.requestRestart() };
      case "checking":
        return { label: "Checking…", waiting: true, run: () => undefined };
      default:
        return { label: "Check for updates", waiting: busy, run: () => void updates.check() };
    }
  });

  const heading = () => document.getElementById(HEADING_ID);
</script>

<div class="app-updates" use:keepFocus={heading}>
  <h3 id={HEADING_ID} tabindex="-1">App updates</h3>
  {#if update === null || state === null}
    {#if updates.error}
      <p class="notice critical" role="status">
        Jet couldn't read its update state. <code>{updates.error.code}</code>
      </p>
      <div class="actions">
        <button class="secondary-button" onclick={() => void updates.watch()}>Try again</button>
      </div>
    {:else}
      <span class="loading-bar short" aria-hidden="true"></span>
    {/if}
  {:else}
    <!-- Spoken once per state: download progress stays out of this region. -->
    <p role="status" aria-live="polite">
      {updateStatusText(update)}
      {#if state.kind === "failed"}<code>{state.error.code}</code>{/if}
    </p>
    {#if state.kind === "downloading"}
      <div class="progress">
        <progress
          aria-label={`Downloading Jet ${state.version}`}
          max={state.total ?? undefined}
          value={state.total === null ? undefined : state.downloaded}
        >
          {percent === null ? "" : `${percent}%`}
        </progress>
        {#if percent !== null}<span class="quiet" aria-hidden="true">{percent}%</span>{/if}
      </div>
    {/if}
    {#if updates.error}
      <p class="notice critical" role="status">{updates.error.message} <code>{updates.error.code}</code></p>
    {/if}

    {#if state.kind !== "disabled"}
      <div class="actions">
        <button
          class="primary-button"
          aria-disabled={primary.waiting}
          aria-busy={state.kind === "checking" || state.kind === "downloading"}
          onclick={() => {
            if (!primary.waiting) primary.run();
          }}
        >
          {primary.label}
        </button>
        {#if state.kind === "available"}
          <button class="secondary-button" disabled={busy} onclick={() => void updates.check()}>
            Check for updates
          </button>
        {/if}
      </div>
      {#if state.kind === "available"}
        <p class="quiet">Installing a .deb or .rpm update asks for your password.</p>
      {/if}

      {#if updates.automatic.kind === "ready"}
        <label class="toggle">
          <input
            type="checkbox"
            checked={updates.automatic.preferences.checkForUpdates}
            disabled={updates.automatic.saving}
            onchange={(event) => void updates.setAutomatic(event.currentTarget.checked)}
          />
          Check for updates automatically
        </label>
        <p class="quiet">
          Jet checks once, shortly after it starts. The check asks github.com for the latest release, which shares this
          computer's address with GitHub. This choice is kept on this computer only.
        </p>
        {#if updates.automatic.notice}<p role="status">{updates.automatic.notice}</p>{/if}
      {:else if updates.automatic.kind === "failed"}
        <p class="notice" role="status">
          Jet couldn't read whether it checks for updates automatically. <code>{updates.automatic.error.code}</code>
        </p>
      {/if}
    {/if}
  {/if}
</div>

{#if updates.confirmingRestart && state?.kind === "ready"}
  <SettingsDialog
    title={`Restart Jet with version ${state.version}?`}
    lead="Every Jet window closes and opens again."
    focus="primary"
    returnFocus={[HEADING_ID]}
    focusKey="restart"
    oncancel={() => updates.cancelRestart()}
  >
    <p class="dialog-note">Tasks keep running on their Planes while Jet restarts.</p>
    {#snippet footer()}
      <button type="button" class="secondary-button" data-dialog-cancel onclick={() => updates.cancelRestart()}>
        Later
      </button>
      <button
        type="button"
        class="primary-button"
        data-dialog-primary
        disabled={updates.busy === "restarting"}
        onclick={() => void updates.restart()}
      >
        {updates.busy === "restarting" ? "Restarting…" : "Restart Jet"}
      </button>
    {/snippet}
  </SettingsDialog>
{/if}

<style>
  .app-updates {
    display: grid;
    gap: 10px;
  }

  .app-updates h3,
  .app-updates p {
    margin: 0;
  }

  .app-updates code {
    margin-left: 4px;
    color: var(--quiet);
    font-size: 11px;
  }

  .progress {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  progress {
    width: min(100%, 360px);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .quiet {
    color: var(--muted);
  }
</style>
