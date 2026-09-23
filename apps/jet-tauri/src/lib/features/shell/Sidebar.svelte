<script lang="ts">
  import type { DesktopSession, SidebarDestination } from "./session.svelte";
  import { currentPlatform, shortcutAria, shortcutLabel } from "./shortcuts";

  let { session }: { session: DesktopSession } = $props();
  const catalog = $derived(session.catalog);
  const planes = $derived(session.planes);
  const platform = currentPlatform();
  let searchInput = $state<HTMLInputElement>();

  // Ctrl+K: the Search field takes focus once it is rendered.
  $effect(() => {
    if (searchInput && session.takeFocusRequest("search")) searchInput.focus();
  });

  function selected(destination: SidebarDestination): boolean {
    return session.sidebarSelection === destination;
  }
</script>

<aside class="sidebar" class:hidden={!session.sidebarPresented} aria-label="Jet navigation">
  <div class="sidebar-brand">
    <span class="brand-mark" aria-hidden="true"></span>
    <strong>Jet</strong>
  </div>

  <nav aria-label="Primary">
    <div class="nav-group primary-actions">
      <button
        class:active={selected("new-task")}
        aria-keyshortcuts={shortcutAria("new-task", platform)}
        onclick={() => session.select("new-task")}
      >
        New task
        <kbd>{shortcutLabel("new-task", platform)}</kbd>
      </button>
      <button
        class:active={selected("search")}
        aria-keyshortcuts={shortcutAria("search", platform)}
        onclick={() => session.select("search")}
      >
        Search
        <kbd>{shortcutLabel("search", platform)}</kbd>
      </button>
      {#if selected("search")}
        <form class="sidebar-search" onsubmit={(event) => { event.preventDefault(); void catalog.search(); }}>
          <label for="conversation-search">Search tasks</label>
          <div>
            <input
              bind:this={searchInput}
              id="conversation-search"
              bind:value={catalog.searchText}
              maxlength="256"
              placeholder="Name, path, or branch"
            />
            <button type="submit" disabled={catalog.searching || !catalog.searchText.trim()}>
              {catalog.searching ? "Searching" : "Search"}
            </button>
          </div>
        </form>
        {#if catalog.searchState}
          <div class="search-results" aria-live="polite">
            {#each catalog.groups as group (group.planeId)}
              <section class="search-group" aria-label={`Results on ${group.label}`}>
                {#if group.showHeading}
                  <button
                    class="search-group-heading"
                    aria-pressed={catalog.searchFilter === group.planeId}
                    title={catalog.searchFilter === group.planeId ? "Show every Plane" : `Show only ${group.label}`}
                    onclick={() => catalog.toggleSearchFilter(group.planeId)}
                  >
                    {group.label}
                  </button>
                {/if}
                {#if group.state.kind === "ready"}
                  {#each group.state.result.hits as hit (`${hit.conversationId}-${hit.sequence}`)}
                    <button onclick={() => session.openSearchHit(hit.conversationId, group.planeId)}>
                      <span>{hit.excerpt}</span>
                      <small>{hit.field}</small>
                    </button>
                  {/each}
                {/if}
                {#if group.text}
                  <p>
                    {group.text}
                    {#if group.state.kind !== "ready" && group.state.kind !== "searching"}
                      <code>{group.state.error.code}</code>
                    {/if}
                  </p>
                {/if}
                {#if group.indexing}
                  <p>Still indexing on {group.label}</p>
                {/if}
              </section>
            {/each}
          </div>
        {/if}
      {/if}
      <button class:active={selected("attention")} onclick={() => session.select("attention")}>
        <span>Needs attention</span>
        {#if session.attentionCount > 0}
          <span class="attention-count" aria-label={`${session.attentionCount} items`}>{session.attentionCount}</span>
        {/if}
      </button>
    </div>

    <div class="nav-group">
      <p class="nav-heading">Projects</p>
      {#if session.setupSnapshot?.projects.length}
        {#each session.setupSnapshot.projects as project (project.id)}
          <button
            class:active={selected("project") && session.selectedProjectId === project.id}
            onclick={() => session.selectProject(project.id)}
          >
            {project.name}
          </button>
        {/each}
      {/if}
      <button
        class:active={selected("project")}
        aria-keyshortcuts={shortcutAria("add-project", platform)}
        onclick={() => session.select("project")}
      >
        {session.setupSnapshot?.projects.length ? "Manage Projects" : "Add a Project"}
      </button>
    </div>

    <div class="nav-group">
      <p class="nav-heading">Recent</p>
      {#each catalog.visibleRows as conversation (`${conversation.planeId}:${conversation.id}`)}
        <button
          class:active={selected("conversation") && session.isSelected(conversation.planeId, conversation.id)}
          class="conversation-link"
          aria-current={session.isSelected(conversation.planeId, conversation.id) ? "page" : undefined}
          aria-label={planes.multiple ? `${conversation.title}, on ${conversation.planeLabel}` : undefined}
          onclick={() => session.openConversation(conversation.id, true, conversation.planeId)}
        >
          <span class="conversation-link-title">{conversation.title}</span>
          {#if planes.multiple}
            <small aria-hidden="true">{conversation.planeLabel}</small>
          {/if}
        </button>
      {:else}
        {#if catalog.empty}
          <p class="empty-nav">No tasks yet</p>
        {:else if catalog.statusRows.length === 0}
          <p class="empty-nav">{planes.error ? "Tasks unavailable" : "Loading tasks…"}</p>
        {/if}
      {/each}
      {#if catalog.hasMore}
        <button class="load-more" onclick={() => catalog.showMore()}>Show more</button>
      {/if}
      {#each catalog.statusRows as row (row.planeId)}
        <div class="empty-nav recent-status" role={row.kind === "loading" ? undefined : "status"}>
          <p>
            {row.text}
            {#if row.error}<code>{row.error.code}</code>{/if}
          </p>
          {#if row.action === "retry"}
            <button class="text-button" onclick={() => session.retryPlane(row.planeId)}>Retry</button>
          {:else if row.action === "pair_again"}
            <button class="text-button" onclick={() => session.pairAgain(row.planeId)}>Pair again</button>
          {:else if row.action === "open_planes"}
            <button class="text-button" onclick={() => session.openPlanes({ planeId: row.planeId, focus: "detail" })}>
              Open Planes
            </button>
          {/if}
        </div>
      {/each}
    </div>

    <div class="nav-group secondary-actions">
      <button class:active={selected("schedules")} onclick={() => session.select("schedules")}>
        Schedules
      </button>
      <button class:active={selected("trash")} onclick={() => session.select("trash")}>
        Jet Trash
      </button>
      <button class:active={selected("planes")} onclick={() => session.openPlanes()}>
        <span>Planes</span>
        {#if planes.attentionCount > 0}
          <span
            class="attention-count"
            aria-label={`${planes.attentionCount} ${planes.attentionCount === 1 ? "Plane needs" : "Planes need"} attention`}
          >{planes.attentionCount}</span>
        {/if}
      </button>
      <button aria-keyshortcuts={shortcutAria("settings", platform)} onclick={() => void session.openSettings()}>
        Settings
        <kbd>{shortcutLabel("settings", platform)}</kbd>
      </button>
    </div>
  </nav>

  <div class="plane-status" aria-live="polite">
    {#if planes.aggregate}
      {@const aggregate = planes.aggregate}
      <span
        class:online={aggregate.tone === "ok"}
        class:failed={aggregate.tone === "danger"}
        class="status-dot"
        aria-hidden="true"
      ></span>
      <span>
        <strong>{aggregate.title}</strong>
        <small>{aggregate.detail}</small>
      </span>
    {:else}
      <span class:online={session.connectionState === "online"} class="status-dot" aria-hidden="true"></span>
      <span>
        <strong>{session.localPlaneLabel}</strong>
        <small>{session.connectionLabel}</small>
      </span>
    {/if}
  </div>
</aside>
