<script lang="ts">
  import { paneTitle, settingsTargetForError } from "$lib/features/settings/model";
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import PlaneHealthNotice from "$lib/features/system/PlaneHealthNotice.svelte";
  import type { PublicError } from "$lib/jet/bridge";
  import type { PlaneId } from "$lib/jet/planes";
  import {
    CAPPED_TEXT,
    DUE_TEXT,
    TOMBSTONE_TEXT,
    emptyText,
    formatDate,
    reasonLabel,
    refusalLinkLabel,
    restoreActionText,
    sectionText,
    sectionView,
    trashStatus,
  } from "./model";

  let {
    session,
    planeId,
    planeLabel,
  }: { session: DesktopSession; planeId: PlaneId; planeLabel: string } = $props();

  const trash = $derived(session.trash);
  const section = $derived(trash.section(planeId));
  const view = $derived(sectionView(section));
  const message = $derived(sectionText(section, planeLabel));
  /** Restore needs a current list: not while offline or failed. */
  const current = $derived(section.kind === "ready" || section.kind === "empty");
  // Relative dates are computed when the list changes.
  const now = $derived.by(() => {
    void section;
    return Date.now();
  });
  const headingId = $derived(`trash-plane-${planeId}`);

  function link(error: PublicError) {
    const target = settingsTargetForError(error);
    return target ? { target, label: refusalLinkLabel(target.section, paneTitle(target.pane)) } : null;
  }
</script>

<section class="trash-section" aria-labelledby={headingId}>
  <header class="trash-section-heading">
    <h2 id={headingId}>{planeLabel}</h2>
    {#if section.kind !== "loading"}
      <button class="text-button" onclick={() => void trash.load(planeId)}>
        {section.kind === "ready" || section.kind === "empty" ? "Refresh" : "Try again"}
      </button>
    {/if}
  </header>

  <PlaneHealthNotice
    health={session.health}
    {planeId}
    {planeLabel}
    openSettings={(target) => void session.openSettings(target)}
  />

  {#if (section.kind === "ready" || section.kind === "empty") && section.freshness === "stale"}
    {@const loaded = formatDate(section.loadedAt, now)}
    <p class="trash-status" role="status">
      Showing Jet Trash from <time datetime={loaded.iso}>{loaded.absolute}</time>. Refresh to check for changes.
    </p>
  {/if}
  {#if message}
    <p
      class="trash-status"
      class:section-error={section.kind !== "loading"}
      role={section.kind === "loading" ? undefined : "status"}
    >
      {message}
      {#if section.kind === "failed" || section.kind === "denied" || section.kind === "unsupported"}
        <code>{section.error.code}</code>
      {/if}
    </p>
  {/if}

  {#if section.kind === "empty"}
    <p class="trash-status">{emptyText(section.view)}</p>
  {:else if view && view.entries.length > 0}
    {#if view.capped}
      <p class="trash-status">{CAPPED_TEXT}</p>
    {/if}
    <ul class="trash-list">
      {#each view.entries as entry (entry.conversationId)}
        {@const title = trash.title(planeId, entry.conversationId)}
        {@const status = trashStatus(entry, now)}
        {@const action = trash.rowAction(planeId, entry.conversationId)}
        {@const moved = formatDate(entry.trashedAtUnixMs, now)}
        {@const deleted = formatDate(entry.expiresAtUnixMs, now)}
        {@const rowId = `trash-row-${planeId}-${entry.conversationId}`}
        {@const actionText = restoreActionText(action, planeLabel)}
        <li>
          <article class="trash-row" aria-labelledby={rowId}>
            <div class="trash-row-text">
              <h3 id={rowId} class:unnamed={title === null}>{title ?? "Name unavailable"}</h3>
              <p>
                {reasonLabel(entry.reason)} · Moved <time datetime={moved.iso}>{moved.absolute}</time>
                · Deleted on <time datetime={deleted.iso}>{deleted.absolute}</time> ({deleted.relative})
              </p>
              {#if status === "due"}
                <p>{DUE_TEXT}</p>
              {:else if status === "tombstone"}
                <p id={`${rowId}-tombstone`}>{TOMBSTONE_TEXT}</p>
              {/if}
              {#if actionText}
                <p
                  class:section-error={action.kind === "refused" || action.kind === "uncertain"}
                  role={action.kind === "refused" || action.kind === "uncertain" ? "alert" : "status"}
                >
                  {actionText}
                  {#if action.kind === "refused"}
                    {@const refusal = link(action.error)}
                    {#if refusal}
                      <button class="text-button" onclick={() => void session.openSettings(refusal.target)}>
                        {refusal.label}
                      </button>
                    {/if}
                  {/if}
                </p>
              {/if}
            </div>
            {#if entry.restorable}
              <button
                class="secondary-button"
                disabled={!current || action.kind === "restoring" || action.kind === "restored"}
                onclick={() => void trash.restore(planeId, entry.conversationId)}
              >
                {action.kind === "uncertain" ? "Try again" : action.kind === "restoring" ? "Restoring…" : "Restore"}
              </button>
            {:else}
              <button class="secondary-button" disabled aria-describedby={`${rowId}-tombstone`}>Restore</button>
            {/if}
          </article>
        </li>
      {/each}
    </ul>
  {/if}
</section>
