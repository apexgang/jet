<script lang="ts">
  import type { DesktopSession, SidebarDestination } from "./session.svelte";

  let { session }: { session: DesktopSession } = $props();

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
      <button class:active={selected("new-task")} onclick={() => session.select("new-task")}>
        New task
        <kbd>⌘N</kbd>
      </button>
      <button class:active={selected("search")} onclick={() => session.select("search")}>
        Search
        <kbd>⌘K</kbd>
      </button>
      {#if selected("search")}
        <form class="sidebar-search" onsubmit={(event) => { event.preventDefault(); void session.search(); }}>
          <label for="conversation-search">Search tasks</label>
          <div>
            <input
              id="conversation-search"
              bind:value={session.searchText}
              maxlength="256"
              placeholder="Name, path, or branch"
            />
            <button type="submit" disabled={session.searchBusy || !session.searchText.trim()}>
              {session.searchBusy ? "Searching" : "Search"}
            </button>
          </div>
        </form>
        {#if session.searchResult}
          <div class="search-results" aria-live="polite">
            {#each session.searchResult.hits as hit (`${hit.conversationId}-${hit.sequence}`)}
              <button onclick={() => session.openSearchHit(hit.conversationId, session.searchResult?.planeId)}>
                <span>{hit.excerpt}</span>
                <small>{hit.field}</small>
              </button>
            {:else}
              <p>No matching tasks</p>
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
      <button class:active={selected("project")} onclick={() => session.select("project")}>
        {session.setupSnapshot?.projects.length ? "Manage Projects" : "Add a Project"}
      </button>
    </div>

    <div class="nav-group">
      <p class="nav-heading">Recent</p>
      {#each session.conversations as conversation (`${conversation.planeId}:${conversation.id}`)}
        <button
          class:active={selected("conversation") && session.isSelected(conversation.planeId, conversation.id)}
          class="conversation-link"
          aria-current={session.isSelected(conversation.planeId, conversation.id) ? "page" : undefined}
          onclick={() => session.openConversation(conversation.id, true, conversation.planeId)}
        >
          {conversation.title}
        </button>
      {:else}
        <p class="empty-nav">
          {session.conversationFreshness === "live" ? "No tasks yet" : "Tasks unavailable"}
        </p>
      {/each}
      {#if session.nextConversationPage}
        <button class="load-more" disabled={session.conversationBusy} onclick={() => session.loadMoreConversations()}>
          {session.conversationBusy ? "Loading" : "Show more"}
        </button>
      {/if}
    </div>

    <div class="nav-group secondary-actions">
      <button class:active={selected("schedules")} onclick={() => session.select("schedules")}>
        Schedules
      </button>
      <button class:active={selected("planes")} onclick={() => session.select("planes")}>
        Planes
      </button>
      <button class:active={selected("settings")} onclick={() => session.select("settings")}>
        Settings
      </button>
    </div>
  </nav>

  <div class="plane-status" aria-live="polite">
    <span class:online={session.connectionState === "online"} class="status-dot"></span>
    <span>
      <strong>{session.localPlaneLabel}</strong>
      <small>{session.connectionLabel}</small>
    </span>
  </div>
</aside>
