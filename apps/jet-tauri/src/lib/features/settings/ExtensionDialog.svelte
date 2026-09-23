<script lang="ts">
  import type { ExtensionFacts } from "$lib/jet/extensions";
  import { actionLabel, entryActionLabel, entryStateText, offeredActions } from "./extensions-model";
  import type { ExtensionsSession } from "./extensions-session.svelte";
  import { refusalText } from "./model";
  import SettingsDialog from "./SettingsDialog.svelte";

  let {
    extensions,
    planeLabel,
    blocked,
  }: {
    extensions: ExtensionsSession;
    planeLabel: string;
    /** Changes are paused (read-only Recovery or a stale view). */
    blocked: boolean;
  } = $props();

  const inspection = $derived(extensions.inspection);
  const operation = $derived(extensions.operation);
  const review = $derived(
    (operation.kind === "confirm" || operation.kind === "applying" || operation.kind === "uncertain") &&
      inspection.kind === "ready"
      ? operation
      : null,
  );
  const preview = $derived(
    operation.kind === "confirm" && operation.review.subject.kind === "change_extension"
      ? operation.review.subject.preview
      : null,
  );
  const busy = $derived(operation.kind === "preparing" || operation.kind === "applying");
  /** Each step remounts the dialog so focus starts on that step's action. */
  const step = $derived(review ? "review" : "inspect");
  const destructive = $derived(review?.action === "remove");

  function close(): void {
    if (busy) return;
    extensions.closeInspection();
  }

  function fileCountText(facts: ExtensionFacts): string {
    return facts.fileCount === 1 ? "1 file" : `${facts.fileCount.toLocaleString("en-US")} files`;
  }
</script>

{#snippet facts(details: ExtensionFacts)}
  <div><dt>Source</dt><dd class="selectable">{details.source ?? "Not reported"}</dd></div>
  <div><dt>Publisher</dt><dd>{details.publisher ?? "Not reported"} <small>(Not verified by Jet)</small></dd></div>
  <div><dt>Version</dt><dd class="selectable">{details.version ?? "Not reported"}</dd></div>
  <div>
    <dt>Files</dt>
    <dd>
      {fileCountText(details)}
      {#if details.files.length > 0}
        <ul class="files">
          {#each details.files as file, index (index)}
            <li>
              <span class="selectable">{file.path}</span>
              <span class="mono selectable hash">{file.sha256}</span>
            </li>
          {/each}
        </ul>
      {/if}
      {#if details.files.length < details.fileCount}
        <p class="dialog-note">Not every file can be shown here.</p>
      {/if}
    </dd>
  </div>
  <div><dt>Access</dt><dd>Can run programs as you, with your user account's permissions.</dd></div>
{/snippet}

{#if inspection.kind !== "none"}
  {#key step}
    {#if review}
      {@const subject = review.subject}
      <SettingsDialog
        title={`${actionLabel(review.action)} ${subject.extensionId}?`}
        lead={`This changes ${subject.harness}'s own configuration on ${planeLabel}. Tasks that are running now are not affected. New tasks wait until the change finishes.`}
        focus={destructive || operation.kind === "uncertain" ? "cancel" : "primary"}
        oncancel={close}
      >
        <dl class="removal-facts">
          <div><dt>Plane</dt><dd>{planeLabel}</dd></div>
          <div><dt>Harness</dt><dd>{subject.harness}</dd></div>
          <div><dt>Extension</dt><dd class="selectable">{subject.extensionId}</dd></div>
          <div><dt>Change</dt><dd>{actionLabel(review.action)}</dd></div>
          {#if preview}
            {@render facts(preview)}
          {/if}
        </dl>
        {#if operation.kind === "uncertain"}
          <p class="dialog-alert" role="alert">
            Jet couldn't confirm this change. It may have been queued. <code>{operation.error.code}</code>
          </p>
        {/if}
        {#snippet footer()}
          <button type="button" class="secondary-button" data-dialog-cancel disabled={busy} onclick={close}>
            {operation.kind === "uncertain" ? "Close" : "Cancel"}
          </button>
          {#if operation.kind === "uncertain"}
            <button type="button" class="primary-button" data-dialog-primary onclick={() => void extensions.retry()}>
              Retry same change
            </button>
          {:else}
            <button type="button" class="secondary-button" disabled={busy} onclick={() => extensions.back()}>Back</button>
            <button
              type="button"
              class={destructive ? "danger-button" : "primary-button"}
              data-dialog-primary
              disabled={busy}
              onclick={() => void extensions.confirm()}
            >
              {operation.kind === "applying" ? "Saving…" : actionLabel(review.action)}
            </button>
          {/if}
        {/snippet}
      </SettingsDialog>
    {:else}
      {@const entry = inspection.entry}
      {@const actions = inspection.kind === "ready" ? offeredActions(entry, inspection.inspection) : []}
      {@const reviewable = inspection.kind === "ready" && inspection.inspection.reviewable}
      <SettingsDialog
        title={entry.id}
        lead={`${inspection.subject.harness} ${entry.kind === "plugin" ? "plugin" : "extension"} on ${planeLabel}`}
        focus="cancel"
        oncancel={close}
      >
        {#if inspection.kind === "loading"}
          <p class="dialog-note" role="status">Reading details from {planeLabel}…</p>
        {:else if inspection.kind === "failed"}
          <p class="dialog-alert" role="alert">{inspection.error.message} <code>{inspection.error.code}</code></p>
        {:else}
          <dl class="removal-facts">
            <div><dt>Harness</dt><dd>{inspection.subject.harness}</dd></div>
            <div><dt>Kind</dt><dd>{entry.kind === "plugin" ? "Plugin" : "Skill, hook or tool server"}</dd></div>
            {#if entryStateText(entry)}
              <div><dt>State</dt><dd>{entryStateText(entry)}</dd></div>
            {/if}
            {@render facts(inspection.inspection)}
          </dl>
          {#if !reviewable}
            <p class="dialog-note" role="status">
              Jet can't show what this change touches, so it can't be confirmed here.
            </p>
          {:else if blocked}
            <p class="dialog-note" role="status">Changes are paused on {planeLabel}.</p>
          {/if}
        {/if}
        {#if operation.kind === "refused"}
          <p class="dialog-alert" role="alert">{refusalText(operation.error)} <code>{operation.error.code}</code></p>
        {/if}
        {#snippet footer()}
          <button type="button" class="secondary-button" data-dialog-cancel disabled={busy} onclick={close}>Close</button>
          {#if reviewable}
            {#each actions as action (action)}
              <button
                type="button"
                class="secondary-button"
                disabled={busy || blocked}
                onclick={() => void extensions.prepare(action)}
              >
                {operation.kind === "preparing" && operation.action === action
                  ? "Checking…"
                  : `${entryActionLabel(entry, action)}…`}
              </button>
            {/each}
          {/if}
        {/snippet}
      </SettingsDialog>
    {/if}
  {/key}
{/if}

<style>
  .files {
    display: grid;
    gap: 4px;
    max-height: 180px;
    margin: 6px 0 0;
    padding: 0;
    overflow-y: auto;
    list-style: none;
  }

  .files li {
    display: grid;
    gap: 1px;
    font-size: 12px;
    overflow-wrap: anywhere;
  }

  .hash {
    color: var(--muted);
    font-size: 11px;
  }

  .mono {
    font-family: ui-monospace, "SFMono-Regular", Menlo, monospace;
  }

  .selectable {
    user-select: text;
  }

  small {
    color: var(--muted);
  }
</style>
