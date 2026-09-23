<script lang="ts">
  import { onMount, tick } from "svelte";

  import type { SettingKeyId, SettingsReview } from "$lib/jet/settings";
  import {
    LAST_WRITER_WINS,
    SETTING_PLACEMENT,
    blockText,
    isBindingKey,
    isConsentKey,
    reviewConsequence,
    reviewDestructive,
    sourceLabel,
    valueText,
    type BindingOption,
  } from "./model";

  let {
    review,
    settingKey,
    scopeLabel,
    planeLabel,
    projects,
    bindings = [],
    block,
    onconfirm,
    oncancel,
  }: {
    review: SettingsReview;
    settingKey: SettingKeyId;
    /** "This Plane" or the Project's name. */
    scopeLabel: string;
    planeLabel: string;
    projects: ReadonlyArray<{ id: string; name: string }>;
    /** Account bindings, so binding and consent values show labels, not UUIDs. */
    bindings?: ReadonlyArray<BindingOption>;
    /** Changes are paused (read-only Recovery or a stale view): Change is disabled. */
    block: "read_only" | "stale" | null;
    onconfirm: () => void;
    oncancel: () => void;
  } = $props();

  let dialog = $state<HTMLDialogElement>();
  let confirmButton = $state<HTMLButtonElement>();
  let cancelButton = $state<HTMLButtonElement>();
  const titleId = $props.id();
  const placement = $derived(SETTING_PLACEMENT[settingKey]);
  const consequence = $derived(reviewConsequence(settingKey, review));
  /** A shorter retention deletes data for good: start on Cancel. */
  const destructive = $derived(reviewDestructive(settingKey, review));
  /** Binding and consent keys are Plane-only: clearing restores the empty built-in. */
  const clearedText = $derived(
    isBindingKey(settingKey) || isConsentKey(settingKey)
      ? valueText(settingKey, { type: "text", value: "" }, bindings)
      : "The inherited value",
  );

  // The row returns focus to its control once the change settles: the
  // control is locked while this dialog is open.
  function close(): void {
    if (dialog?.open) dialog.close();
  }

  function cancel(): void {
    close();
    oncancel();
  }

  function confirm(): void {
    if (block !== null) return;
    close();
    onconfirm();
  }

  onMount(() => {
    void tick().then(() => {
      dialog?.showModal();
      if (destructive || block !== null) cancelButton?.focus();
      else confirmButton?.focus();
    });
    return () => {
      if (dialog?.open) dialog.close();
    };
  });
</script>

<dialog
  bind:this={dialog}
  class="removal-dialog settings-review"
  aria-labelledby={titleId}
  oncancel={(event) => {
    event.preventDefault();
    cancel();
  }}
>
  <form method="dialog" onsubmit={(event) => event.preventDefault()}>
    <header>
      <h2 id={titleId}>Change “{placement.label}”?</h2>
      {#if consequence}<p>{consequence}</p>{/if}
    </header>

    <dl class="removal-facts">
      <div><dt>Plane</dt><dd>{planeLabel}</dd></div>
      <div><dt>Applies to</dt><dd>{scopeLabel}</dd></div>
      {#if review.before}
        <div>
          <dt>Now</dt>
          <dd>{valueText(settingKey, review.before.value, bindings)} ({sourceLabel(review.before.source, projects)})</dd>
        </div>
      {/if}
      <div>
        <dt>After</dt>
        <dd>{review.after === null ? clearedText : valueText(settingKey, review.after, bindings)}</dd>
      </div>
    </dl>

    <p class="disclosure">{LAST_WRITER_WINS}</p>
    {#if block !== null}
      <p class="disclosure" role="status">{blockText(block, planeLabel)}</p>
    {/if}

    <footer>
      <button bind:this={cancelButton} type="button" class="secondary-button" onclick={cancel}>Cancel</button>
      <button
        bind:this={confirmButton}
        type="button"
        class={destructive ? "danger-button" : "primary-button"}
        disabled={block !== null}
        onclick={confirm}
      >
        Change
      </button>
    </footer>
  </form>
</dialog>

<style>
  .disclosure {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }

  .settings-review dd {
    overflow: visible;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }
</style>
