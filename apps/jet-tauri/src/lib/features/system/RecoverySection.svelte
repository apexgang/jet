<script lang="ts">
  import type { RecoveryReview, SystemHealth } from "$lib/jet/system";
  import SectionState from "$lib/features/settings/SectionState.svelte";
  import SettingsDialog from "$lib/features/settings/SettingsDialog.svelte";
  import { sectionHeadingId, withIssues } from "$lib/features/settings/model";
  import {
    EXPORT_BUNDLE_TEXT,
    bytesText,
    purgeBlock,
    purgeReviewText,
    purgedText,
    recoveryHeadline,
    recoveryRefusalText,
    restoreBlock,
    restoreReviewLines,
    snapshotReasonLabel,
    snapshotWhen,
    unconfirmedText,
    unixMs,
  } from "./model";
  import type { SystemSession } from "./session.svelte";

  let { system, planeLabel }: { system: SystemSession; planeLabel: string } = $props();

  // Recovery has no partial sections of its own: only the status read feeds it.
  const view = $derived(withIssues(system.health, []));
  const dialog = $derived(system.recovery);
  const preparing = $derived(dialog.kind === "preparing" ? dialog.action : null);
  /** The snapshot row whose restore is being checked. */
  const checkingSnapshot = $derived(dialog.kind === "preparing" ? dialog.snapshotId : null);
  const sending = $derived(dialog.kind === "sending");
  const shownReview = $derived<RecoveryReview | null>(
    dialog.kind === "review" || dialog.kind === "sending" || dialog.kind === "unconfirmed" ? dialog.review : null,
  );
  const returnFocus = [sectionHeadingId("recovery")];

  function isoDate(value: string): string | undefined {
    const at = unixMs(value);
    return at === null ? undefined : new Date(at).toISOString();
  }

  function dialogTitle(): string {
    switch (dialog.kind) {
      case "review":
      case "sending":
        return dialog.review.kind === "restore_snapshot"
          ? `Restore the snapshot from ${snapshotWhen(dialog.review.takenAtUnixMs)}?`
          : "Remove old snapshots?";
      case "done":
        return dialog.outcome.kind === "restored" ? "Snapshot restored" : "Old snapshots removed";
      case "unconfirmed":
        return dialog.review.kind === "restore_snapshot" ? "Restore not confirmed" : "Removal not confirmed";
      case "stale":
      case "restarted":
        return "Recovery changed";
      case "refused":
        return "Jet didn't make this change";
      default:
        return "Recovery";
    }
  }
</script>

<section class="settings-section" aria-labelledby={sectionHeadingId("recovery")}>
  <h2 id={sectionHeadingId("recovery")} tabindex="-1">Recovery</h2>
  <p>Verified copies of this Plane's Jet data that Jet can restore if its data is damaged.</p>
  <SectionState state={view} title="Recovery" {planeLabel} onretry={() => void system.load()}>
    {#snippet children(health: SystemHealth)}
      {@const recovery = health.recovery}
      <p class:warning={recovery.kind === "read_only"} role={recovery.kind === "read_only" ? "status" : undefined}>
        {recoveryHeadline(recovery, health.planeLabel)}
      </p>
      {#if recovery.kind !== "unsupported"}
        {@const restoring = recovery.kind === "read_only"}
        {@const blockedRestore = restoreBlock(recovery)}
        {#if recovery.ledger.kind === "corrupt"}
          <p class="notice critical" role="status">{blockedRestore ?? purgeBlock(recovery, health.security, health.planeLabel)}</p>
        {:else if blockedRestore}
          <p class="notice" role="status">{blockedRestore}</p>
        {/if}

        {#if recovery.snapshots.length > 0}
          <ul class="snapshots" aria-label="Recovery snapshots, newest first">
            {#each recovery.snapshots as snapshot (snapshot.snapshotId)}
              <li>
                <span class="snapshot-facts">
                  <time datetime={isoDate(snapshot.takenAtUnixMs)}>{snapshotWhen(snapshot.takenAtUnixMs)}</time>
                  <span class="quiet">{snapshotReasonLabel(snapshot.reason)} · {bytesText(snapshot.bytes)}</span>
                </span>
                {#if restoring}
                  {@const checking = checkingSnapshot === snapshot.snapshotId}
                  <button
                    class="secondary-button"
                    disabled={blockedRestore !== null || preparing !== null || sending}
                    aria-label={`${checking ? "Checking" : "Restore"} the snapshot from ${snapshotWhen(snapshot.takenAtUnixMs)}`}
                    aria-busy={checking}
                    onclick={() => void system.prepareRecovery({ kind: "restore_snapshot", snapshot_id: snapshot.snapshotId })}
                  >
                    {checking ? "Checking…" : "Restore…"}
                  </button>
                {/if}
              </li>
            {/each}
          </ul>
          {#if recovery.snapshotCount > recovery.snapshots.length}
            <p class="quiet">Showing the newest {recovery.snapshots.length} of {recovery.snapshotCount} snapshots.</p>
          {/if}
        {/if}

        {#if recovery.kind === "serving"}
          {@const blockedPurge = purgeBlock(recovery, health.security, health.planeLabel)}
          <div class="purge">
            <p>
              Deleted tasks can stay in older snapshots for up to about five weeks. Removing old snapshots ends that
              early.
            </p>
            {#if blockedPurge && recovery.ledger.kind !== "corrupt"}
              <p class="quiet" role="status">{blockedPurge}</p>
            {/if}
            <button
              class="danger-button"
              disabled={blockedPurge !== null || preparing !== null || sending}
              onclick={() => void system.prepareRecovery({ kind: "purge_snapshots" })}
            >
              {preparing === "purge_snapshots" ? "Checking…" : "Remove old snapshots…"}
            </button>
          </div>
        {/if}
      {/if}
      <p class="quiet">{EXPORT_BUNDLE_TEXT}</p>
    {/snippet}
  </SectionState>
</section>

{#if dialog.kind !== "closed" && dialog.kind !== "preparing"}
  <SettingsDialog
    title={dialogTitle()}
    lead={shownReview ? `On ${shownReview.planeLabel}` : undefined}
    focus={dialog.kind === "review" ? "cancel" : "primary"}
    {returnFocus}
    focusKey={dialog.kind}
    oncancel={() => system.closeRecovery()}
  >
    {#if (dialog.kind === "review" || dialog.kind === "sending") && dialog.review.kind === "restore_snapshot"}
      {@const review = dialog.review}
      <dl class="removal-facts">
        <dt>Snapshot</dt>
        <dd>{snapshotReasonLabel(review.reason)} · {bytesText(review.bytes)}</dd>
      </dl>
      <ul class="consequences">
        {#each restoreReviewLines(review) as line (line)}
          <li>{line}</li>
        {/each}
      </ul>
    {:else if (dialog.kind === "review" || dialog.kind === "sending") && dialog.review.kind === "purge_snapshots"}
      <p class="dialog-alert">{purgeReviewText(dialog.review)}</p>
      <p class="dialog-note">
        Jet's record of deleted data lists {dialog.review.deletionsRecorded} deletions. Your tasks and settings aren't
        changed.
      </p>
    {:else if dialog.kind === "done" && dialog.outcome.kind === "restored"}
      <p role="status">Restored. Jet reloaded everything shown for {dialog.planeLabel}.</p>
      <p class="dialog-note">
        The store now holds the snapshot from {snapshotWhen(dialog.outcome.takenAtUnixMs)}.
        {#if dialog.outcome.replacedName}The damaged data was kept as <code>{dialog.outcome.replacedName}</code>.{/if}
      </p>
    {:else if dialog.kind === "done" && dialog.outcome.kind === "purged"}
      <p role="status">{purgedText(dialog.outcome.removedCount)}</p>
    {:else if dialog.kind === "unconfirmed"}
      <p role="status">{unconfirmedText(dialog.review, dialog.check)}</p>
      <p class="dialog-note">Jet doesn't send it again on its own. <code>{dialog.error.code}</code></p>
    {:else if dialog.kind === "stale" || dialog.kind === "refused"}
      <p role="alert">
        {recoveryRefusalText(dialog.error, planeLabel)} <code>{dialog.error.code}</code>
      </p>
    {:else if dialog.kind === "restarted"}
      <p role="alert">Jet on {planeLabel} started again since you opened this review. Review it again.</p>
    {/if}
    {#snippet footer()}
      {#if dialog.kind === "review" || dialog.kind === "sending"}
        <button type="button" class="secondary-button" data-dialog-cancel disabled={sending} onclick={() => system.closeRecovery()}>
          Cancel
        </button>
        {#if dialog.review.kind === "restore_snapshot"}
          <button type="button" class="danger-button" data-dialog-primary disabled={sending} onclick={() => void system.confirmRecovery()}>
            {sending ? "Restoring…" : "Restore snapshot"}
          </button>
        {:else}
          <button type="button" class="danger-button" data-dialog-primary disabled={sending} onclick={() => void system.confirmRecovery()}>
            {sending ? "Removing…" : "Remove old snapshots"}
          </button>
        {/if}
      {:else if dialog.kind === "stale" || dialog.kind === "restarted"}
        <button type="button" class="secondary-button" data-dialog-cancel onclick={() => system.closeRecovery()}>Close</button>
        <button type="button" class="primary-button" data-dialog-primary onclick={() => void system.reloadRecovery()}>
          Reload
        </button>
      {:else}
        <button type="button" class="primary-button" data-dialog-primary onclick={() => system.closeRecovery()}>Close</button>
      {/if}
    {/snippet}
  </SettingsDialog>
{/if}

<style>
  .snapshots {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: 13px;
  }

  .snapshots li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 4px 12px;
    padding: 8px 0;
    border-bottom: 1px solid var(--border-soft);
  }

  .snapshot-facts {
    display: grid;
    gap: 2px;
  }

  .purge {
    display: grid;
    gap: 8px;
  }

  .purge button {
    justify-self: start;
  }

  .quiet {
    color: var(--muted);
  }

  .warning {
    padding: 10px 12px;
    /* Transparent until a forced palette paints it, where the tint is lost. */
    border: 1px solid transparent;
    border-radius: 8px;
    background: color-mix(in srgb, var(--warning) 12%, var(--raised));
    color: var(--text) !important;
  }

  .consequences {
    display: grid;
    gap: 6px;
    margin: 0;
    padding-left: 18px;
    font-size: 13px;
  }
</style>
