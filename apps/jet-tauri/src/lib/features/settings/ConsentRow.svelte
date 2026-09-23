<script lang="ts">
  import { tick } from "svelte";

  import {
    CONSENT_FOR,
    consentState,
    refusalText,
    sectionData,
    valueText,
    type BindingKey,
  } from "./model";
  import type { SettingsSession } from "./session.svelte";
  import SettingsReviewDialog from "./SettingsReviewDialog.svelte";

  let {
    session,
    bindingKey,
    missingText,
  }: {
    session: SettingsSession;
    /** The binding whose exact UUID this consent authorizes. */
    bindingKey: BindingKey;
    /** What happens without consent, for the bound account's label. */
    missingText: (label: string) => string;
  } = $props();

  const PLANE = { type: "plane" } as const;
  const id = $props.id();
  /** An undisplayable consent can't equal a binding: it reads as another account's. */
  const UNREADABLE = "\u0000";

  const consentKey = $derived(CONSENT_FOR[bindingKey]);
  const bindings = $derived(sectionData(session.work)?.bindings ?? []);
  const binding = $derived(session.setting(bindingKey, PLANE));
  const consent = $derived(session.setting(consentKey, PLANE));
  const bindingId = $derived(binding?.value.type === "text" ? binding.value.value : "");
  const consentId = $derived(
    consent === undefined ? "" : consent.value.type === "text" ? consent.value.value : UNREADABLE,
  );
  const status = $derived(consentState(bindingId, consentId));
  const label = $derived(bindings.find((option) => option.id === bindingId)?.label ?? "the chosen account");
  const row = $derived(session.row(consentKey, PLANE));
  const block = $derived(session.mutationBlock(PLANE));
  const inFlight = $derived(
    row.kind === "checking" || row.kind === "confirm" || row.kind === "applying" || row.kind === "uncertain",
  );

  let rowElement = $state<HTMLDivElement>();
  /** Focus returns to the checkbox once a change started here settles. */
  let returnFocus = $state(false);

  function started(): void {
    returnFocus = true;
  }

  $effect(() => {
    if (!returnFocus || inFlight) return;
    returnFocus = false;
    void tick().then(() => {
      const active = document.activeElement;
      if (active && active !== document.body && !rowElement?.contains(active)) return;
      const control = document.getElementById(`${id}-consent`);
      if (control instanceof HTMLElement && !control.matches(":disabled")) control.focus();
    });
  });

  function toggle(checked: boolean): void {
    started();
    // Consent names the stored binding exactly; unchecking clears it.
    void session.change(
      consentKey,
      PLANE,
      checked ? { kind: "set", value: { type: "text", value: bindingId } } : { kind: "clear" },
    );
  }
</script>

{#if consent === undefined && binding !== undefined && session.sectionFor(PLANE)?.kind === "ready"}
  <p class="consent-note">This setting needs a newer Jet service on {session.planeLabel}.</p>
{:else if status !== "none" || row.kind !== "idle"}
  <div class="consent-row" aria-busy={inFlight} bind:this={rowElement}>
    {#if status !== "none"}
      <label class="toggle" for={`${id}-consent`}>
        <input
          id={`${id}-consent`}
          type="checkbox"
          aria-describedby={`${id}-state`}
          checked={status === "granted"}
          disabled={block !== null || inFlight || consent === undefined}
          onchange={(event) => toggle(event.currentTarget.checked)}
        />
        <span>Allow Jet to send task content to {label}</span>
      </label>
      <p class="consent-note" id={`${id}-state`}>
        {#if status === "granted"}
          Only {label} may receive task content. Choosing another account doesn't carry this over.
        {:else if status === "missing"}
          {missingText(label)}
        {:else}
          Consent names a different account. Allow {label}, or choose that account again.
        {/if}
      </p>
    {/if}

    <div class="row-status" role="status">
      {#if row.kind === "checking"}
        <p>Checking the current value…</p>
      {:else if row.kind === "applying"}
        <p>Saving…</p>
      {:else if row.kind === "uncertain"}
        <div class="row-notice">
          <p>Jet couldn't confirm this change. It may have been saved. <code>{row.error.code}</code></p>
          <button
            class="text-button"
            aria-label="Retry same change: consent for {label}"
            disabled={block !== null}
            onclick={() => {
              started();
              void session.retry(consentKey, PLANE);
            }}
          >
            Retry same change
          </button>
          <button
            class="text-button"
            aria-label="Show current value: consent for {label}"
            onclick={() => void session.showCurrent(PLANE)}
          >
            Show current value
          </button>
        </div>
      {:else if row.kind === "changed_elsewhere"}
        <div class="row-notice">
          <p>
            This was changed on {session.planeLabel} to {valueText(consentKey, row.current.value, bindings)}. Your
            change wasn't saved.
          </p>
          <button
            class="text-button"
            aria-label="Use current: consent for {label}"
            onclick={() => {
              started();
              session.dismiss(consentKey, PLANE);
            }}
          >
            Use current
          </button>
        </div>
      {:else if row.kind === "refused"}
        <div class="row-notice critical">
          <p>{refusalText(row.error)} <code>{row.error.code}</code></p>
          <button
            class="text-button"
            aria-label="Dismiss: consent for {label}"
            onclick={() => {
              started();
              session.dismiss(consentKey, PLANE);
            }}
          >
            Dismiss
          </button>
        </div>
      {/if}
    </div>
  </div>
{/if}

{#if row.kind === "confirm"}
  <SettingsReviewDialog
    review={row.review}
    settingKey={consentKey}
    scopeLabel="This Plane"
    planeLabel={session.planeLabel}
    projects={[]}
    {bindings}
    {block}
    onconfirm={() => {
      started();
      void session.confirm(consentKey, PLANE);
    }}
    oncancel={() => {
      started();
      session.dismiss(consentKey, PLANE);
    }}
  />
{/if}

<style>
  .consent-row {
    display: grid;
    gap: 6px;
    padding: 0 0 12px 28px;
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text);
    font-weight: 560;
  }

  .toggle input {
    width: 18px;
    height: 18px;
    accent-color: var(--accent);
  }

  .consent-note {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }

  .row-status {
    font-size: 12px;
  }

  .row-status:empty {
    display: none;
  }

  .row-status p {
    margin: 0;
    color: var(--muted);
  }

  .row-notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--warning) 8%, var(--raised));
  }

  .row-notice p {
    flex: 1 1 240px;
    color: var(--text) !important;
  }

  .row-notice.critical {
    background: color-mix(in srgb, var(--danger) 10%, var(--raised));
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
