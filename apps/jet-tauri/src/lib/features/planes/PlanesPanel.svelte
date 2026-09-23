<script lang="ts">
  import { tick, untrack } from "svelte";

  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import AddPlaneDialog from "./AddPlaneDialog.svelte";
  import { planeStatus } from "./model";
  import PlaneDetail from "./PlaneDetail.svelte";

  let { session }: { session: DesktopSession } = $props();
  const planes = $derived(session.planes);
  const remoteCount = $derived(planes.planes.filter((plane) => plane.kind === "remote").length);
  const maximum = $derived(planes.snapshot?.maximumRemotePlanes ?? 16);
  const addBlockedReason = $derived(
    planes.snapshot?.identity.key === "unsupported"
      ? "This computer has no supported secure storage for a pairing key."
      : remoteCount >= maximum
        ? `This computer already has the maximum of ${maximum} remote Planes. Forget one first.`
        : null,
  );

  let addButton = $state<HTMLButtonElement>();

  // Deep links: "add" opens the wizard (or focuses the disabled button and
  // its reason); "repair" opens it at Pair again for the selected Plane.
  $effect(() => {
    const request = planes.focusRequest;
    if (request?.section !== "add" && request?.section !== "repair") return;
    untrack(() => {
      planes.focusRequest = null;
      if (request.section === "repair") {
        void planes.startRepair(planes.selectedPlaneId);
      } else if (addBlockedReason) {
        void tick().then(() => addButton?.focus());
      } else {
        planes.startAdd();
      }
    });
  });

  $effect(() => {
    if (!planes.detail) untrack(() => planes.select(planes.selectedPlaneId));
  });
</script>

<section class="planes-panel" aria-labelledby="planes-title">
  <header class="setup-header">
    <div>
      <h1 id="planes-title">Planes</h1>
      <p>Planes run your tasks. This computer is always listed; remote Planes you pair with appear here too.</p>
    </div>
    <button class="secondary-button" onclick={() => { void session.refreshPlanes(); void planes.loadDetail(); void planes.pairing.load(); }}>
      Refresh
    </button>
  </header>

  {#if planes.notice}
    <div class="notice planes-notice" role="status">
      <p>{planes.notice}</p>
      <button class="text-button" onclick={() => (planes.noticeDismissed = true)}>Dismiss</button>
    </div>
  {/if}

  {#if planes.error && !planes.snapshot}
    <div class="setup-failure" role="alert">
      <div>
        <h2>Planes couldn't be listed</h2>
        <p>{planes.error.message}</p>
        <code>{planes.error.code}</code>
      </div>
      <button class="primary-button" onclick={() => session.refreshPlanes()}>Try again</button>
    </div>
  {:else}
    <div class="planes-layout">
      <nav class="planes-list" aria-label="Planes list">
        <ul>
          {#each planes.planes as plane (plane.planeId)}
            {@const status = planeStatus(plane)}
            <li>
              <button
                class:active={plane.planeId === planes.selectedPlaneId}
                aria-current={plane.planeId === planes.selectedPlaneId ? "true" : undefined}
                onclick={() => planes.select(plane.planeId)}
              >
                <span class="plane-row-label">{plane.label}</span>
                <span class="plane-row-status">
                  <span
                    class:online={status.tone === "ok"}
                    class:failed={status.tone === "danger"}
                    class="status-dot"
                    aria-hidden="true"
                  ></span>
                  {status.text}
                  {#if plane.credential === "session"}
                    <span class="plane-tag">This session only</span>
                  {/if}
                </span>
                {#if status.attention}
                  <span class="attention-count" aria-label="Needs attention">!</span>
                {/if}
              </button>
            </li>
          {/each}
        </ul>
        {#if remoteCount === 0}
          <p class="planes-empty">No remote Planes. Add one to see its tasks here.</p>
        {/if}
        {#if addBlockedReason}
          <button
            bind:this={addButton}
            class="secondary-button"
            aria-disabled="true"
            aria-describedby="add-plane-reason"
            onclick={(event) => event.preventDefault()}
          >
            Add a Plane
          </button>
          <p id="add-plane-reason" class="planes-reason">{addBlockedReason}</p>
        {:else}
          <button bind:this={addButton} class="secondary-button" onclick={() => planes.startAdd()}>
            Add a Plane
          </button>
        {/if}
      </nav>

      <PlaneDetail {session} />
    </div>
  {/if}
</section>

<AddPlaneDialog {planes} />

<style>
  .planes-panel {
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr);
    min-width: 0;
    min-height: 0;
    overflow: auto;
    background: var(--background);
  }

  .planes-notice {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin: 0 32px 16px;
  }

  .planes-notice p {
    margin: 0;
  }

  .planes-layout {
    display: grid;
    grid-template-columns: 240px minmax(0, 1fr);
    align-items: start;
    gap: 24px;
    padding: 0 32px 32px;
  }

  .planes-list {
    display: grid;
    gap: 10px;
  }

  .planes-list ul {
    display: grid;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .planes-list li button {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 4px 8px;
    width: 100%;
    padding: 10px 12px;
    border: 1px solid var(--border-soft);
    border-radius: 8px;
    background: var(--raised);
    color: var(--text);
    text-align: left;
  }

  .planes-list li button:hover {
    background: var(--hover);
  }

  .planes-list li button.active {
    border-color: var(--accent);
    background: var(--selected);
  }

  .plane-row-label {
    overflow: hidden;
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .plane-row-status {
    display: flex;
    flex-wrap: wrap;
    grid-column: 1;
    align-items: center;
    gap: 6px;
    color: var(--muted);
    font-size: 12px;
  }

  .planes-list .attention-count {
    grid-column: 2;
    grid-row: 1 / span 2;
    align-self: center;
  }

  .plane-tag {
    padding: 1px 6px;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--muted);
    font-size: 10px;
  }

  .planes-empty,
  .planes-reason {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }

  .planes-list > .secondary-button[aria-disabled="true"] {
    justify-self: start;
    opacity: 0.55;
  }

  @media (max-width: 1100px) {
    .planes-layout {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
