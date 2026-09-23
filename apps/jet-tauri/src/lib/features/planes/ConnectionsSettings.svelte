<script lang="ts">
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import { LOCAL_PLANE } from "$lib/jet/planes";
  import { planeStatus } from "./model";

  let { session }: { session: DesktopSession } = $props();
  const planes = $derived(session.planes);
  const local = $derived(planes.plane(LOCAL_PLANE));
  const remoteCount = $derived(planes.planes.filter((plane) => plane.kind === "remote").length);
  const attention = $derived(planes.attentionCount);

  const summary = $derived.by(() => {
    const parts = [`This computer: ${local ? planeStatus(local).text : "Not read yet"}`];
    parts.push(remoteCount === 1 ? "1 remote Plane" : `${remoteCount} remote Planes`);
    if (attention > 0) parts.push(`${attention} need${attention === 1 ? "s" : ""} attention`);
    return parts.join(" · ");
  });

  const keyState = $derived.by(() => {
    switch (planes.snapshot?.identity.key) {
      case "present":
        return "Stored in your system keyring";
      case "not_created":
        return "Not created yet";
      case "session_only":
        return "Kept for this session only";
      case "unavailable":
        return "Keyring unavailable";
      case "locked":
        return "Keyring locked";
      case "unsupported":
        return "No supported secure storage on this computer";
      default:
        return "Checked when you pair with a remote Plane";
    }
  });

  const clientPrefix = $derived(planes.snapshot?.identity.clientId.slice(0, 8) ?? null);
</script>

<section class="connections-settings" aria-labelledby="connections-title">
  <h1 id="connections-title">Connections</h1>
  <p class="connections-summary" role="status">{summary}</p>
  <dl class="connections-facts">
    <div>
      <dt>This computer's pairing key</dt>
      <dd>
        {keyState}
        {#if planes.snapshot?.identity.fingerprint}
          <code>{planes.snapshot.identity.fingerprint}</code>
        {/if}
      </dd>
    </div>
    {#if clientPrefix}
      <div><dt>Client</dt><dd><code>{clientPrefix}</code></dd></div>
    {/if}
  </dl>
  <p class="connections-help">
    Pair other computers, and manage the computers that can control each Plane, from Planes.
  </p>
  <div class="connections-actions">
    <button class="secondary-button" onclick={() => session.openPlanes({ focus: "detail" })}>Open Planes</button>
    <button
      class="secondary-button"
      onclick={() => session.openPlanes({ planeId: LOCAL_PLANE, focus: "clients" })}
    >
      Paired computers on this computer
    </button>
  </div>
</section>

<style>
  .connections-settings {
    display: grid;
    gap: 12px;
    max-width: 720px;
    padding: 32px 32px 0;
    line-height: 1.6;
  }

  h1 {
    margin: 0;
    font-size: 24px;
  }

  .connections-summary {
    margin: 0;
    color: var(--text);
  }

  .connections-help {
    margin: 0;
    color: var(--muted);
  }

  .connections-facts {
    display: grid;
    gap: 6px;
    margin: 0;
  }

  .connections-facts div {
    display: grid;
    grid-template-columns: 200px minmax(0, 1fr);
    gap: 12px;
  }

  .connections-facts dt {
    color: var(--muted);
  }

  .connections-facts dd {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin: 0;
  }

  .connections-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
</style>
