<script lang="ts">
  import type { PlaneId } from "$lib/jet/planes";
  import type { SettingsTarget } from "$lib/jet/settings-window";
  import type { PlaneHealth } from "./health.svelte";
  import { collectResultText, noticeLink, noticeText } from "./model";

  let {
    health,
    planeId,
    planeLabel,
    openSettings = null,
  }: {
    health: PlaneHealth;
    planeId: PlaneId;
    planeLabel: string;
    /** Settings links render only when the Settings window can be opened. */
    openSettings?: ((target: SettingsTarget) => void) | null;
  } = $props();

  let region = $state<HTMLElement>();

  const notice = $derived(health.notice(planeId));
  const link = $derived(notice && openSettings ? noticeLink(notice.kind, planeId) : null);
  const collect = $derived(health.collectState(planeId));
  const critical = $derived(notice?.kind === "read_only" || notice?.kind === "ledger_corrupt");

  // Needs attention routes here: take a pending focus request once shown.
  $effect(() => {
    if (!health.focusPending || !region) return;
    const target = region;
    health.focusPending = false;
    queueMicrotask(() => target.focus());
  });
</script>

{#if notice}
  <section
    bind:this={region}
    class="notice health-notice"
    class:critical
    tabindex="-1"
    aria-labelledby={`plane-health-${planeId}`}
  >
    <p id={`plane-health-${planeId}`} role="status">{noticeText(notice, planeLabel)}</p>
    <div class="health-actions">
      {#if notice.kind === "disk_pressure"}
        <button
          class="text-button"
          disabled={collect.kind === "collecting"}
          onclick={() => void health.collect(planeId)}
        >{collect.kind === "collecting" ? "Freeing space…" : "Free space"}</button>
      {/if}
      {#if link && openSettings}
        {@const open = openSettings}
        <button class="text-button" onclick={() => open(link.target)}>{link.label}</button>
      {/if}
      {#if notice.kind === "disk_pressure"}
        <button class="text-button" onclick={() => health.dismiss(planeId)}>Dismiss</button>
      {/if}
    </div>
    {#if notice.kind === "disk_pressure"}
      {#if collect.kind === "done"}
        <p role="status">{collectResultText(collect.removed)}</p>
      {:else if collect.kind === "failed"}
        <p role="alert">{collect.error.message}</p>
      {:else}
        <p>Jet removes unused temporary files older than a day. Tasks, workspaces, and recovery snapshots aren't touched.</p>
      {/if}
    {/if}
  </section>
{/if}

<style>
  .health-notice {
    display: grid;
    gap: 6px;
  }

  .health-notice:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .health-notice > p:first-child {
    margin: 0;
    color: var(--text);
  }

  .health-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
  }
</style>
