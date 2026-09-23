<script lang="ts">
  import NotificationSettings from "$lib/features/notifications/NotificationSettings.svelte";
  import PlanesPanel from "$lib/features/planes/PlanesPanel.svelte";
  import type { DesktopSession } from "./session.svelte";
  import SetupPanel from "$lib/features/setup/SetupPanel.svelte";

  let { session }: { session: DesktopSession } = $props();
  let composer = $state<HTMLTextAreaElement>();

  const status = $derived.by(() => {
    if (session.conversationFreshness === "cached") return "Offline cache";
    if (session.connectionState !== "online") return "Reconnecting";
    if (session.conversationFreshness === "loading" || session.conversationBusy) return "Loading";
    if (session.supervision?.execution?.activity === "waiting_for_approval") return "Approval needed";
    if (session.supervision?.execution?.activity === "waiting_for_user") return "Waiting for you";
    if (session.supervision?.execution?.activity === "waiting_for_auth") return "Sign-in needed";
    if (session.supervision?.execution?.activity === "waiting_for_quota") return "Quota paused";
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

  function approvalStateLabel(state: string) {
    switch (state) {
      case "allowed": return "Action allowed";
      case "denied": return "Action denied";
      case "unavailable": return "Decision unavailable";
      default: return "Approval needed";
    }
  }
</script>

{#if session.sidebarSelection === "settings"}
  <NotificationSettings />
{:else if session.sidebarSelection === "project"}
  <SetupPanel {session} />
{:else if session.sidebarSelection === "planes"}
  <PlanesPanel {session} />
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
          Runs on {session.runsOnLabel}
        </p>
      </div>
      <span
        class:working={status === "Working"}
        class:warning={status === "Offline cache" || status === "Reconnecting" || status === "Recovery needed" || status.includes("needed")}
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
        {#if session.selectionUnavailable}
          <div class="notice" role="status">
            <strong>{session.selectedPlaneLabel} is unavailable</strong>
            <p>{session.selectedPlaneLabel} is unavailable. This task will load when it reconnects.</p>
            <button class="text-button" onclick={() => session.openPlanes({ planeId: session.selectedPlaneId, focus: "detail" })}>
              Open Planes
            </button>
          </div>
        {:else if session.conversationFreshness === "cached"}
          <div class="notice">
            <strong>Showing cached state</strong>
            <p>Jet will refresh this Conversation after {session.selectedPlaneLabel} reconnects.</p>
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
            {:else if entry.kind === "approval" && entry.approval}
              <article class="approval-card" aria-label={`Approval request for ${entry.approval.tool}`}>
                <div class="approval-card-heading">
                  <div class="approval-title-row">
                    <h2>{entry.approval.tool}</h2>
                    <span class="approval-state">{approvalStateLabel(entry.approval.state)}</span>
                  </div>
                  <span>{entry.approval.scope}</span>
                </div>
                <dl>
                  <div><dt>Target</dt><dd>{entry.approval.target}</dd></div>
                  <div><dt>Consequence</dt><dd>{entry.approval.consequence}</dd></div>
                </dl>
                <div class="approval-action">
                  <span>Requested action</span>
                  <pre>{entry.approval.action}</pre>
                </div>
                {#if entry.approval.rationale}
                  <p class="approval-rationale">{entry.approval.rationale}</p>
                {/if}
                <div class="approval-actions">
                  {#if entry.approval.canAuthorizeRetry}
                    <button
                      class="primary-action"
                      disabled={session.controlBusy !== null}
                      onclick={() => session.retryApproval(entry.approval!)}
                    >Authorize one retry</button>
                  {:else if entry.approval.state === "requested" || entry.approval.state === "unavailable"}
                    <span class="protocol-limit">Approve and Reject need the planned approval-decision protocol command.</span>
                  {/if}
                  <span class="approval-spacer"></span>
                  <button
                    disabled={!session.canInterruptTurn || session.controlBusy !== null}
                    onclick={() => session.requestRunControl("interrupt_turn")}
                  >Interrupt Turn…</button>
                  <button
                    class="danger-action"
                    disabled={!session.canStopRun || session.controlBusy !== null}
                    onclick={() => session.requestRunControl("stop_run")}
                  >Stop Run…</button>
                </div>
              </article>
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
        {#if session.queueIsFull}
          <p class="composer-warning" role="alert">The Turn queue is full. Withdraw a queued Turn or wait for one to finish.</p>
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
          <span><small>Runs on</small>{session.runsOnLabel}</span>
          <span class:over-limit={session.draftBytes > session.maximumPromptBytes} class="draft-limit">
            <small>Message</small>{session.draftBytes.toLocaleString()} / {session.maximumPromptBytes.toLocaleString()} bytes
          </span>
        </div>
      </div>
    </footer>
  </section>
{/if}
