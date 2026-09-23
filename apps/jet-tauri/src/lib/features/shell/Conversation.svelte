<script lang="ts">
  import { landedTarget, paneTitle, settingsTargetForError } from "$lib/features/settings/model";
  import PlanesPanel from "$lib/features/planes/PlanesPanel.svelte";
  import SchedulesDestination from "$lib/features/schedules/SchedulesDestination.svelte";
  import type { DesktopSession } from "./session.svelte";
  import { currentPlatform, shortcutAria } from "./shortcuts";
  import SetupPanel from "$lib/features/setup/SetupPanel.svelte";
  import PlaneHealthNotice from "$lib/features/system/PlaneHealthNotice.svelte";
  import { retentionLine } from "$lib/features/system/model";
  import MoveToTrashDialog from "$lib/features/trash/MoveToTrashDialog.svelte";
  import TrashView from "$lib/features/trash/TrashView.svelte";
  import { TOMBSTONE_TEXT, bannerText, refusalLinkLabel, restoreActionText } from "$lib/features/trash/model";

  let { session }: { session: DesktopSession } = $props();
  let composer = $state<HTMLTextAreaElement>();
  let moveTrigger = $state<HTMLElement | null>(null);
  const platform = currentPlatform();

  /** The selected task's own Jet Trash state; never inferred from the list. */
  const trashBanner = $derived(
    session.trash.bannerFor(session.selectedPlaneId, session.selectedConversationId),
  );

  const retention = $derived(
    session.selectedConversationId &&
      session.conversationDetail?.conversation.id === session.selectedConversationId
      ? retentionLine(session.conversationDetail.retention)
      : null,
  );

  const status = $derived.by(() => {
    if (session.conversationFreshness === "cached") return "Offline cache";
    if (!session.selectedPlaneOnline) return "Reconnecting";
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

  /** The approval the task is waiting on, if any: the newest requested one. */
  const pendingApproval = $derived(
    session.timeline.findLast((entry) => entry.approval?.state === "requested")?.approval ?? null,
  );

  /**
   * What the status region announces. The timeline itself is not live, so
   * streamed output never floods a screen reader; only status changes are
   * spoken. "Loading" is a transient refresh and is not announced.
   */
  const liveStatus = $derived.by(() => {
    if (status === "Loading") return null;
    if (status === "Approval needed" && pendingApproval) return `Approval needed: ${pendingApproval.tool}`;
    if (status === "Completed") return "Run completed";
    return `Task status: ${status}`;
  });
  let announcedStatus = $state("");

  $effect(() => {
    if (liveStatus !== null) announcedStatus = liveStatus;
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

{#if session.sidebarSelection === "project"}
  <SetupPanel {session} />
{:else if session.sidebarSelection === "planes"}
  <PlanesPanel {session} />
{:else if session.sidebarSelection === "schedules"}
  <SchedulesDestination standalone />
{:else if session.sidebarSelection === "trash"}
  <TrashView {session} />
{:else}
  <section class="conversation" aria-label="Current task">
    <header class="conversation-header">
      <button
        class="icon-button sidebar-toggle"
        aria-label={session.sidebarPresented ? "Hide sidebar" : "Show sidebar"}
        aria-keyshortcuts={shortcutAria("toggle-sidebar", platform)}
        title={session.sidebarPresented ? "Hide sidebar" : "Show sidebar"}
        onclick={() => session.toggleSidebar()}
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
        {#if retention}
          <p class="retention-line" title={retention}>{retention}</p>
        {/if}
      </div>
      <span
        class:working={status === "Working"}
        class:warning={status === "Offline cache" || status === "Reconnecting" || status === "Recovery needed" || status.includes("needed")}
        class="run-status"
      >
        {status}
      </span>
      {#if status === "Sign-in needed" || status === "Quota paused"}
        {@const target = landedTarget("accounts", session.selectedPlaneId)}
        {#if target}
          <button class="text-button" onclick={() => void session.openSettings(target)}>Open Agents settings</button>
        {/if}
      {/if}
      <button
        class="icon-button"
        disabled={!session.canMoveToTrash}
        title={session.selectedConversationId && !session.selectedPlaneOnline
          ? `Reconnect to ${session.selectedPlaneLabel} to move this task to Jet Trash`
          : undefined}
        onclick={(event) => {
          moveTrigger = event.currentTarget;
          void session.openMoveToTrash();
        }}
      >
        Move to Trash…
      </button>
      <button
        class="icon-button"
        aria-label={session.workPanelPresented ? "Hide work panel" : "Show work panel"}
        aria-keyshortcuts={shortcutAria("toggle-work-panel", platform)}
        title={session.workPanelPresented ? "Hide work panel" : "Show work panel"}
        onclick={() => (session.workPanelPresented = !session.workPanelPresented)}
      >
        Work panel
      </button>
    </header>

    <p class="visually-hidden" role="status">{announcedStatus}</p>

    <div class="timeline" aria-busy={session.conversationBusy}>
      <div class="timeline-inner">
        <PlaneHealthNotice
          health={session.health}
          planeId={session.selectedPlaneId}
          planeLabel={session.selectedPlaneLabel}
          openSettings={(target) => void session.openSettings(target)}
        />
        {#if trashBanner.kind === "trashed" && session.selectedConversationId}
          {@const conversationId = session.selectedConversationId}
          {@const action = session.trash.rowAction(session.selectedPlaneId, conversationId)}
          {@const actionText = restoreActionText(action, session.selectedPlaneLabel)}
          <section class="notice trash-banner" aria-label="In Jet Trash">
            <p role="status">{bannerText(trashBanner.entry, Date.now())}</p>
            {#if !trashBanner.entry.restorable}
              <p>{TOMBSTONE_TEXT}</p>
            {/if}
            {#if actionText}
              <p role={action.kind === "refused" || action.kind === "uncertain" ? "alert" : "status"}>{actionText}</p>
            {/if}
            <div class="trash-banner-actions">
              {#if trashBanner.entry.restorable}
                <button
                  class="text-button"
                  disabled={!session.selectedPlaneOnline || action.kind === "restoring" || action.kind === "restored"}
                  onclick={() => void session.trash.restore(session.selectedPlaneId, conversationId)}
                >{action.kind === "uncertain" ? "Try again" : "Restore"}</button>
              {/if}
              {#if action.kind === "refused"}
                {@const target = settingsTargetForError(action.error)}
                {#if target}
                  <button class="text-button" onclick={() => void session.openSettings(target)}>
                    {refusalLinkLabel(target.section, paneTitle(target.pane))}
                  </button>
                {/if}
              {/if}
              <button class="text-button" onclick={() => session.select("trash")}>Open Jet Trash</button>
            </div>
          </section>
        {/if}
        {#if session.selectionUnavailable}
          <div class="notice" role="status">
            <p>{session.selectedPlaneLabel} is unavailable. This task will load when it reconnects.</p>
            {#if session.pairAgainTarget(session.selectedPlaneError)}
              <button class="text-button" onclick={() => session.pairAgain(session.selectedPlaneId)}>Pair again</button>
            {/if}
            <button class="text-button" onclick={() => session.openPlanes({ planeId: session.selectedPlaneId, focus: "detail" })}>
              Open Planes
            </button>
            {#if session.selectedPlaneError}
              {@const target = settingsTargetForError(session.selectedPlaneError)}
              {#if target}
                <button class="text-button" onclick={() => void session.openSettings(target)}>
                  Open {paneTitle(target.pane)} settings
                </button>
              {/if}
            {/if}
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
                    onclick={(event) => session.requestRunControl("interrupt_turn", event.currentTarget)}
                  >Interrupt Turn…</button>
                  <button
                    class="danger-action"
                    disabled={!session.canStopRun || session.controlBusy !== null}
                    onclick={(event) => session.requestRunControl("stop_run", event.currentTarget)}
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
            disabled={!session.canSubmitDraft || session.conversationBusy || !session.selectedPlaneOnline}
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
    <MoveToTrashDialog {session} returnFocus={moveTrigger} />
  </section>
{/if}

