<script lang="ts">
  import type { SettingsSession } from "./session.svelte";

  let {
    session,
    onopenaudit,
  }: {
    session: SettingsSession;
    /** Opens Safety › Audit. Absent when the audit is already on screen. */
    onopenaudit?: () => void;
  } = $props();
</script>

<!-- Controls are disabled by these states, never by a copied list of keys. -->
<div class="plane-banners" role="status">
  {#if session.planeState?.recovery === "read_only"}
    <p class="plane-banner">
      This Plane is in read-only recovery. You can look at its settings, but changes wait until it's restored.
    </p>
  {/if}
  {#if session.planeState?.security === "degraded"}
    <div class="plane-banner">
      <p>Changes to trust and policy are paused until this Plane's audit is repaired (Safety › Audit).</p>
      {#if onopenaudit}<button class="text-button" onclick={onopenaudit}>Open Audit</button>{/if}
    </div>
  {/if}
  {#if session.watch === "failed"}
    <div class="plane-banner">
      <p>
        Jet stopped checking {session.planeLabel} for changes, so changes are paused.
        {#if session.watchError}<code>{session.watchError.code}</code>{/if}
      </p>
      <button class="text-button" onclick={() => void session.showCurrent(null)}>Try again</button>
    </div>
  {/if}
</div>

<style>
  .plane-banners {
    display: grid;
    gap: 8px;
  }

  .plane-banners:empty {
    display: none;
  }

  .plane-banner {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin: 0;
    padding: 10px 12px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--warning) 12%, var(--raised));
    color: var(--text);
    font-size: 13px;
  }

  .plane-banner p {
    margin: 0;
    color: var(--text);
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
