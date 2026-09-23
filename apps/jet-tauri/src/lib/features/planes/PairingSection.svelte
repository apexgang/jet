<script lang="ts">
  import { onMount, untrack } from "svelte";

  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import type { Plane } from "$lib/jet/planes";
  import ClientChangeDialog from "./ClientChangeDialog.svelte";
  import {
    countdownText,
    formatClockTime,
    formatFingerprint,
    formatPairedDate,
    identityPrefix,
    millisecondsLeft,
    pairingEndedCopy,
    planeErrorCopy,
    spokenDigits,
  } from "./model";

  let { session, plane }: { session: DesktopSession; plane: Plane } = $props();

  const planes = $derived(session.planes);
  const pairing = $derived(session.planes.pairing);
  const label = $derived(plane.label);
  const section = $derived(pairing.planeId === plane.planeId ? pairing.state : { kind: "loading" as const });
  const view = $derived(pairing.planeId === plane.planeId ? pairing.view : null);
  const offer = $derived(pairing.offer);
  const confirm = $derived(pairing.confirm);
  const canMutate = $derived(pairing.canMutate);
  const blockedReason = $derived(pairing.blockedReason);
  const pending = $derived(view?.pending ?? null);
  const awaiting = $derived(
    confirm.kind === "entering" || confirm.kind === "sending" || confirm.kind === "mismatch",
  );
  const flowActive = $derived(
    offer.kind === "opening" ||
      offer.kind === "shown" ||
      offer.kind === "already_disclosed" ||
      offer.kind === "stopping" ||
      awaiting ||
      confirm.kind === "confirmed",
  );
  const clientPrefix = $derived(
    (view?.thisClientId ?? planes.snapshot?.identity.clientId ?? "").slice(0, 8) || null,
  );
  const planePrefix = $derived(identityPrefix(plane.planeIdentity));
  const pausedId = $derived(`pairing-paused-${plane.planeId}`);
  const describedBy = $derived(!canMutate && blockedReason ? pausedId : undefined);

  let now = $state(Date.now());
  let soonAnnouncement = $state("");
  let announcedFor: string | null = null;
  let confirmInput = $state<HTMLInputElement>();

  const deadline = $derived(
    offer.kind === "shown" ? offer.expiresAtUnixMs : awaiting && pending ? pending.expiresAtUnixMs : null,
  );

  $effect(() => {
    const planeId = plane.planeId;
    untrack(() => pairing.show(planeId));
  });

  onMount(() => () => {
    planes.cancelSwitch();
    pairing.leave();
  });

  // A coarse, text-only countdown: at most one update every 10 s.
  $effect(() => {
    if (!deadline) return;
    now = Date.now();
    const timer = setInterval(() => {
      now = Date.now();
      pairing.expire(now);
    }, 10_000);
    return () => clearInterval(timer);
  });

  $effect(() => {
    if (!deadline) return;
    const left = millisecondsLeft(deadline, now);
    untrack(() => {
      if (left > 0 && left <= 30_000 && announcedFor !== deadline) {
        announcedFor = deadline;
        soonAnnouncement = offer.kind === "shown" ? "The pairing code expires soon." : "The pairing request expires soon.";
      }
    });
  });

  $effect(() => {
    if (confirm.kind === "entering" && confirm.value === "") untrack(() => confirmInput?.focus());
  });

  function guarded(action: () => unknown): (event: MouseEvent) => void {
    return (event) => {
      if (!canMutate) {
        event.preventDefault();
        return;
      }
      void action();
    };
  }
</script>

<section class="plane-section pairing-section" aria-labelledby="plane-pairing-heading">
  <h3 id="plane-pairing-heading" tabindex="-1">Paired clients on {label}</h3>
  <p class="pairing-muted">
    {planePrefix ? `Plane ${planePrefix} · ` : ""}You are acting as this computer{clientPrefix ? ` (client ${clientPrefix})` : ""}
  </p>

  {#if planes.pendingSwitch}
    <div class="pairing-callout" role="alert">
      <p>Stop pairing on {label}? The code stops working.</p>
      <div class="pairing-actions">
        <button class="danger-button" disabled={offer.kind === "stopping"} onclick={() => planes.confirmSwitch()}>
          {offer.kind === "stopping" ? "Stopping…" : "Stop pairing and switch"}
        </button>
        <button class="secondary-button" onclick={() => planes.cancelSwitch()}>Keep pairing</button>
      </div>
    </div>
  {/if}

  {#if section.kind === "loading"}
    <p class="pairing-muted" aria-live="polite">Checking pairing on {label}…</p>
  {:else if section.kind === "unsupported"}
    <p class="pairing-muted">Pairing controls aren't available on {label}. Update Jet on {label}.</p>
  {:else if section.kind === "offline" || section.kind === "denied" || section.kind === "failed"}
    <div class="section-error" role="status">
      <p>{planeErrorCopy(section.error, label)} <code>{section.error.code}</code></p>
    </div>
    <div class="pairing-actions">
      <button class="secondary-button" onclick={() => pairing.load()}>Retry</button>
    </div>
  {:else if view}
    {#if section.kind === "stale"}
      <div class="section-error" role="status">
        <p>
          Last checked {formatClockTime(pairing.checkedAtUnixMs)}.
          {planeErrorCopy(section.error, label)} <code>{section.error.code}</code>
        </p>
        <button class="secondary-button" onclick={() => pairing.load()}>Retry</button>
      </div>
    {/if}

    {#if blockedReason}
      <p id={pausedId} class="pairing-paused" role="status">{blockedReason}</p>
    {/if}

    {#if pairing.notice}
      <p class="pairing-notice" role="status">{pairing.notice}</p>
    {/if}
    {#if pairing.actionError}
      <p class="section-error" role="alert">
        {planeErrorCopy(pairing.actionError, label)} <code>{pairing.actionError.code}</code>
      </p>
    {/if}
    {#if pairing.stopFailure}
      <div class="section-error" role="alert">
        <p>
          {planeErrorCopy(pairing.stopFailure.error, label)} The code stays valid until {formatClockTime(pairing.stopFailure.validUntilUnixMs)}.
          <code>{pairing.stopFailure.error.code}</code>
        </p>
        <button class="secondary-button" disabled={offer.kind === "stopping"} onclick={() => pairing.stop()}>Try again</button>
      </div>
    {/if}

    <p class="pairing-live" aria-live="polite">{soonAnnouncement}</p>

    {#if offer.kind === "opening"}
      <p class="pairing-muted" aria-live="polite">Opening pairing on {label}…</p>
    {:else if offer.kind === "stopping" && !planes.pendingSwitch}
      <p class="pairing-muted" aria-live="polite">Stopping pairing…</p>
    {:else if offer.kind === "shown"}
      <div class="pairing-offer">
        <div aria-live="polite">
          <p class="pairing-code">
            <span aria-hidden="true">{offer.code}</span>
            <span class="visually-hidden">Pairing code {spokenDigits(offer.code)}</span>
          </p>
        </div>
        <p>
          On the other computer, open Jet, add a Plane with this computer's SSH address, then type this code. It works
          once and expires in about 2 minutes. {offer.attemptsRemaining} wrong attempt{offer.attemptsRemaining === 1 ? "" : "s"} left.
        </p>
        <p class="pairing-muted" aria-live="off">{countdownText(offer.expiresAtUnixMs, now)}</p>
        <div class="pairing-actions">
          <button class="secondary-button" aria-describedby="stop-pairing-help" onclick={() => pairing.stop()}>
            Stop pairing
          </button>
        </div>
        <p id="stop-pairing-help" class="pairing-muted">This also stops any other pairing on {label}.</p>
      </div>
    {:else if offer.kind === "already_disclosed"}
      <div class="pairing-offer">
        <p>The code was created but didn't reach this window. For safety it can't be shown again.</p>
        <div class="pairing-actions">
          <button
            class="primary-button"
            aria-disabled={canMutate ? undefined : "true"}
            aria-describedby={describedBy}
            onclick={guarded(() => pairing.getNewCode())}
          >
            Get a new code
          </button>
          <button class="secondary-button" onclick={() => pairing.stop()}>Stop pairing</button>
        </div>
      </div>
    {/if}

    {#if awaiting && (confirm.kind === "entering" || confirm.kind === "sending" || confirm.kind === "mismatch")}
      <form
        class="pairing-offer"
        aria-labelledby="pairing-claim-heading"
        onsubmit={(event) => {
          event.preventDefault();
          if (canMutate) void pairing.submitConfirm();
        }}
      >
        <h4 id="pairing-claim-heading">A computer wants to pair</h4>
        <p>Type the 6-digit code shown on the other computer.</p>
        {#if pending}
          <p class="pairing-muted" aria-live="off">{countdownText(pending.expiresAtUnixMs, now)}</p>
        {/if}
        <label for="pairing-confirm-input">Code shown on the other computer</label>
        <input
          bind:this={confirmInput}
          id="pairing-confirm-input"
          class="pairing-input"
          inputmode="numeric"
          autocomplete="off"
          maxlength="7"
          value={confirm.kind === "entering" ? confirm.value : ""}
          disabled={confirm.kind === "sending"}
          aria-describedby={confirm.kind === "mismatch" ? "pairing-mismatch" : describedBy}
          oninput={(event) => pairing.setConfirmValue(event.currentTarget.value)}
        />
        {#if confirm.kind === "mismatch"}
          <p id="pairing-mismatch" class="section-error" role="alert">
            That code doesn't match. Check the other screen and try again. If they really differ, stop pairing: another
            computer may be answering.
          </p>
        {/if}
        <div class="pairing-actions">
          <button
            class="primary-button"
            type="submit"
            aria-disabled={canMutate && confirm.kind === "entering" && confirm.value.length === 7 ? undefined : "true"}
            aria-describedby={describedBy}
          >
            {confirm.kind === "sending" ? "Confirming…" : "Confirm"}
          </button>
          <button class="secondary-button" type="button" onclick={() => pairing.stop()}>Reject</button>
        </div>
      </form>
    {:else if confirm.kind === "confirmed"}
      <p class="pairing-notice" role="status">Confirmed. Waiting for the other computer to finish pairing…</p>
    {/if}

    {#if !flowActive}
      {#if offer.kind === "ended" && offer.reason !== "claimed"}
        <p class="pairing-muted" role="status">{pairingEndedCopy(offer.reason)}</p>
      {/if}
      {#if view.gate === "open"}
        <div class="pairing-callout">
          <p>{label} is accepting new pairings.</p>
          <div class="pairing-actions">
            <button class="secondary-button" onclick={() => pairing.stop()}>Close pairing</button>
          </div>
        </div>
      {:else if view.clients.length === 0}
        <p class="pairing-muted">No other computers can control {label}.</p>
      {/if}
      <div class="pairing-actions">
        <button
          class="primary-button"
          aria-disabled={canMutate ? undefined : "true"}
          aria-describedby={describedBy}
          onclick={guarded(() => pairing.pairComputer())}
        >
          Pair a computer
        </button>
      </div>
    {/if}

    <h4 id="plane-clients-heading" class="pairing-clients-heading" tabindex="-1">Paired computers</h4>
    {#if view.clients.length === 0}
      <p class="pairing-muted">No computers are paired with {label}.</p>
    {:else}
      <ul class="pairing-clients" aria-labelledby="plane-clients-heading">
        {#each view.clients as client (client.clientId)}
          <li>
            <div class="pairing-client-facts">
              <code class="pairing-fingerprint">{formatFingerprint(client.fingerprint)}</code>
              {#if client.isThisComputer}<span class="pairing-tag">This computer</span>{/if}
              <span class="pairing-muted">{formatPairedDate(client.pairedAtUnixMs)}</span>
              <span class:enabled={client.access === "enabled"} class="pairing-access">
                {client.access === "enabled" ? "Enabled" : "Disabled"}
              </span>
            </div>
            <div class="pairing-actions" role="group" aria-label={`Actions for ${formatFingerprint(client.fingerprint)}`}>
              <button
                class="secondary-button"
                aria-disabled={canMutate && !pairing.changeInFlight ? undefined : "true"}
                aria-describedby={describedBy}
                onclick={guarded(() =>
                  pairing.prepareChange(client.clientId, client.access === "enabled" ? "disable" : "enable"))}
              >
                {client.access === "enabled" ? "Disable" : "Enable"}
              </button>
              <button
                class="text-button danger"
                aria-disabled={canMutate && !pairing.changeInFlight ? undefined : "true"}
                aria-describedby={describedBy}
                onclick={guarded(() => pairing.prepareChange(client.clientId, "revoke"))}
              >
                Revoke…
              </button>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
    <ClientChangeDialog {pairing} />
  {/if}
</section>

<style>
  .pairing-section {
    display: grid;
    gap: 10px;
  }

  .pairing-section h3,
  .pairing-section h4 {
    margin: 0;
  }

  .pairing-section h3 {
    font-size: 14px;
  }

  .pairing-section h4 {
    font-size: 13px;
  }

  .pairing-section h3:focus-visible,
  .pairing-section h4:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }

  .pairing-section p {
    margin: 0;
  }

  .pairing-muted {
    color: var(--muted);
  }

  /* Present in the accessibility tree so later text is announced. */
  .pairing-live {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  .pairing-paused {
    color: var(--warning);
  }

  .pairing-notice {
    color: var(--success-text);
  }

  .pairing-callout,
  .pairing-offer {
    display: grid;
    gap: 8px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--raised);
  }

  .pairing-callout {
    border-color: var(--warning);
  }

  .pairing-code {
    font-family: ui-monospace, "SFMono-Regular", "DejaVu Sans Mono", monospace;
    font-size: 32px;
    font-weight: 650;
    letter-spacing: 0.08em;
    user-select: all;
  }

  .pairing-input {
    width: 10ch;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--text);
    font-family: ui-monospace, "SFMono-Regular", "DejaVu Sans Mono", monospace;
    font-size: 18px;
    letter-spacing: 0.08em;
  }

  .pairing-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .pairing-clients-heading {
    margin-top: 6px !important;
  }

  .pairing-clients {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .pairing-clients li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px;
    border: 1px solid var(--border-soft);
    border-radius: 8px;
  }

  .pairing-client-facts {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .pairing-fingerprint {
    user-select: all;
  }

  .pairing-tag {
    padding: 1px 6px;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--muted);
    font-size: 11px;
  }

  .pairing-access {
    padding: 1px 6px;
    border: 1px solid var(--warning);
    border-radius: 999px;
    color: var(--warning);
    font-size: 11px;
  }

  .pairing-access.enabled {
    border-color: var(--success-text);
    color: var(--success-text);
  }

  .pairing-section [aria-disabled="true"] {
    opacity: 0.55;
    cursor: default;
  }

  @media (forced-colors: active) {
    .pairing-section [aria-disabled="true"] {
      color: GrayText;
      border-color: GrayText;
      opacity: 1;
    }
  }
</style>
