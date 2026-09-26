<script lang="ts">
  import { tick, untrack } from "svelte";
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import type { Schedule } from "$lib/jet/schedules";
  import SidebarToggle from "$lib/features/shell/SidebarToggle.svelte";
  let { session }: { session: DesktopSession; standalone?: boolean } = $props();
  const model = $derived(session.schedules);
  let dialog = $state<HTMLDialogElement>();
  let cancelButton = $state<HTMLButtonElement>();
  let review = $state<{ kind: "create" } | { kind: "cancel"; task: Schedule } | null>(null);
  let returnFocus: HTMLElement | null = null;
  $effect(() => {
    const plane = session.selectedPlaneId; const conversation = session.selectedConversationId;
    untrack(() => model.select(plane, conversation));
  });
  async function confirm(action: NonNullable<typeof review>, event: Event) {
    returnFocus = event.currentTarget instanceof HTMLFormElement ? event.currentTarget.querySelector<HTMLButtonElement>('button[type="submit"]') : event.currentTarget as HTMLElement;
    review = action; dialog?.showModal(); await tick(); cancelButton?.focus();
  }
  function close() { dialog?.close(); review = null; returnFocus?.focus(); }
  async function apply() {
    if (!review) return;
    const success = review.kind === "create" ? await model.create() : await model.cancel(review.task);
    if (success) close();
  }
  function due(task: Schedule) { return new Date(Number(task.nextDueAtUnixMs)).toLocaleString(); }
</script>
<section class="schedules-destination" aria-labelledby="schedules-title">
  <header class="setup-header">
    <SidebarToggle {session} />
    <div><h1 id="schedules-title" tabindex="-1">Schedules</h1><p>Ask Jet to return to a task at the same time each day.</p></div>
    <button class="secondary-button" onclick={() => model.refresh()} disabled={model.loading || !model.scope}>Refresh</button>
  </header>
  <div class="schedule-workspace">
    <label class="schedule-task" for="scheduled-task">Task
      <select id="scheduled-task" value={session.selectedConversationId ? `${session.selectedPlaneId}:${session.selectedConversationId}` : ""} disabled={model.busy} onchange={(event) => {
        const task = session.catalog.visibleRows.find((row) => `${row.planeId}:${row.id}` === event.currentTarget.value);
        if (task) void session.openConversation(task.id, false, task.planeId);
      }}>
        {#if !session.selectedConversationId}<option value="">Choose a saved task</option>{/if}
        {#each session.catalog.visibleRows as task (`${task.planeId}:${task.id}`)}<option value={`${task.planeId}:${task.id}`}>{task.title}{session.planes.multiple ? ` · ${task.planeLabel}` : ""}</option>{/each}
      </select>
    </label>
    {#if model.notice}<p role="status">{model.notice}</p>{/if}
    {#if model.error}<p role="alert" class="schedule-error">{model.error} <button onclick={() => model.refresh()}>Reload</button></p>{/if}
    {#if !model.scope}
      <div class="schedule-empty"><h2>Choose the work to come back to</h2><p>A schedule continues an existing task with instructions you provide.</p><button onclick={() => session.select("new-task")}>Start a new task</button></div>
    {:else}
      <div class="schedule-list" aria-busy={model.loading}>
        {#each model.tasks as task (task.id)}
          <article><div><h2>Every day at {task.localTime.slice(0, 5)}</h2><p>{task.prompt}</p><small>{task.timeZone} · Next: {due(task)}</small></div><button disabled={model.busy || !session.selectedPlaneOnline} onclick={(event) => confirm({ kind: "cancel", task }, event)}>Cancel…</button></article>
        {:else}<p class="schedule-empty">{model.loading ? "Loading schedules…" : "No daily schedules for this task."}</p>{/each}
      </div>
      <form class="schedule-form" onsubmit={(event) => { event.preventDefault(); void confirm({ kind: "create" }, event); }}>
        <h2>Add a daily schedule</h2>
        <div class="schedule-time"><label>Time<input type="time" bind:value={model.localTime} required disabled={model.busy} /></label><label>Time zone<input bind:value={model.timeZone} maxlength="128" required disabled={model.busy} spellcheck="false" /></label></div>
        <label>What should Jet do?<textarea bind:value={model.prompt} rows="4" maxlength="8192" required disabled={model.busy} placeholder="For example, review recent changes and summarize anything that needs attention."></textarea></label>
        <p class="schedule-help">Runs on {session.runsOnLabel}. The Plane keeps the timing and delivers scheduled instructions when it is available. Schedules stay with this task until you cancel them.</p>
        <button class="primary-button" type="submit" disabled={!model.valid || model.busy || !session.selectedPlaneOnline || model.loading}>Review schedule…</button>
      </form>
    {/if}
  </div>
</section>
<dialog bind:this={dialog} class="schedule-review" aria-labelledby="schedule-review-title" oncancel={(event) => { event.preventDefault(); if (!model.busy) close(); }}>
  {#if review}
    <h2 id="schedule-review-title">{review.kind === "create" ? "Create this daily schedule?" : "Cancel this schedule?"}</h2>
    <p>{review.kind === "create" ? `Every day at ${model.localTime} in ${model.timeZone}, Jet will send these instructions to ${session.selectedConversationTitle}:` : "Future firings and pending messages from this schedule will be canceled. Work already running continues."}</p>
    {#if review.kind === "create"}<blockquote>{model.prompt}</blockquote>{/if}
    {#if model.error}<p role="alert" class="schedule-error">{model.error}</p>{/if}
    <div class="schedule-actions"><button bind:this={cancelButton} disabled={model.busy} onclick={close}>{review.kind === "create" ? "Keep editing" : "Keep schedule"}</button><button class="primary-button" disabled={model.busy || !session.selectedPlaneOnline} onclick={apply}>{model.busy ? "Saving…" : review.kind === "create" ? "Create schedule" : "Cancel schedule"}</button></div>
  {/if}
</dialog>
<style>
  .schedules-destination { min-width: 0; min-height: 0; overflow: auto; }
  .schedule-workspace { max-width: 720px; margin: 0 auto; padding: 16px 32px 40px; }
  .schedule-task, .schedule-form label { display: grid; gap: 8px; font-weight: 500; }
  .schedule-task { grid-template-columns: auto minmax(0, 1fr); align-items: center; gap: 16px; margin-bottom: 24px; }
  .schedule-list article { display: flex; align-items: start; gap: 24px; padding: 20px 0; border-bottom: 1px solid var(--border); }
  .schedule-list article > div { flex: 1; min-width: 0; }
  h2 { margin: 0; font-size: 16px; font-weight: 600; }
  p { line-height: 1.6; }
  .schedule-list p { white-space: pre-wrap; overflow-wrap: anywhere; }
  small, .schedule-help, .schedule-empty { color: var(--muted); }
  .schedule-form { display: grid; gap: 16px; margin-top: 32px; }
  .schedule-form > button { justify-self: start; }
  .schedule-time { display: grid; grid-template-columns: 140px minmax(0, 1fr); gap: 16px; }
  textarea { resize: vertical; min-height: 80px; padding: 12px; }
  .schedule-help { margin: 0; font-size: 12px; }
  .schedule-review { width: min(480px, 85vw); padding: 24px; border: 1px solid var(--border); border-radius: 8px; background: var(--raised); color: var(--text); }
  blockquote { margin: 16px 0; padding: 12px; border-left: 2px solid var(--border-strong); white-space: pre-wrap; overflow-wrap: anywhere; max-height: 220px; overflow: auto; }
  .schedule-actions { display: flex; justify-content: end; gap: 8px; margin-top: 24px; }
  .schedule-error { color: var(--danger-text); }
</style>
