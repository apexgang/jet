<script lang="ts">
  import { tick, untrack } from "svelte";

  import { CLIENTS_ANCHORS, focusLost, restoreFocus } from "./focus";
  import { formatFingerprint, identityPrefix, planeErrorCopy } from "./model";
  import type { OwnerPairing } from "./pairing.svelte";

  let { pairing }: { pairing: OwnerPairing } = $props();

  const change = $derived(pairing.change);
  const review = $derived(change.kind === "none" || change.kind === "preparing" ? null : change.review);
  /** Revoke, and disabling this computer over its own connection, need a modal. */
  const modalChange = $derived(
    review !== null &&
      (review.change === "revoke" || (review.change === "disable" && review.viaThisPlaneConnection)),
  );
  const modalOpen = $derived(
    modalChange &&
      (change.kind === "review" ||
        change.kind === "sending" ||
        (change.kind === "done" && change.receipt.kind === "applied_unverified")),
  );
  const fingerprint = $derived(review ? formatFingerprint(review.fingerprint) : "");
  const who = $derived(review?.isThisComputer ? "this computer" : fingerprint);
  const planePrefix = $derived(identityPrefix(review?.planeIdentity ?? null));

  let dialog = $state<HTMLDialogElement>();
  let cancelButton = $state<HTMLButtonElement>();
  let closeButton = $state<HTMLButtonElement>();
  let returnFocus: HTMLElement | null = null;

  $effect(() => {
    const open = modalOpen;
    const done = change.kind === "done";
    untrack(() => {
      if (open && dialog && !dialog.open) {
        returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        void tick().then(() => {
          if (!dialog || dialog.open) return;
          dialog.showModal();
          (done ? closeButton : cancelButton)?.focus();
        });
      } else if (open && done) {
        void tick().then(() => closeButton?.focus());
      } else if (!open && dialog?.open) {
        dialog.close();
        // An applied revoke removes the row that held "Revoke…" once the
        // list reloads, so focus goes to the list heading instead.
        const removed = change.kind === "done" && change.receipt.kind === "applied" && change.review.change === "revoke";
        const target = removed ? null : returnFocus;
        returnFocus = null;
        void tick().then(() => restoreFocus(target, CLIENTS_ANCHORS));
      }
    });
  });

  // The inline Disable review appears after the list: move focus to it, and
  // back to where it was opened from (or the list heading) when it ends.
  let inlineHeading = $state<HTMLHeadingElement>();
  let inlineReturn: HTMLElement | null = null;
  const inlineOpen = $derived(
    review !== null && !modalChange && (change.kind === "review" || change.kind === "sending"),
  );
  $effect(() => {
    const open = inlineOpen;
    untrack(() => {
      if (open && inlineReturn === null) {
        inlineReturn = document.activeElement instanceof HTMLElement ? document.activeElement : document.body;
        void tick().then(() => inlineHeading?.focus());
      } else if (!open && inlineReturn !== null) {
        const target = inlineReturn === document.body ? null : inlineReturn;
        inlineReturn = null;
        void tick().then(() => {
          if (focusLost()) restoreFocus(target, CLIENTS_ANCHORS);
        });
      }
    });
  });

  const sending = $derived(change.kind === "sending");

  function dismiss(): void {
    if (change.kind === "sending") return;
    pairing.cancelChange();
  }

  /** Busy buttons stay focusable (aria-disabled), so focus never drops out. */
  function execute(): void {
    if (change.kind === "sending") return;
    void pairing.executeChange();
  }
</script>

<dialog
  bind:this={dialog}
  class="removal-dialog client-change-dialog"
  aria-labelledby="client-change-title"
  oncancel={(event) => { event.preventDefault(); dismiss(); }}
>
  {#if review && modalChange}
    <form method="dialog" onsubmit={(event) => event.preventDefault()}>
      {#if change.kind === "done" && change.receipt.kind === "applied_unverified"}
        <header>
          <h2 id="client-change-title">{review.planeLabel} no longer accepts this computer</h2>
          <p>
            {review.planeLabel} no longer accepts this computer, so the change almost certainly took effect. Jet
            can't read it back from here.
          </p>
        </header>
        <footer>
          <button class="secondary-button" type="button" onclick={() => pairing.forgetPlane()}>
            Forget {review.planeLabel}
          </button>
          <button bind:this={closeButton} class="primary-button" type="button" onclick={dismiss}>Close</button>
        </footer>
      {:else}
        <header>
          <h2 id="client-change-title">
            {review.change === "revoke" ? "Revoke" : "Disable"} {who} on {review.planeLabel}?
          </h2>
          {#if review.change === "revoke"}
            <p>
              It can no longer control {review.planeLabel}. Its key is deleted there, so it must pair again. This
              can't be undone.
            </p>
          {:else}
            <p>It stops controlling {review.planeLabel} until you enable it again. Its key is kept.</p>
          {/if}
          {#if review.viaThisPlaneConnection}
            <p class="client-change-warning">This computer will lose access to {review.planeLabel} immediately.</p>
          {/if}
        </header>
        <dl class="removal-facts">
          <div><dt>Plane</dt><dd>{review.planeLabel}{planePrefix ? ` · Plane ${planePrefix}` : ""}</dd></div>
          <div><dt>Computer</dt><dd><code>{fingerprint}</code>{review.isThisComputer ? " (this computer)" : ""}</dd></div>
        </dl>
        <footer>
          <button
            bind:this={cancelButton}
            class="secondary-button"
            type="button"
            aria-disabled={sending ? "true" : undefined}
            onclick={dismiss}
          >
            Cancel
          </button>
          <button
            class="danger-button"
            type="button"
            aria-disabled={sending ? "true" : undefined}
            onclick={execute}
          >
            {change.kind === "sending"
              ? review.change === "revoke" ? "Revoking…" : "Disabling…"
              : review.change === "revoke" ? "Revoke" : "Disable"}
          </button>
        </footer>
      {/if}
    </form>
  {/if}
</dialog>

{#if review && inlineOpen}
  <section class="client-change-inline" aria-labelledby="client-change-inline-title">
    <h4 id="client-change-inline-title" tabindex="-1" bind:this={inlineHeading}>
      {review.change === "enable" ? "Enabling" : "Disable"} {who} on {review.planeLabel}{review.change === "enable" ? "…" : "?"}
    </h4>
    {#if review.change === "disable"}
      <p>It stops controlling {review.planeLabel} until you enable it again. Its key is kept.</p>
      <div class="client-change-actions">
        <button class="danger-button" aria-disabled={sending ? "true" : undefined} onclick={execute}>
          {sending ? "Disabling…" : "Disable"}
        </button>
        <button class="secondary-button" aria-disabled={sending ? "true" : undefined} onclick={dismiss}>Cancel</button>
      </div>
    {/if}
  </section>
{/if}

{#if change.kind === "uncertain"}
  <section class="client-change-inline" role="alert" aria-labelledby="client-change-uncertain-title">
    <h4 id="client-change-uncertain-title">Change not confirmed</h4>
    <p>
      Jet couldn't confirm the change reached {change.review.planeLabel}.
      {planeErrorCopy(change.error, change.review.planeLabel)} <code>{change.error.code}</code>
    </p>
    <div class="client-change-actions">
      <button class="primary-button" onclick={() => pairing.executeChange()}>Retry the same request</button>
    </div>
  </section>
{/if}

{#if change.kind === "done" && change.receipt.kind === "applied_unverified" && !modalChange}
  <section class="client-change-inline" role="status">
    <p>
      {change.review.planeLabel} no longer accepts this computer, so the change almost certainly took effect. Jet
      can't read it back from here.
    </p>
    <div class="client-change-actions">
      <button class="secondary-button" onclick={() => pairing.forgetPlane()}>Forget {change.review.planeLabel}</button>
      <button class="secondary-button" onclick={dismiss}>Close</button>
    </div>
  </section>
{/if}

<style>
  .client-change-dialog p {
    margin: 0;
  }

  .client-change-warning {
    color: var(--warning);
    font-weight: 600;
  }

  .client-change-inline {
    display: grid;
    gap: 8px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--raised);
  }

  .client-change-inline h4,
  .client-change-inline p {
    margin: 0;
  }

  .client-change-dialog button[aria-disabled="true"],
  .client-change-inline button[aria-disabled="true"] {
    opacity: 0.55;
    cursor: default;
  }

  @media (forced-colors: active) {
    .client-change-dialog button[aria-disabled="true"],
    .client-change-inline button[aria-disabled="true"] {
      color: GrayText;
      border-color: GrayText;
      opacity: 1;
    }
  }

  .client-change-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
</style>
