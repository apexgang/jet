<script lang="ts">
  import { tick } from "svelte";
  import { paneTitle, settingsTargetForError } from "$lib/features/settings/model";
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import type { PublicError } from "$lib/jet/bridge";
  import type { TrashMode } from "$lib/jet/retention";
  import {
    SNAPSHOT_TEXT,
    STOP_ACKNOWLEDGEMENT,
    UNCERTAIN_TEXT,
    UNCHECKED_TEXT,
    auditLine,
    formatDate,
    modeConsequence,
    modeName,
    moveRefusalReviewsAgain,
    moveRefusalText,
    protectionLine,
    refusalLinkLabel,
  } from "./model";

  let {
    session,
    returnFocus = null,
  }: {
    session: DesktopSession;
    /** The control that opened the dialog; focus goes back to it on close. */
    returnFocus?: HTMLElement | null;
  } = $props();

  let element = $state<HTMLDialogElement>();
  let forgetRadio = $state<HTMLInputElement>();
  let cancelButton = $state<HTMLButtonElement>();

  const trash = $derived(session.trash);
  const dialog = $derived(trash.dialog);
  const kind = $derived(trash.dialog.kind);
  const now = $derived.by(() => {
    void dialog;
    return Date.now();
  });
  /** The states that show the full review. */
  const reviewing = $derived(
    dialog.kind === "review" ||
      dialog.kind === "review_unchecked" ||
      dialog.kind === "sending" ||
      dialog.kind === "uncertain"
      ? dialog
      : null,
  );
  const preview = $derived("preview" in dialog ? dialog.preview : null);
  const planeLabel = $derived(preview?.planeLabel ?? ("planeId" in dialog ? session.planes.label(dialog.planeId) : ""));

  // Open and close the native modal with the state; focus follows the state:
  // the pre-selected Forget option in a review, otherwise Cancel.
  $effect(() => {
    const target = element;
    if (!target) return;
    if (kind !== "closed" && !target.open) target.showModal();
    if (kind === "closed" && target.open) {
      target.close();
      returnFocus?.focus();
    }
  });

  $effect(() => {
    const current = kind;
    if (current === "closed" || current === "sending") return;
    void tick().then(() => {
      if (current === "review" || current === "review_unchecked") forgetRadio?.focus();
      else cancelButton?.focus();
    });
  });

  function close(): void {
    trash.closeMove();
  }

  function choose(mode: TrashMode): void {
    trash.chooseMode(mode);
  }

  function link(error: PublicError) {
    const target = settingsTargetForError(error);
    return target ? { target, label: refusalLinkLabel(target.section, paneTitle(target.pane)) } : null;
  }
</script>

<dialog
  bind:this={element}
  aria-labelledby="move-trash-title"
  class="removal-dialog move-trash-dialog"
  oncancel={(event) => {
    event.preventDefault();
    close();
  }}
>
  <form method="dialog" onsubmit={(event) => event.preventDefault()}>
    <header>
      <h2 id="move-trash-title">
        {preview ? `Move “${preview.title}” to Jet Trash?` : "Move to Jet Trash"}
      </h2>
      {#if planeLabel}
        <p>On {planeLabel}</p>
      {/if}
    </header>

    {#if dialog.kind === "loading"}
      <p role="status">Checking what this task still needs…</p>
    {:else if dialog.kind === "failed"}
      <p class="removal-obstacles" role="alert">{moveRefusalText(dialog.error)}</p>
    {:else if dialog.kind === "already"}
      {@const entry = dialog.preview.trash}
      {#if entry}
        {@const deleted = formatDate(entry.expiresAtUnixMs, now)}
        <p role="status">
          This task is already in Jet Trash and will be deleted on
          <time datetime={deleted.iso}>{deleted.absolute}</time> ({deleted.relative}).
        </p>
      {/if}
    {:else if dialog.kind === "trashed"}
      {@const deleted = formatDate(dialog.entry.expiresAtUnixMs, now)}
      <p role="status">
        Moved to Jet Trash. Jet deletes it on <time datetime={deleted.iso}>{deleted.absolute}</time>
        ({deleted.relative}). You can restore it until then.
      </p>
    {:else if dialog.kind === "stale" || dialog.kind === "refused"}
      {@const refusal = link(dialog.error)}
      <div class="removal-obstacles" role="alert">
        <p>{moveRefusalText(dialog.error)}</p>
        {#if refusal}
          <button class="text-button" type="button" onclick={() => void session.openSettings(refusal.target)}>
            {refusal.label}
          </button>
        {/if}
      </div>
    {:else if reviewing}
      {@const locked = reviewing.kind === "uncertain" || reviewing.kind === "sending"}
      {@const review = reviewing.preview}
      <fieldset class="move-trash-modes" disabled={locked}>
        <legend class="visually-hidden">How to remove this task</legend>
        <label class="move-trash-mode">
          <input
            bind:this={forgetRadio}
            type="radio"
            name="move-trash-mode"
            value="forget"
            checked={reviewing.mode === "forget"}
            onchange={() => choose("forget")}
          />
          <span>
            <strong>Forget in Jet</strong>
            <small>{modeConsequence("forget", review, now)}</small>
          </span>
        </label>
        <label class="move-trash-mode">
          <input
            type="radio"
            name="move-trash-mode"
            value="delete_everywhere"
            checked={reviewing.mode === "delete_everywhere"}
            onchange={() => choose("delete_everywhere")}
          />
          <span>
            <strong>Delete everywhere</strong>
            <small>{modeConsequence("delete_everywhere", review, now)}</small>
          </span>
        </label>
      </fieldset>
      <p class="move-trash-note">The Plane sets the exact date when the task moves.</p>

      {#if reviewing.kind === "uncertain"}
        <div class="removal-obstacles" role="alert">
          <p>{UNCERTAIN_TEXT}</p>
          <p>You chose {modeName(reviewing.mode)}. Jet must confirm it before you choose differently.</p>
        </div>
      {/if}

      {#if review.workspaceUnchecked}
        <p class="removal-obstacles">{UNCHECKED_TEXT}</p>
      {:else if review.protections && review.protections.length > 0}
        <ul class="move-trash-protections">
          {#each review.protections as protection (protection.kind)}
            <li>{protectionLine(protection.kind)}</li>
          {/each}
        </ul>
      {/if}
      {#if !review.workspaceUnchecked && auditLine(review.auditRecords)}
        <p class="move-trash-note">{auditLine(review.auditRecords)}</p>
      {/if}
      <p class="move-trash-note">{SNAPSHOT_TEXT(review.planeLabel)}</p>

      {#if reviewing.mode === "delete_everywhere" && review.stopAcknowledgementRequired}
        <label class="checkbox-row">
          <input
            type="checkbox"
            checked={reviewing.stopAcknowledged}
            disabled={locked}
            onchange={(event) => trash.acknowledgeStop(event.currentTarget.checked)}
          />
          <span>{STOP_ACKNOWLEDGEMENT}</span>
        </label>
      {/if}
    {/if}

    <footer>
      {#if dialog.kind === "trashed"}
        <button bind:this={cancelButton} class="primary-button" type="button" onclick={close}>Close</button>
      {:else}
        <button
          bind:this={cancelButton}
          class="secondary-button"
          type="button"
          disabled={dialog.kind === "sending"}
          onclick={close}
        >{dialog.kind === "uncertain" ? "Close" : "Cancel"}</button>
        {#if dialog.kind === "already"}
          <button class="primary-button" type="button" onclick={() => void trash.restoreFromDialog()}>Restore</button>
        {:else if dialog.kind === "stale" || dialog.kind === "failed" || (dialog.kind === "refused" && moveRefusalReviewsAgain(dialog.error))}
          <button class="primary-button" type="button" onclick={() => void trash.reviewAgain()}>Review again</button>
        {:else if reviewing}
          <button
            class={reviewing.mode === "delete_everywhere" ? "danger-button" : "primary-button"}
            type="button"
            disabled={!trash.canConfirm || dialog.kind === "sending"}
            onclick={() => void trash.confirm()}
          >
            {#if dialog.kind === "sending"}
              Sending…
            {:else if dialog.kind === "uncertain"}
              Try again
            {:else}
              {reviewing.mode === "delete_everywhere" ? "Delete everywhere" : "Move to Trash"}
            {/if}
          </button>
        {/if}
      {/if}
    </footer>
  </form>
</dialog>
