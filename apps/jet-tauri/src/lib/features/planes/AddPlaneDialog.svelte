<script lang="ts">
  import { tick, untrack } from "svelte";

  import { ADD_ANCHORS, CONNECTION_ANCHORS, restoreFocus } from "./focus";
  import { enrollmentFailureCopy, formatClockTime, normalizeManualCode, spokenDigits } from "./model";
  import type { PlanesSession } from "./session.svelte";

  let { planes }: { planes: PlanesSession } = $props();

  const enrollment = $derived(planes.enrollment);
  const wizard = $derived(enrollment.state);
  const repairing = $derived("repairOf" in wizard && wizard.repairOf !== null);
  const destination = $derived(
    wizard.step === "confirm" || wizard.step === "completing"
      ? wizard.enrollment.destination
      : wizard.step === "done"
        ? wizard.plane.label
        : "destination" in wizard
          ? wizard.destination
          : "",
  );
  const title = $derived(repairing ? `Pair again with ${destination}` : "Add a Plane");

  let dialog = $state<HTMLDialogElement>();
  let returnFocus: HTMLElement | null = null;
  /** Whether the last wizard was Pair again, for where focus returns. */
  let repairedPlane = false;
  $effect(() => {
    if (enrollment.open) repairedPlane = repairing;
  });
  let now = $state(Date.now());

  // One modal owner: open on the first step, close (and return focus) when
  // the wizard ends. Focus moves to the step's main control.
  $effect(() => {
    const open = enrollment.open;
    const step = wizard.step;
    untrack(() => {
      if (open && dialog && !dialog.open) {
        returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        void tick().then(() => {
          if (!dialog || dialog.open) return;
          dialog.showModal();
          focusStep();
        });
      } else if (open && dialog?.open) {
        void tick().then(focusStep);
      } else if (!open && dialog?.open) {
        dialog.close();
        // Pair again's button leaves once the Plane is online; Setup's
        // "Add a Plane" left when Planes opened.
        const anchors = repairedPlane ? CONNECTION_ANCHORS : ADD_ANCHORS;
        const target = returnFocus;
        returnFocus = null;
        void tick().then(() => restoreFocus(target, anchors));
      }
      void step;
    });
  });

  // Countdown text updates at most every 10 s and never ends the step.
  $effect(() => {
    if (wizard.step !== "confirm" && wizard.step !== "completing") return;
    now = Date.now();
    const timer = setInterval(() => (now = Date.now()), 10_000);
    return () => clearInterval(timer);
  });

  function focusStep(): void {
    const target = dialog?.querySelector<HTMLElement>("[data-autofocus]");
    target?.focus();
  }

  /** "Confirm on {destination} by {time}" only while the clocks plausibly agree. */
  function confirmBy(unixMs: string): string | null {
    const deadline = Number(unixMs);
    if (!Number.isFinite(deadline)) return null;
    const left = deadline - now;
    return left > 0 && left <= 5 * 60_000 ? formatClockTime(deadline) : null;
  }

  /** The label of a listed Plane an error names, for "same Plane as …". */
  function existingLabel(planeId: string): string | null {
    return planes.has(planeId) ? planes.label(planeId) : null;
  }

  function onCodeInput(event: Event): void {
    const input = event.currentTarget as HTMLInputElement;
    const grouped = normalizeManualCode(input.value);
    input.value = grouped;
    enrollment.setCode(grouped);
  }
</script>

<dialog
  bind:this={dialog}
  class="removal-dialog add-plane-dialog"
  aria-labelledby="add-plane-title"
  oncancel={(event) => { event.preventDefault(); enrollment.cancel(); }}
>
  {#if wizard.step !== "closed"}
    <form
      method="dialog"
      onsubmit={(event) => {
        event.preventDefault();
        if (wizard.step === "destination") void enrollment.submitDestination();
        else if (wizard.step === "code") void enrollment.submitCode();
        else if (wizard.step === "confirm") void enrollment.finish();
      }}
    >
      <header>
        <h2 id="add-plane-title">{title}</h2>
        {#if repairing && wizard.step !== "done"}
          <p>SSH address <code>{destination}</code></p>
        {/if}
      </header>

      {#if wizard.step === "destination"}
        <div class="add-plane-field">
          <label for="add-plane-destination">SSH address</label>
          <input
            id="add-plane-destination"
            data-autofocus
            autocomplete="off"
            spellcheck="false"
            placeholder="user@host or an SSH config alias"
            aria-describedby="add-plane-destination-help"
            aria-invalid={wizard.error ? "true" : undefined}
            value={wizard.value}
            oninput={(event) => enrollment.setDestination(event.currentTarget.value)}
          />
          <p id="add-plane-destination-help" class="add-plane-muted">
            Jet connects with your SSH settings. The computer's host key must already be trusted: connect once with
            <code>ssh &lt;address&gt;</code> in a terminal. Jet never accepts a new or changed host key.
          </p>
          {#if wizard.error}
            <p class="section-error" role="alert">
              {enrollmentFailureCopy(wizard.error, wizard.value || "that address", existingLabel)} <code>{wizard.error.code}</code>
            </p>
          {/if}
        </div>
        <footer>
          <button class="secondary-button" type="button" onclick={() => enrollment.cancel()}>Cancel</button>
          <button class="primary-button" type="submit" disabled={wizard.value.trim() === ""}>Continue</button>
        </footer>
      {:else if wizard.step === "checking"}
        <p class="add-plane-muted" aria-live="polite" data-autofocus tabindex="-1">
          Connecting to {wizard.destination} over SSH…
        </p>
        <footer>
          <button class="secondary-button" type="button" onclick={() => enrollment.cancel()}>Cancel</button>
        </footer>
      {:else if wizard.step === "secure_storage"}
        {#if wizard.error.code === "identity.secret_store_locked"}
          <p role="alert">Unlock your keyring, then choose Check again. <code>{wizard.error.code}</code></p>
          <footer>
            <button class="secondary-button" type="button" onclick={() => enrollment.cancel()}>Cancel</button>
            <button class="primary-button" type="button" data-autofocus onclick={() => enrollment.checkAgain()}>
              Check again
            </button>
          </footer>
        {:else}
          <div class="add-plane-copy">
            <p role="alert">
              Jet keeps this computer's pairing key in your system keyring, and it couldn't use one.
              <code>{wizard.error.code}</code>
            </p>
            <p class="add-plane-muted">
              Install and start a Secret Service keyring, such as GNOME Keyring (package <code>gnome-keyring</code>),
              KWallet with its Secret Service integration enabled, or KeePassXC with Secret Service integration turned
              on. Then sign out and back in, or start it, and choose Check again.
            </p>
          </div>
          <footer>
            <button class="secondary-button" type="button" onclick={() => enrollment.cancel()}>Cancel</button>
            <button
              class="secondary-button"
              type="button"
              aria-describedby="add-plane-session-help"
              onclick={() => enrollment.pairForSessionOnly()}
            >
              Pair for this session only
            </button>
            <button class="primary-button" type="button" data-autofocus onclick={() => enrollment.checkAgain()}>
              Check again
            </button>
          </footer>
          <p id="add-plane-session-help" class="add-plane-muted">
            The pairing ends when Jet quits. You'll need to pair again after restarting.
          </p>
        {/if}
      {:else if wizard.step === "code" || wizard.step === "claiming"}
        <div class="add-plane-field">
          <p>On {wizard.destination}, open Jet and start pairing a new computer. Type the 8-digit code it shows.</p>
          <label for="add-plane-code">Code shown on {wizard.destination}</label>
          <input
            id="add-plane-code"
            class="add-plane-code-input"
            data-autofocus={wizard.step === "code" ? true : undefined}
            inputmode="numeric"
            autocomplete="one-time-code"
            spellcheck="false"
            maxlength="9"
            aria-describedby="add-plane-code-help"
            aria-invalid={wizard.step === "code" && wizard.error ? "true" : undefined}
            disabled={wizard.step === "claiming"}
            value={wizard.step === "code" ? wizard.value : undefined}
            oninput={onCodeInput}
          />
          <p id="add-plane-code-help" class="add-plane-muted">
            The code works once and expires after about 2 minutes. If {wizard.destination} has no screen, start pairing
            from another computer that is already paired with it.
          </p>
          {#if wizard.step === "code" && wizard.error}
            <p class="section-error" role="alert">
              {enrollmentFailureCopy(wizard.error, wizard.destination, existingLabel)} <code>{wizard.error.code}</code>
            </p>
          {/if}
          {#if wizard.step === "claiming"}
            <!-- The input is disabled while checking, so focus waits here. -->
            <p class="add-plane-muted" aria-live="polite" data-autofocus tabindex="-1">
              Checking the code with {wizard.destination}…
            </p>
          {/if}
        </div>
        <footer>
          <button class="secondary-button" type="button" onclick={() => enrollment.cancel()}>Cancel</button>
          <button
            class="primary-button"
            type="submit"
            disabled={wizard.step === "claiming" || wizard.value.replace(/[^0-9]/g, "").length !== 8}
          >
            {wizard.step === "claiming" ? "Checking…" : "Continue"}
          </button>
        </footer>
      {:else if wizard.step === "confirm" || wizard.step === "completing"}
        {@const pending = wizard.enrollment}
        {@const deadline = confirmBy(pending.confirmByUnixMs)}
        <div class="add-plane-copy">
          <p class="add-plane-string">
            <span aria-hidden="true">{pending.authenticationString}</span>
            <span class="visually-hidden">Confirmation code {spokenDigits(pending.authenticationString)}</span>
          </p>
          <p>On {pending.destination}, type this code to confirm. Then choose Finish here.</p>
          <p class="add-plane-muted">Plane identity: <code>{pending.planeIdentity}</code></p>
          {#if deadline}
            <p class="add-plane-muted" aria-live="off">Confirm on {pending.destination} by {deadline}.</p>
          {/if}
          {#if pending.sessionOnly}
            <p class="add-plane-warning" role="note">This pairing ends when Jet quits. Set up secure storage to keep it.</p>
          {/if}
          {#if wizard.step === "confirm" && wizard.error}
            <p class="section-error" role="alert">
              {enrollmentFailureCopy(wizard.error, pending.destination, existingLabel)} <code>{wizard.error.code}</code>
            </p>
          {/if}
          {#if wizard.step === "completing"}
            <p class="add-plane-muted" aria-live="polite" data-autofocus tabindex="-1">
              Finishing pairing with {pending.destination}…
            </p>
          {/if}
        </div>
        <footer>
          <button class="secondary-button" type="button" onclick={() => enrollment.cancel()}>Cancel</button>
          <button
            class="primary-button"
            type="submit"
            data-autofocus={wizard.step === "confirm" ? true : undefined}
            disabled={wizard.step === "completing"}
          >
            {wizard.step === "completing" ? "Finishing…" : "Finish pairing"}
          </button>
        </footer>
      {:else if wizard.step === "done"}
        <p role="status">
          {wizard.repaired
            ? `${wizard.plane.label} is paired again and connected.`
            : `${wizard.plane.label} is paired and connected. Its tasks now appear in Recent.`}
        </p>
        {#if wizard.plane.credential === "session"}
          <p class="add-plane-warning" role="note">This pairing ends when Jet quits. Set up secure storage to keep it.</p>
        {/if}
        <footer>
          <button class="primary-button" type="button" data-autofocus onclick={() => enrollment.cancel()}>Close</button>
        </footer>
      {:else if wizard.step === "failed"}
        <p class="section-error" role="alert">
          {enrollmentFailureCopy(wizard.error, wizard.destination, existingLabel)} <code>{wizard.error.code}</code>
        </p>
        <footer>
          <button class="secondary-button" type="button" onclick={() => enrollment.cancel()}>Close</button>
          <button class="primary-button" type="button" data-autofocus onclick={() => enrollment.retry()}>
            Try again
          </button>
        </footer>
      {/if}
    </form>
  {/if}
</dialog>

<style>
  .add-plane-dialog p {
    margin: 0;
  }

  .add-plane-dialog footer {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
  }

  .add-plane-field,
  .add-plane-copy {
    display: grid;
    gap: 8px;
  }

  /* The shared removal-dialog layout pulls inputs up under their label. */
  .add-plane-dialog input:not([type="checkbox"]) {
    margin-top: 0;
  }

  .add-plane-muted {
    color: var(--muted);
    font-size: 12px;
  }

  .add-plane-muted:focus {
    outline: none;
  }

  .add-plane-code-input {
    font-family: ui-monospace, "SFMono-Regular", Menlo, monospace;
    font-size: 18px;
    letter-spacing: 0.12em;
  }

  .add-plane-string {
    font-family: ui-monospace, "SFMono-Regular", Menlo, monospace;
    font-size: 28px;
    font-weight: 650;
    letter-spacing: 0.12em;
    user-select: all;
  }

  .add-plane-warning {
    color: var(--warning);
    font-weight: 600;
  }
</style>
