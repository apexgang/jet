<script lang="ts">
  import type { DesktopSession } from "./session.svelte";
  import { currentPlatform, shortcutAria, shortcutLabel } from "./shortcuts";
  import Mark from "$lib/features/workspace/Mark.svelte";
  let { session, inert = false }: { session: DesktopSession; inert?: boolean } = $props();
  const catalog = $derived(session.catalog);
  const planes = $derived(session.planes);
  const platform = currentPlatform();
  let searchInput = $state<HTMLInputElement>();
  $effect(() => {
    if (searchInput && session.takeFocusRequest("search")) searchInput.focus();
  });
</script>

<aside class="sidebar" class:hidden={!session.sidebarPresented} {inert} aria-label="Jet navigation">
  <div class="library-heading"><Mark small /><strong>Jet</strong><span>Workspace</span></div>
  <nav class="library" aria-label="Primary">
    <button class="new-conversation" aria-keyshortcuts={shortcutAria("new-task", platform)} onclick={() => session.select("new-task")}>
      <span>New task</span><kbd>{shortcutLabel("new-task", platform)}</kbd>
    </button>
    <button class="library-search" aria-keyshortcuts={shortcutAria("search", platform)} onclick={() => session.select("search")}>
      <svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="8" cy="8" r="5.5" /><path d="m12 12 5 5" /></svg>
      <span>Find a task</span><kbd>{shortcutLabel("search", platform)}</kbd>
    </button>
    {#if session.sidebarSelection === "search"}
      <form class="library-query" onsubmit={(event) => { event.preventDefault(); void catalog.search(); }}>
        <label class="visually-hidden" for="conversation-search">Search tasks</label>
        <input bind:this={searchInput} id="conversation-search" bind:value={catalog.searchText} maxlength="256" placeholder="Search your work" />
        <button type="submit" disabled={catalog.searching || !catalog.searchText.trim()}>{catalog.searching ? "Finding…" : "Find"}</button>
      </form>
      {#if catalog.searchState}
        <div class="library-results" aria-live="polite">
          {#each catalog.groups as group (group.planeId)}
            <section aria-label={`Results on ${group.label}`}>
              {#if group.showHeading}<h3>{group.label}</h3>{/if}
              {#if group.state.kind === "ready"}
                {#each group.state.result.hits as hit (`${hit.conversationId}-${hit.sequence}`)}
                  <button class="saved-task" onclick={() => session.openSearchHit(hit.conversationId, group.planeId)}>{hit.excerpt}</button>
                {/each}
              {/if}
              {#if group.text}<p class="library-note">{group.text}</p>{/if}
              {#if group.indexing}<p class="library-note">Still indexing on {group.label}</p>{/if}
            </section>
          {/each}
        </div>
      {/if}
    {/if}
    {#if session.attentionCount > 0}
      <button class="attention-link" onclick={() => session.select("attention")}>Needs attention <span>{session.attentionCount}</span></button>
    {/if}
    <div class="library-section-heading">Your tasks</div>
    <div class="saved-tasks">
      {#each catalog.visibleRows as conversation (`${conversation.planeId}:${conversation.id}`)}
        <button class="saved-task" class:active={session.isSelected(conversation.planeId, conversation.id) && session.sidebarSelection === "conversation"}
          aria-current={session.isSelected(conversation.planeId, conversation.id) && session.sidebarSelection === "conversation" ? "page" : undefined}
          onclick={() => session.openConversation(conversation.id, true, conversation.planeId)}>
          <span>{conversation.title}</span>
          {#if planes.multiple}<small>{conversation.planeLabel}</small>{/if}
        </button>
      {:else}
        <p class="library-note">Your tasks will appear here.</p>
      {/each}
      {#if catalog.hasMore}<button class="library-more" onclick={() => catalog.showMore()}>Show earlier tasks</button>{/if}
      {#each catalog.statusRows as row (row.planeId)}
        <div class="library-note" role="status">
          <p>{row.text}</p>
          {#if row.action === "retry"}<button onclick={() => session.retryPlane(row.planeId)}>Try again</button>
          {:else if row.action === "pair_again"}<button onclick={() => session.pairAgain(row.planeId)}>Pair again</button>
          {:else if row.action === "open_planes"}<button onclick={() => session.openPlanes({ planeId: row.planeId, focus: "detail" })}>View Plane</button>{/if}
        </div>
      {/each}
    </div>
  </nav>
  <div class="library-utilities">
    <div class="utility-links">
      <button class:active={session.sidebarSelection === "project"} onclick={() => session.select("project")}>Projects</button>
      <button class:active={session.sidebarSelection === "schedules"} onclick={() => session.select("schedules")}>Schedules</button>
      <button class:active={session.sidebarSelection === "trash"} onclick={() => session.select("trash")}>Trash</button>
    </div>
    <button class="library-connection" onclick={() => session.openPlanes()} title="Planes are computers running Jet">
      <span class="status-dot" class:online={session.connectionState === "online"} aria-hidden="true"></span>
      <span role="status">{planes.aggregate?.title ?? session.localPlaneLabel}<small>{planes.aggregate?.detail ?? session.connectionLabel}</small></span>
      <span aria-hidden="true">›</span>
    </button>
    <button class="library-settings" aria-label="Settings" aria-keyshortcuts={shortcutAria("settings", platform)} onclick={() => void session.openSettings()}>Settings <kbd>{shortcutLabel("settings", platform)}</kbd></button>
  </div>
</aside>
