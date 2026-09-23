<script lang="ts">
  import type { SystemHealth } from "$lib/jet/system";
  import SectionState from "$lib/features/settings/SectionState.svelte";
  import { withIssues } from "$lib/features/settings/model";
  import { budgetText, collectResultText, diskPressureText } from "./model";
  import type { SystemSession } from "./session.svelte";

  let { system, planeLabel }: { system: SystemSession; planeLabel: string } = $props();

  const view = $derived(withIssues(system.health, ["storage"]));
  const pressure = $derived(diskPressureText(system.diskPressureAt));
  const collecting = $derived(system.collect.kind === "collecting");
</script>

<!-- Mounted inside Settings › Safety › Storage, above its size rows. -->
<div class="storage-health">
  <p>
    Jet keeps free space for your computer and refuses new work when the disk is nearly full. Jet reports this only
    when it refuses work.
  </p>
  {#if pressure}
    <p class="notice warning" role="status">{pressure}</p>
  {/if}
  <div class="collect">
    <div class="collect-copy">
      <button class="secondary-button" disabled={collecting} onclick={() => void system.freeSpace()}>
        {collecting ? "Freeing space…" : "Free disposable space"}
      </button>
      <p>Jet removes unused temporary files older than a day. Tasks, workspaces, and recovery snapshots aren't touched.</p>
    </div>
    <div role="status">
      {#if system.collect.kind === "done"}
        <p>{collectResultText(system.collect.removed)}</p>
      {:else if system.collect.kind === "failed"}
        <p class="section-error">
          {system.collect.error.message} <code>{system.collect.error.code}</code>
        </p>
      {/if}
    </div>
  </div>
  <SectionState state={view} title="Storage health" {planeLabel} onretry={() => void system.load()}>
    {#snippet children(health: SystemHealth)}
      {#if budgetText(health.storage.disposableMiB)}
        <p>{budgetText(health.storage.disposableMiB)}</p>
      {/if}
    {/snippet}
  </SectionState>
</div>

<style>
  .storage-health,
  .collect,
  .collect-copy {
    display: grid;
    gap: 8px;
  }

  .collect-copy button {
    justify-self: start;
  }

  .warning {
    padding: 10px 12px;
    /* Transparent until a forced palette paints it, where the tint is lost. */
    border: 1px solid transparent;
    border-radius: 8px;
    background: color-mix(in srgb, var(--warning) 12%, var(--raised));
    color: var(--text) !important;
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
