<script lang="ts">
  import type { DesktopSession } from "./session.svelte";

  let { session }: { session: DesktopSession } = $props();
  let composer: HTMLTextAreaElement;

  const status = $derived.by(() => {
    switch (session.scenario.state) {
      case "first_launch":
        return "Connecting";
      case "ready":
        return "Ready";
      case "queued":
        return "Queued";
      case "completed":
        return "Completed";
      case "offline":
        return "Offline";
      case "stale_cursor":
        return "Refreshing";
      case "denied":
        return "Action denied";
      case "unsupported":
        return "Unsupported";
      case "recovery":
        return "Recovery needed";
    }

    const activity = session.scenario.run?.activity;
    if (activity === "working") return "Working";
    if (activity === "waiting_for_approval") return "Approval needed";
    if (activity === "reconnecting") return "Reconnecting";
    if (session.scenario.run?.lifecycle === "completed") return "Completed";
    return "Ready";
  });

  $effect(() => {
    if (session.composerFocusRequest > 0) {
      queueMicrotask(() => composer?.focus());
    }
  });

  function handleComposerKey(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      session.submitDraft();
    }
  }
</script>

<section class="conversation" aria-label="Current task">
  <header class="conversation-header">
    <button
      class="icon-button sidebar-toggle"
      aria-label={session.sidebarPresented ? "Hide sidebar" : "Show sidebar"}
      title={session.sidebarPresented ? "Hide sidebar" : "Show sidebar"}
      onclick={() => (session.sidebarPresented = !session.sidebarPresented)}
    >
      Sidebar
    </button>
    <div class="conversation-title">
      <h1>{session.scenario.conversation?.title ?? "New task"}</h1>
      <p>
        {session.scenario.project?.name ?? "Choose a Project"}
        <span aria-hidden="true">·</span>
        Runs on {session.scenario.plane.name}
      </p>
    </div>
    <span class:working={status === "Working"} class:warning={status.includes("needed")} class="run-status">
      {status}
    </span>
    <button
      class="icon-button"
      aria-label={session.workPanelPresented ? "Hide work panel" : "Show work panel"}
      title={session.workPanelPresented ? "Hide work panel" : "Show work panel"}
      onclick={() => (session.workPanelPresented = !session.workPanelPresented)}
    >
      Work panel
    </button>
  </header>

  <div class="timeline" aria-live="polite">
    <div class="timeline-inner">
      {#if session.scenario.notice}
        <div class:critical={session.scenario.notice.tone === "critical"} class="notice">
          <strong>{session.scenario.notice.title}</strong>
          <p>{session.scenario.notice.message}</p>
        </div>
      {/if}

      {#if session.scenario.timeline.length === 0}
        <div class="empty-state">
          <h2>What should Jet do?</h2>
          <p>Describe the outcome. You can choose where it runs before sending.</p>
        </div>
      {:else}
        <!-- ASVS 1.2.1: fixture and future agent text stays in escaped Svelte interpolation. -->
        {#each session.scenario.timeline as entry (entry.id)}
          {#if entry.kind === "user"}
            <div class="timeline-user"><p>{entry.text}</p></div>
          {:else if entry.kind === "activity"}
            <p class="timeline-activity">{entry.text}</p>
          {:else}
            <article class:result={entry.kind === "result"} class="timeline-agent">
              <p>{entry.text}</p>
            </article>
          {/if}
        {/each}
      {/if}
    </div>
  </div>

  <footer class="composer-region">
    <div class="composer-inner">
      {#if session.actionNotice}
        <p class="action-notice" role="status">{session.actionNotice}</p>
      {/if}
      <div class="composer-box">
        <textarea
          bind:this={composer}
          bind:value={session.draft}
          maxlength="20000"
          rows="2"
          aria-label="Task message"
          placeholder="Describe what you want Jet to do"
          onkeydown={handleComposerKey}
        ></textarea>
        <button class="send-button" disabled={!session.canSubmitDraft} onclick={() => session.submitDraft()}>
          Send
        </button>
      </div>
      <div class="context-row" aria-label="Task context">
        <span><small>Project</small>{session.scenario.project?.name ?? "Choose"}</span>
        <span><small>Agent</small>{session.scenario.capabilities.harnesses[0] ?? "Choose"}</span>
        <span><small>Runs on</small>{session.scenario.plane.name}</span>
      </div>
    </div>
  </footer>
</section>
