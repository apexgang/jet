<script lang="ts">
  import type { DesktopSession } from "./session.svelte";
  import SetupPanel from "$lib/features/setup/SetupPanel.svelte";

  let { session }: { session: DesktopSession } = $props();
  let composer = $state<HTMLTextAreaElement>();

  const status = $derived.by(() => {
    if (session.conversationFreshness === "cached") return "Offline cache";
    if (session.conversationFreshness === "loading" || session.conversationBusy) return "Loading";
    const lifecycle = session.selectedRun?.lifecycle;
    if (lifecycle === "starting") return "Starting";
    if (lifecycle === "active") return "Working";
    if (lifecycle === "stopping") return "Stopping";
    if (lifecycle === "completed") return "Completed";
    if (lifecycle === "failed") return "Failed";
    if (lifecycle === "canceled") return "Canceled";
    if (lifecycle === "lost") return "Recovery needed";
    return "Ready";
  });

  $effect(() => {
    if (session.composerFocusRequest > 0) queueMicrotask(() => composer?.focus());
  });

  function handleComposerKey(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      void session.submitDraft();
    }
  }
</script>

{#if session.sidebarSelection === "project"}
  <SetupPanel {session} />
{:else}
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
        <h1>{session.selectedConversationTitle}</h1>
        <p>
          {session.selectedProjectName}
          <span aria-hidden="true">·</span>
          Runs on {session.scenario.plane.name}
        </p>
      </div>
      <span
        class:working={status === "Working"}
        class:warning={status === "Offline cache" || status === "Recovery needed"}
        class="run-status"
      >
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

    <div class="timeline" aria-live="polite" aria-busy={session.conversationBusy}>
      <div class="timeline-inner">
        {#if session.conversationFreshness === "cached"}
          <div class="notice">
            <strong>Showing cached state</strong>
            <p>Jet will refresh this Conversation after the local Plane reconnects.</p>
          </div>
        {/if}

        {#if session.timeline.length === 0}
          <div class="empty-state">
            <h2>{session.selectedConversationId ? "Live activity starts here" : "What should Jet do?"}</h2>
            <p>
              {session.selectedConversationId
                ? "The current Run state is restored above. New ordered activity will appear here."
                : "Describe the outcome. Jet will create an isolated Workspace in the selected Project."}
            </p>
          </div>
        {:else}
          {#each session.timeline as entry (entry.id)}
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
            maxlength="65536"
            rows="2"
            aria-label="Task message"
            placeholder="Describe what you want Jet to do"
            onkeydown={handleComposerKey}
          ></textarea>
          <button
            class="send-button"
            disabled={!session.canSubmitDraft || session.conversationBusy || session.connectionState !== "online"}
            onclick={() => session.submitDraft()}
          >
            {session.conversationBusy ? "Sending" : "Send"}
          </button>
        </div>
        <div class="context-row" aria-label="Task context">
          <span><small>Project</small>{session.selectedProjectName}</span>
          <span><small>Agent</small>{session.selectedHarnessName}</span>
          <span><small>Runs on</small>{session.scenario.plane.name}</span>
        </div>
      </div>
    </footer>
  </section>
{/if}
