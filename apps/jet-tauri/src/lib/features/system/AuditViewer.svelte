<script lang="ts">
  import { untrack } from "svelte";

  import SettingsDialog from "$lib/features/settings/SettingsDialog.svelte";
  import { landedTarget, sectionData, sectionHeadingId } from "$lib/features/settings/model";
  import type { SettingsSection } from "$lib/jet/settings-window";
  import type { AuditEntry, SecurityView } from "$lib/jet/system";
  import {
    EPOCH_UNCERTAIN_TEXT,
    actorLabel,
    auditIntro,
    decisionLabel,
    deniedText,
    degradedText,
    entryWhen,
    epochRefusalText,
    epochReviewText,
    exportFailureText,
    outcomeLabel,
    riskLabel,
    savedText,
    unsupportedText,
  } from "./audit-model";
  import { MAX_SHOWN_RECORDS } from "./audit.svelte";
  import { unixMs } from "./model";
  import type { SystemSession } from "./session.svelte";

  let {
    system,
    planeLabel,
    onopen,
  }: {
    system: SystemSession;
    planeLabel: string;
    /** Shows another Settings section in this window. */
    onopen: (section: SettingsSection) => void;
  } = $props();

  const audit = $derived(system.audit);

  // The first page loads once per Plane selection while this section is shown.
  $effect(() => {
    void system.selection;
    untrack(() => void system.audit.ensureLoaded());
  });

  const security = $derived<SecurityView | null>(sectionData(system.health)?.security ?? null);
  const degraded = $derived(security?.kind === "degraded" ? security : null);
  /** An earlier request is still unconfirmed: offer it for "Try again" whatever the audit shows. */
  const pendingEpoch = $derived(sectionData(system.health)?.pendingEpoch === true);
  const epochReady = $derived(pendingEpoch || degraded?.exported === true);
  const state = $derived(audit.state);
  const entries = $derived<AuditEntry[]>(
    state.kind === "loading" || state.kind === "ready" || state.kind === "offline" || state.kind === "failed"
      ? state.entries
      : [],
  );
  const exporting = $derived(audit.exporting);
  const dialog = $derived(audit.epoch);
  const sending = $derived(dialog.kind === "sending");
  const diagnostics = $derived(landedTarget("diagnostics") !== null);
  const returnFocus = [sectionHeadingId("audit")];

  function isoDate(value: string): string | undefined {
    const at = unixMs(value);
    return at === null ? undefined : new Date(at).toISOString();
  }

  function dialogTitle(): string {
    switch (dialog.kind) {
      case "review":
      case "sending":
      case "retry_epoch":
        return "Start a new audit period?";
      case "done":
        return "New audit period started";
      case "stale":
      case "restarted":
        return "Security audit changed";
      default:
        return "Jet didn't start a new audit period";
    }
  }
</script>

<div class="audit-viewer">
  <p>{auditIntro(planeLabel)}</p>

  {#if degraded}
    <p class="notice critical" role="status">{degradedText(planeLabel, degraded.breach)}</p>
  {/if}

  <div class="audit-actions">
    <button
      type="button"
      class="secondary-button"
      disabled={exporting.kind === "saving"}
      onclick={() => void audit.exportEvidence()}
    >
      {exporting.kind === "saving" ? "Saving…" : "Save audit evidence…"}
    </button>
    {#if degraded || pendingEpoch}
      <button
        type="button"
        class="danger-button"
        disabled={!epochReady || dialog.kind === "preparing" || sending}
        aria-describedby={epochReady ? undefined : "audit-epoch-hint"}
        onclick={() => void audit.prepareEpoch()}
      >
        {dialog.kind === "preparing" ? "Checking…" : "Start new audit period…"}
      </button>
    {/if}
  </div>
  {#if pendingEpoch}
    <p class="quiet" role="status">Jet is still confirming a request to start a new audit period. Open it to try again.</p>
  {:else if degraded && !degraded.exported}
    <p id="audit-epoch-hint" class="quiet">Save the evidence of this audit period first.</p>
  {/if}
  {#if exporting.kind === "saved"}
    <p role="status">{savedText(exporting.records, exporting.fileName)}</p>
  {:else if exporting.kind === "failed"}
    <p class="section-error" role="alert">
      {exportFailureText(exporting.error, planeLabel)} <code>{exporting.error.code}</code>
    </p>
  {/if}

  <label class="reveal">
    <input
      type="checkbox"
      checked={audit.reveal}
      disabled={state.kind === "loading"}
      onchange={(event) => void audit.setReveal(event.currentTarget.checked)}
    />
    Show identifiers
  </label>

  {#if state.kind === "denied"}
    <p class="notice" role="status">{deniedText(planeLabel)}</p>
  {:else if state.kind === "unsupported"}
    <p class="notice" role="status">{unsupportedText(planeLabel)}</p>
  {:else}
    {#if state.kind === "offline"}
      <p class="notice" role="status">
        Jet can't reach {planeLabel}.{entries.length > 0 ? " Showing the records already loaded." : ""}
      </p>
    {:else if state.kind === "failed"}
      <p class="section-error" role="alert">
        Jet couldn't read the security audit. <code>{state.error.code}</code>
      </p>
    {/if}

    {#if entries.length > 0}
      <div class="table-scroll">
        <table class="audit-table">
          <caption>Oldest first</caption>
          <thead>
            <tr>
              <th scope="col">When</th>
              <th scope="col">Decision</th>
              <th scope="col">By</th>
              <th scope="col">Risk</th>
              <th scope="col">Outcome</th>
              {#if audit.reveal}<th scope="col">Identifiers</th>{/if}
            </tr>
          </thead>
          <tbody>
            {#each entries as entry (entry.sequence)}
              <tr>
                <td><time datetime={isoDate(entry.recordedAtUnixMs)}>{entryWhen(entry)}</time></td>
                <td>{decisionLabel(entry.decision)}</td>
                <td>{actorLabel(entry.actor.kind)}</td>
                <td class:danger={entry.risk === "destructive"}>{riskLabel(entry.risk)}</td>
                <td>{outcomeLabel(entry.outcome)}</td>
                {#if audit.reveal}
                  <td>
                    <div class="identifiers">
                      {#if entry.actor.clientId}<span>Device <code>{entry.actor.clientId}</code></span>{/if}
                      {#if entry.target.identity}<span>{entry.target.kind} <code>{entry.target.identity}</code></span>{/if}
                      {#if entry.target.reference}<span>Reference <code>{entry.target.reference}</code></span>{/if}
                    </div>
                  </td>
                {/if}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else if state.kind === "ready"}
      <p class="quiet">No security decisions are recorded on {planeLabel} yet.</p>
    {/if}

    {#if state.kind === "loading"}
      <p class="quiet" role="status">Loading the security audit…</p>
    {:else if state.kind === "ready"}
      <div class="audit-actions">
        {#if audit.canLoadNewer}
          <button type="button" class="secondary-button" onclick={() => void audit.loadNewer()}>Load newer records</button>
        {/if}
        <button type="button" class="secondary-button" onclick={() => void audit.refresh()}>Refresh</button>
      </div>
      {#if !state.complete && entries.length >= MAX_SHOWN_RECORDS}
        <p class="quiet">This view holds the oldest {MAX_SHOWN_RECORDS.toLocaleString("en-US")} records. Save the evidence to review the rest.</p>
      {/if}
    {:else if state.kind === "offline" || state.kind === "failed"}
      <div class="audit-actions">
        <button type="button" class="secondary-button" onclick={() => void audit.refresh()}>Try again</button>
      </div>
    {/if}
  {/if}
</div>

{#if dialog.kind !== "closed" && dialog.kind !== "preparing"}
  <SettingsDialog
    title={dialogTitle()}
    lead={dialog.kind === "review" || dialog.kind === "sending" || dialog.kind === "retry_epoch"
      ? `On ${dialog.review.planeLabel}`
      : undefined}
    focus={dialog.kind === "review" || dialog.kind === "retry_epoch" ? "cancel" : "primary"}
    {returnFocus}
    focusKey={dialog.kind}
    oncancel={() => audit.closeEpoch()}
  >
    {#if dialog.kind === "review" || dialog.kind === "sending" || dialog.kind === "retry_epoch"}
      <p class="dialog-alert">{epochReviewText(dialog.review.planeLabel)}</p>
      <p class="dialog-note">
        The saved evidence covers audit period {dialog.review.degradedEpoch} through record
        {dialog.review.exportedThrough}.
      </p>
      {#if dialog.kind === "retry_epoch"}
        <p role="alert">{EPOCH_UNCERTAIN_TEXT} <code>{dialog.error.code}</code></p>
      {/if}
    {:else if dialog.kind === "done"}
      <p role="status">
        A new audit period started on {dialog.planeLabel}. Trust, policy, and deletion changes can go ahead again.
      </p>
    {:else if dialog.kind === "stale" || dialog.kind === "refused"}
      <p role="alert">{epochRefusalText(dialog.error, planeLabel)} <code>{dialog.error.code}</code></p>
    {:else if dialog.kind === "restarted"}
      <p role="alert">Jet on {planeLabel} started again since you opened this review. Review it again.</p>
    {/if}
    {#snippet footer()}
      {#if dialog.kind === "review" || dialog.kind === "sending" || dialog.kind === "retry_epoch"}
        <button type="button" class="secondary-button" data-dialog-cancel disabled={sending} onclick={() => audit.closeEpoch()}>
          Cancel
        </button>
        <button type="button" class="danger-button" data-dialog-primary disabled={sending} onclick={() => void audit.confirmEpoch()}>
          {sending ? "Starting…" : dialog.kind === "retry_epoch" ? "Try again" : "Start new audit period"}
        </button>
      {:else if dialog.kind === "stale" || dialog.kind === "restarted"}
        <button type="button" class="secondary-button" data-dialog-cancel onclick={() => audit.closeEpoch()}>Close</button>
        <button type="button" class="primary-button" data-dialog-primary onclick={() => void audit.reloadEpoch()}>
          Reload
        </button>
      {:else if dialog.kind === "refused" && dialog.error.code === "security.gap_unknown" && diagnostics}
        <button type="button" class="secondary-button" data-dialog-cancel onclick={() => audit.closeEpoch()}>Close</button>
        <button
          type="button"
          class="primary-button"
          data-dialog-primary
          onclick={() => {
            audit.closeEpoch();
            onopen("diagnostics");
          }}
        >
          Open Diagnostics
        </button>
      {:else}
        <button type="button" class="primary-button" data-dialog-primary onclick={() => audit.closeEpoch()}>Close</button>
      {/if}
    {/snippet}
  </SettingsDialog>
{/if}

<style>
  .audit-viewer {
    display: grid;
    gap: 10px;
  }

  .audit-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .reveal {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }

  .quiet {
    color: var(--muted);
  }

  .table-scroll {
    overflow-x: auto;
  }

  .audit-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  caption {
    padding-bottom: 6px;
    color: var(--muted);
    text-align: left;
  }

  th,
  td {
    padding: 6px 8px;
    border-top: 1px solid var(--border-soft);
    text-align: left;
    vertical-align: top;
  }

  thead th {
    border-top: 0;
    color: var(--muted);
    font-weight: 600;
  }

  td time {
    white-space: nowrap;
  }

  .danger {
    color: var(--danger);
  }

  .identifiers {
    display: grid;
    gap: 2px;
  }

  .identifiers code {
    word-break: break-all;
  }
</style>
