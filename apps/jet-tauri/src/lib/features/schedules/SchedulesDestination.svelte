<script lang="ts">
  import { schedulesAvailability } from "$lib/features/settings/model";

  let {
    standalone = false,
    changed = false,
  }: {
    /** The main-window destination has its own `h1`; in Settings the body sits under the section's `h2`. */
    standalone?: boolean;
    /** A `schedule.*` event arrived on the selected Plane. */
    changed?: boolean;
  } = $props();
</script>

{#snippet body()}
  <p class="schedules-summary">{schedulesAvailability.summary}</p>
  {#if changed}
    <p class="notice" role="status">Schedules changed on this Plane.</p>
  {/if}
  <details class="schedules-why">
    <summary>Why</summary>
    <p>{schedulesAvailability.detail}</p>
  </details>
{/snippet}

{#if standalone}
  <section class="schedules-destination" aria-labelledby="schedules-title">
    <header class="setup-header">
      <div>
        <h1 id="schedules-title" tabindex="-1">Schedules</h1>
        <p>Tasks a Plane starts on its own, on a schedule.</p>
      </div>
    </header>
    <div class="setup-content schedules-body">
      {@render body()}
    </div>
  </section>
{:else}
  {@render body()}
{/if}

<style>
  .schedules-destination {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    min-width: 0;
    min-height: 0;
    overflow: auto;
    background: var(--background);
  }

  .schedules-body {
    display: grid;
    align-content: start;
    gap: 12px;
  }

  .schedules-summary {
    margin: 0;
    color: var(--text);
  }

  .schedules-why {
    color: var(--muted);
    font-size: 13px;
  }

  .schedules-why summary {
    width: fit-content;
    color: var(--accent-strong);
    cursor: pointer;
  }

  .schedules-why p {
    margin: 6px 0 0;
    max-width: 62ch;
  }

  .notice {
    margin: 0;
  }
</style>
