<script lang="ts">
  import { onMount, untrack } from "svelte";

  import type { CraftView } from "$lib/jet/agents";
  import type { ExtensionCatalogView } from "$lib/jet/extensions";
  import { craftTitle } from "./agents-model";
  import ExtensionDialog from "./ExtensionDialog.svelte";
  import {
    actionLabel,
    changeStateText,
    entryStateText,
    isTerminalChange,
    type ExtensionEntry,
  } from "./extensions-model";
  import { sectionData, withIssues } from "./model";
  import SectionState from "./SectionState.svelte";
  import type { SettingsSession } from "./session.svelte";

  let { session }: { session: SettingsSession } = $props();

  const extensions = $derived(session.extensions);
  const agentsView = $derived(sectionData(session.agents.view));
  const crafts = $derived<CraftView[]>(agentsView?.crafts ?? []);
  const craftIds = $derived(crafts.map((craft) => craft.craftId));
  const blocked = $derived(session.agentsBlock() !== null);
  const uncertain = $derived(
    extensions.operation.kind === "uncertain" && extensions.inspection.kind === "none" ? extensions.operation : null,
  );

  // Each Craft's catalog loads once it is listed on the shown Plane; later
  // reloads come from focus and Check again. Extensions have no events.
  $effect(() => {
    const ids = craftIds;
    void extensions.planeId;
    untrack(() => void extensions.ensureLoaded(ids));
  });

  onMount(() => {
    const onFocus = () => {
      if (extensions.idle && extensions.inspection.kind === "none") void extensions.reloadAll(craftIds);
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  });

  function standaloneRows(catalog: ExtensionCatalogView): ExtensionEntry[] {
    return catalog.standalone.map((entry) => ({ kind: "standalone", ...entry }));
  }

  function pluginRows(catalog: ExtensionCatalogView): ExtensionEntry[] {
    return catalog.plugins.map((entry) => ({ kind: "plugin", ...entry }));
  }

  function harnessOf(craft: CraftView): string {
    const catalog = extensions.catalogs[craft.craftId];
    const data = catalog ? sectionData(catalog) : null;
    return data?.harness ?? craftTitle(craft);
  }
</script>

<section class="settings-section" aria-labelledby="section-extensions">
  <h2 id="section-extensions" tabindex="-1">Extensions</h2>
  <p>Skills, hooks, tool servers and plugins each Harness loads from its own configuration on this Plane.</p>

  {#if uncertain}
    <div class="notice critical" role="status">
      <p>
        Jet couldn't confirm the change to {uncertain.subject.extensionId}. It may have been queued.
        <code>{uncertain.error.code}</code>
      </p>
      <button class="text-button" onclick={() => void extensions.retry()}>Retry same change</button>
    </div>
  {/if}

  <SectionState
    state={withIssues(session.agents.view, ["capabilities"])}
    title="Extensions"
    planeLabel={session.planeLabel}
    onretry={() => void session.agents.load()}
  >
    {#snippet children(_view)}
      {#if crafts.length === 0}
        <p>No Harness is installed on {session.planeLabel}.</p>
      {/if}
      {#each crafts as craft (craft.craftId)}
        {@const harness = harnessOf(craft)}
        {@const state = extensions.catalogs[craft.craftId] ?? { kind: "loading", last: null }}
        {@const changes = extensions.changesFor(craft.craftId)}
        <div class="harness" role="group" aria-labelledby={`extensions-${craft.craftId}`}>
          <div class="harness-heading">
            <h3 id={`extensions-${craft.craftId}`}>{harness}</h3>
            <button
              class="text-button"
              disabled={state.kind === "loading"}
              onclick={() => void extensions.loadCatalog(craft.craftId)}
            >
              Check again
            </button>
          </div>
          <SectionState
            state={withIssues(state, ["catalog"])}
            title={`${harness} extensions`}
            planeLabel={session.planeLabel}
            onretry={() => void extensions.loadCatalog(craft.craftId)}
          >
            {#snippet children(catalog: ExtensionCatalogView)}
              <h4>Extensions</h4>
              {#if catalog.standalone.length === 0}
                <p>No skills, hooks or tool servers are set up for {harness}.</p>
              {:else}
                <ul class="entries">
                  {#each standaloneRows(catalog) as entry (entry.entryToken)}
                    <li>
                      <span class="entry-id">{entry.id}</span>
                      <span class="entry-state" class:off={entry.kind === "standalone" && !entry.enabled}>
                        {entryStateText(entry)}
                      </span>
                      <button
                        class="text-button"
                        disabled={!extensions.idle}
                        aria-label={`Details for ${entry.id}`}
                        onclick={() => void extensions.inspect(craft.craftId, harness, entry)}
                      >
                        Details
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}

              <h4>Plugins</h4>
              {#if catalog.issues.some((issue) => issue.section === "plugins")}
                <p class="notice" role="status">
                  Jet couldn't get {harness}'s plugin list. <code>extensions.plugin_catalog_unavailable</code>
                </p>
              {:else if catalog.plugins.length === 0}
                <p>No plugins are available for {harness}.</p>
              {:else}
                <ul class="entries">
                  {#each pluginRows(catalog) as entry (entry.entryToken)}
                    <li>
                      <span class="entry-id">{entry.id}</span>
                      {#if entryStateText(entry)}<span class="entry-state">{entryStateText(entry)}</span>{/if}
                      <button
                        class="text-button"
                        disabled={!extensions.idle}
                        aria-label={`Details for ${entry.id}`}
                        onclick={() => void extensions.inspect(craft.craftId, harness, entry)}
                      >
                        Details
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}
              {#if catalog.truncated}
                <p class="note">Showing the first entries only.</p>
              {/if}
            {/snippet}
          </SectionState>

          {#if changes.length > 0}
            <ul class="changes" aria-label={`${harness} changes`}>
              {#each changes as change (change.changeId)}
                <li class:critical={change.state === "refused" || change.state === "outcome_unknown"}>
                  <span>
                    {actionLabel(change.action)}
                    {change.extensionId}: {changeStateText(change.state, change.harness)}
                    {#if change.state === "refused"}<code>extension.refused</code>{/if}
                    {#if change.error}<code>{change.error.code}</code>{/if}
                  </span>
                  {#if isTerminalChange(change.state)}
                    <button class="text-button" onclick={() => extensions.forget(change.changeId)}>Dismiss</button>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {/each}
      <p class="note">
        Adding an extension from a folder isn't available in this app yet. Changes apply to each Harness's own
        configuration; tasks that are running now are not affected.
      </p>
    {/snippet}
  </SectionState>
</section>

<ExtensionDialog {extensions} planeLabel={session.planeLabel} {blocked} />

<style>
  .harness {
    display: grid;
    gap: 8px;
    padding: 10px 0;
    border-top: 1px solid var(--border-soft);
  }

  .harness-heading {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }

  .harness-heading h3 {
    margin: 0 !important;
  }

  h4 {
    margin: 4px 0 0;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
  }

  .entries,
  .changes {
    display: grid;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .entries li,
  .changes li {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 12px;
    padding: 6px 0;
    font-size: 13px;
  }

  .entry-id {
    flex: 1 1 200px;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .entry-state {
    color: var(--muted);
    font-size: 12px;
  }

  .entry-state.off {
    color: var(--quiet);
  }

  .changes li {
    padding: 6px 10px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--warning) 8%, var(--raised));
  }

  .changes li span {
    flex: 1 1 240px;
  }

  .changes li.critical {
    background: color-mix(in srgb, var(--danger) 10%, var(--raised));
  }

  .notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: 13px;
  }

  .notice p {
    margin: 0;
    color: var(--text) !important;
  }

  .note {
    font-size: 12px;
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
