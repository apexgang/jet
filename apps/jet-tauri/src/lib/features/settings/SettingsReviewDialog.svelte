<script lang="ts">
  import { onMount, tick } from "svelte";

  import type { SettingKeyId, SettingsReview } from "$lib/jet/settings";
  import { LAST_WRITER_WINS, SETTING_PLACEMENT, sourceLabel, valueText } from "./model";

  let {
    review,
    settingKey,
    scopeLabel,
    planeLabel,
    projects,
    onconfirm,
    oncancel,
  }: {
    review: SettingsReview;
    settingKey: SettingKeyId;
    /** "This Plane" or the Project's name. */
    scopeLabel: string;
    planeLabel: string;
    projects: ReadonlyArray<{ id: string; name: string }>;
    onconfirm: () => void;
    oncancel: () => void;
  } = $props();

  let dialog = $state<HTMLDialogElement>();
  let confirmButton = $state<HTMLButtonElement>();
  let returnFocus: HTMLElement | null = null;
  const titleId = $props.id();
  const placement = $derived(SETTING_PLACEMENT[settingKey]);

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
          <dd>{valueText(settingKey, review.before.value)} ({sourceLabel(review.before.source, projects)})</dd>
        </div>
      {/if}
      <div>
        <dt>After</dt>
        <dd>{review.after === null ? "The inherited value" : valueText(settingKey, review.after)}</dd>
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
