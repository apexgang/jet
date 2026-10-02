<script lang="ts">
  import { dismissMenu } from "$lib/features/workspace/dismiss-menu";
  import RenameTask from "$lib/features/workspace/RenameTask.svelte";
  import Composer from "$lib/features/workspace/Composer.svelte";
  import NewTask from "$lib/features/workspace/NewTask.svelte";
  import Message from "$lib/features/workspace/Message.svelte";
  import { landedTarget, paneTitle, settingsTargetForError } from "$lib/features/settings/model";
  import PlanesPanel from "$lib/features/planes/PlanesPanel.svelte";
  import SchedulesDestination from "$lib/features/schedules/SchedulesDestination.svelte";
  import type { DesktopSession } from "./session.svelte";
  import { currentPlatform, shortcutAria } from "./shortcuts";
  import SidebarToggle from "./SidebarToggle.svelte";
  import SetupPanel from "$lib/features/setup/SetupPanel.svelte";
  import PlaneHealthNotice from "$lib/features/system/PlaneHealthNotice.svelte";
  import { retentionLine } from "$lib/features/system/model";
  import MoveToTrashDialog from "$lib/features/trash/MoveToTrashDialog.svelte";
  import TrashView from "$lib/features/trash/TrashView.svelte";
  import { TOMBSTONE_TEXT, bannerText, refusalLinkLabel, restoreActionText } from "$lib/features/trash/model";

  let { session }: { session: DesktopSession } = $props();
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
  /** The task view is what the destinations below leave to the `{:else}` branch. */
  const taskView = $derived(
    session.sidebarSelection !== "project" &&
      session.sidebarSelection !== "planes" &&
      session.sidebarSelection !== "schedules" &&
      session.sidebarSelection !== "trash",
  );

  // AppShell renders the region outside what the compact overlay makes
  // inert, so status changes are still spoken while the overlay is open.
  $effect(() => {
    if (!taskView) session.taskStatus = "";
    else if (liveStatus !== null) session.taskStatus = liveStatus;
  });

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
  <SchedulesDestination standalone {session} />
{:else if session.sidebarSelection === "trash"}
  <TrashView {session} />
{:else}
  <section class="conversation" class:new-task-screen={!session.selectedConversationId} aria-label="Current task">
    <header class="conversation-header">
      <SidebarToggle {session} />
      <div class="conversation-title">
        <h1>{session.selectedConversationTitle}</h1>
        {#if session.selectedConversationId}<p>{session.selectedProjectName} · {session.runsOnLabel}</p>{/if}
      </div>
      {#if session.selectedConversationId}
        <span class="run-status" class:working={status === "Working"} class:warning={status.includes("needed") || status === "Reconnecting"}>{status}</span>
        <button class="toolbar-action" disabled={!session.selectedRun} onclick={() => session.showPanel("changes", "tab")}>Changes</button>
        <button class="toolbar-action work-panel-toggle" aria-expanded={session.workPanelPresented}
          aria-label={session.workPanelPresented ? "Hide work panel" : "Show work panel"}
          aria-keyshortcuts={shortcutAria("toggle-work-panel", platform)}
          onclick={(event) => session.toggleWorkPanel("toggle", event.currentTarget)}>Details</button>
        <details class="task-menu" use:dismissMenu>
          <summary aria-label="Task actions" title="Task actions">···</summary>
          <div class="task-menu-items">
            <RenameTask {session} />
            <button onclick={() => session.showPanel("terminal", "tab")}>Open Terminal</button>
            <button disabled={!session.canInterruptTurn} onclick={(event) => session.requestRunControl("interrupt_turn", event.currentTarget)}>Interrupt Turn…</button>
            <button disabled={!session.canStopRun} onclick={(event) => session.requestRunControl("stop_run", event.currentTarget)}>Stop Run…</button>
            <button disabled={!session.canMoveToTrash} onclick={(event) => { moveTrigger = event.currentTarget; void session.openMoveToTrash(); }}>Move to Trash…</button>
            {#if retention}<p>{retention}</p>{/if}
          </div>
        </details>
      {:else}<span class="window-location">{session.runsOnLabel}</span>{/if}
    </header>
    {#if !session.selectedConversationId}
      <NewTask {session} />
    {:else}
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
            <h2>{session.selectedConversationId ? "Waiting for the first update" : "What should Jet do?"}</h2>
            <p>
              {session.selectedConversationId
                ? "Your messages and results will appear here as work progresses."
                : "Describe the outcome. Jet will create an isolated Workspace in the selected Project."}
            </p>
          </div>
        {:else}
          {#each session.timeline as entry (entry.id)}
            {#if entry.kind === "user"}
              <article class="timeline-user"><span class="message-author">You</span><p>{entry.text}</p></article>
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
                    <span class="protocol-limit">This version of Jet cannot answer this request. Interrupt the Turn or stop the Run to continue safely.</span>
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
                <span class="message-author">Jet</span>
                <Message text={entry.text} />
              </article>
            {/if}
          {/each}
        {/if}
      </div>
    </div>

    <footer class="composer-region"><Composer {session} /></footer>
    {/if}
    <MoveToTrashDialog {session} returnFocus={moveTrigger} />
  </section>
{/if}
