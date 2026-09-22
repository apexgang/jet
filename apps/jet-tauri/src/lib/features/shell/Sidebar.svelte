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
      <button class:active={selected("attention")} onclick={() => session.select("attention")}>
        <span>Needs attention</span>
        <span class="count" aria-label="1 item">1</span>
      </button>
    </div>

    <div class="nav-group">
      <p class="nav-heading">Projects</p>
      <button class:active={selected("project")} onclick={() => session.select("project")}>
        {session.scenario.project?.name ?? "Choose a Project"}
      </button>
    </div>

    <div class="nav-group">
      <p class="nav-heading">Recent</p>
      <button
        class:active={selected("conversation")}
        class="conversation-link"
        onclick={() => session.select("conversation")}
      >
        {session.scenario.conversation?.title ?? "New task"}
      </button>
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
      <strong>{session.scenario.plane.name}</strong>
      <small>{session.connectionLabel}</small>
    </span>
  </div>
</aside>
