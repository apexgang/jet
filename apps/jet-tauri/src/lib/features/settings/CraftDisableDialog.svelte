<script lang="ts">
  import type { CraftDisableMode, CraftView } from "$lib/jet/agents";
  import type { AgentsSession } from "./agents-session.svelte";
  import { craftTitle } from "./agents-model";
  import { blockText } from "./model";
  import SettingsDialog from "./SettingsDialog.svelte";

  let {
    agents,
    craft,
    planeLabel,
    block,
    onclose,
  }: {
    agents: AgentsSession;
    craft: CraftView;
    planeLabel: string;
    /** Changes are paused (read-only Recovery or a stale view). */
    block: "read_only" | "stale" | null;
    onclose: () => void;
  } = $props();

  let mode = $state<CraftDisableMode>("wait");
  const name = $derived(craftTitle(craft));
  const operation = $derived(agents.operationFor({ action: "disable", craftId: craft.craftId }));
  const busy = $derived(operation.kind === "preparing" || operation.kind === "applying");
  const groupId = $props.id();

  function cancel(): void {
    if (busy) return;
    // The refusal was read here; the section needn't repeat it.
    if (operation.kind === "refused") agents.dismiss();
    onclose();
  }

  async function disable(): Promise<void> {
    if (block !== null) return;
    await agents.disableCraft(craft.craftId, mode);
    // The outcome is shown in the Harnesses section; a refusal stays here.
    if (agents.operationFor({ action: "disable", craftId: craft.craftId }).kind !== "refused") onclose();
  }
</script>

<SettingsDialog title={`Disable ${name}?`} focus="cancel" returnFocus={["section-harnesses"]} oncancel={cancel}>
  <p class="dialog-alert">
    Jet can't turn {name} back on for this Plane. Reinstalling doesn't undo this.
  </p>

  <dl class="removal-facts">
    <div><dt>Plane</dt><dd>{planeLabel}</dd></div>
    <div><dt>Craft package</dt><dd>{craft.craftId} · {craft.version}</dd></div>
  </dl>

  <fieldset class="modes" aria-describedby={`${groupId}-note`}>
    <legend>Tasks using {name}</legend>
    <label class="mode">
      <input type="radio" name={`${groupId}-mode`} value="wait" bind:group={mode} disabled={busy} />
      <span>Let running tasks finish first</span>
    </label>
    <label class="mode">
      <input type="radio" name={`${groupId}-mode`} value="force" bind:group={mode} disabled={busy} />
      <span class="destructive">Stop it now <small>Tasks using it will need your attention.</small></span>
    </label>
  </fieldset>
  <p class="dialog-note" id={`${groupId}-note`}>
    If it was already stopped, choosing “Let running tasks finish first” changes nothing.
  </p>

  {#if block !== null}
    <p class="dialog-note" role="status">{blockText(block, planeLabel)}</p>
  {/if}

  {#if operation.kind === "refused"}
    <p class="dialog-alert" role="alert">{operation.error.message} <code>{operation.error.code}</code></p>
  {/if}

  {#snippet footer()}
    <button type="button" class="secondary-button" data-dialog-cancel disabled={busy} onclick={cancel}>Cancel</button>
    <button
      type="button"
      class="danger-button"
      data-dialog-primary
      disabled={busy || block !== null}
      onclick={() => void disable()}
    >
      {busy ? "Disabling…" : `Disable ${name}`}
    </button>
  {/snippet}
</SettingsDialog>

<style>
  .modes {
    display: grid;
    gap: 8px;
    margin: 0;
    padding: 0;
    border: 0;
  }

  legend {
    margin-bottom: 6px;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
  }

  .mode {
    display: flex;
    align-items: start;
    gap: 10px;
    color: var(--text) !important;
    font-size: 13px !important;
    font-weight: 500 !important;
  }

  .mode input[type="radio"] {
    width: auto;
    margin: 3px 0 0;
    accent-color: var(--accent);
  }

  .mode small {
    display: block;
    color: var(--muted);
    font-size: 12px;
  }

  .destructive {
    color: var(--danger);
  }
</style>
