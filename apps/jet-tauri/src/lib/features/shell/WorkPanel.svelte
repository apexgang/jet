<script lang="ts">
  import type { DesktopSession, WorkPanelTab } from "./session.svelte";

  let { session }: { session: DesktopSession } = $props();

  const tabs: Array<{ id: WorkPanelTab; label: string }> = [
    { id: "changes", label: "Changes" },
    { id: "files", label: "Files" },
    { id: "terminal", label: "Terminal" },
    { id: "run", label: "Run" },
  ];

  const emptyCopy: Record<Exclude<WorkPanelTab, "run">, { title: string; message: string }> = {
    changes: {
      title: "No change details yet",
      message: "Changes will load here when the Run exposes a diff.",
    },
    files: {
      title: "No file selected",
      message: "Choose a file from a Run to inspect it without leaving the task.",
    },
    terminal: {
      title: "No terminal open",
      message: "Workspace terminals will appear here when the Plane provides one.",
    },
  };
</script>

<aside class="work-panel" class:hidden={!session.workPanelPresented} aria-label="Work panel">
  <div class="panel-tabs" role="tablist" aria-label="Work panel views">
    {#each tabs as tab}
      <button
        role="tab"
        aria-selected={session.selectedWorkPanel === tab.id}
        class:active={session.selectedWorkPanel === tab.id}
        onclick={() => session.showPanel(tab.id)}
      >
        {tab.label}
      </button>
    {/each}
  </div>

  <div class="panel-content">
    {#if session.selectedWorkPanel === "run"}
      <section class="run-summary">
        <h2>Current Run</h2>
        <dl>
          <div><dt>Lifecycle</dt><dd>{session.selectedRun?.lifecycle ?? "Not started"}</dd></div>
          <div><dt>Activity</dt><dd>{session.hasLiveRun ? "Streaming" : "Idle"}</dd></div>
          <div><dt>Runs on</dt><dd>{session.scenario.plane.name}</dd></div>
          <div><dt>Cursor</dt><dd>{session.conversationDetail?.cursor ?? session.conversationCursor}</dd></div>
        </dl>
      </section>

      <section class="queue-summary">
        <h2>Queue</h2>
        <p>Queue controls arrive in Wave 2.1. Submitted Turns still follow Plane order.</p>
      </section>
    {:else}
      <div class="panel-empty">
        <h2>{emptyCopy[session.selectedWorkPanel].title}</h2>
        <p>{emptyCopy[session.selectedWorkPanel].message}</p>
      </div>
    {/if}
  </div>
</aside>
