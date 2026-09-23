<script lang="ts">
  import { onMount, tick } from "svelte";

  import type { SettingKeyId, SettingsReview } from "$lib/jet/settings";
  import {
    LAST_WRITER_WINS,
    SETTING_PLACEMENT,
    isBindingKey,
    isConsentKey,
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
    onconfirm: () => void;
    oncancel: () => void;
  } = $props();

  let dialog = $state<HTMLDialogElement>();
  let confirmButton = $state<HTMLButtonElement>();
  let returnFocus: HTMLElement | null = null;
  const titleId = $props.id();
  const placement = $derived(SETTING_PLACEMENT[settingKey]);
  /** Binding and consent keys are Plane-only: clearing restores the empty built-in. */
  const clearedText = $derived(
    isBindingKey(settingKey) || isConsentKey(settingKey)
      ? valueText(settingKey, { type: "text", value: "" }, bindings)
      : "The inherited value",
  );

  function close(): void {
    if (dialog?.open) dialog.close();
    returnFocus?.focus();
    returnFocus = null;
  }

  function cancel(): void {
    close();
    oncancel();
  }

  function confirm(): void {
    close();
    onconfirm();
  }

  onMount(() => {
    returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    void tick().then(() => {
      dialog?.showModal();
      confirmButton?.focus();
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
      {#if placement.consequence}<p>{placement.consequence}</p>{/if}
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

    <footer>
      <button type="button" class="secondary-button" onclick={cancel}>Cancel</button>
      <button bind:this={confirmButton} type="button" class="primary-button" onclick={confirm}>Change</button>
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
