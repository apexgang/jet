<script lang="ts">
  import { untrack } from "svelte";
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import { operationLabel, deliverySummary, statusLabel } from "./model";
  import type { DeliveryOperation } from "$lib/jet/delivery";
  let { session }: { session: DesktopSession } = $props();
  const delivery = $derived(session.delivery);
  let kind = $state<DeliveryOperation["kind"]>("commit");
  let remote = $state("origin");
  let base = $state("");
  let branch = $state("");
  let runId = $state("");
  let turn = $state(1);
  const needsCheckpoint = $derived(kind === "commit" || kind === "draft_pull_request");
  const blocked = $derived(session.connectionState !== "online" || !delivery.fresh || delivery.busy || delivery.review?.kind === "uncertain" || delivery.review?.kind === "sending");
  const runs = $derived(session.conversationDetail?.runs ?? []);
  $effect(() => {
    const id = session.selectedConversationId;
    const online = session.connectionState === "online";
    const visible = session.workPanelPresented && session.selectedWorkPanel === "delivery";
    untrack(() => {
      if (delivery.conversationId !== id) { runId = ""; turn = 1; branch = ""; remote = "origin"; base = ""; }
      delivery.select(id);
      if (online && id && visible) void delivery.refresh();
      else if (!online) delivery.offline();
    });
  });
  $effect(() => {
    if (session.connectionState !== "online" || !session.selectedConversationId || !session.workPanelPresented || session.selectedWorkPanel !== "delivery") return;
    // History is bounded to the newest 100 operations. A live poll also covers
    // automatic delivery and operations started by another client.
    const timer = setInterval(() => { if (!delivery.loading) void delivery.refresh(); }, 4000);
    return () => clearInterval(timer);
  });
  function prepare() {
    let operation: DeliveryOperation;
    switch (kind) {
      case "branch": operation = { kind, name: branch.trim() }; break;
      case "commit": operation = { kind, run_id: runId, turn }; break;
      case "push": operation = { kind, remote: remote.trim() }; break;
      case "draft_pull_request": operation = { kind, run_id: runId, turn, remote: remote.trim(), base: base.trim() }; break;
    }
    void delivery.prepare(operation);
  }
</script>

<section class="delivery-panel" aria-label="Git delivery">
  <header class="work-heading"><div><h2>Deliver work</h2><p>Review each step before it changes Git or GitHub.</p></div></header>
  {#if !session.selectedConversationId}
    <p>Select a task to review and deliver its work.</p>
  {:else}
    {#if !delivery.fresh}<p role="status">{delivery.loading ? "Loading delivery history…" : "Delivery history is stale or unavailable. Reconnect and refresh before sending work."}</p>{/if}
    {#if delivery.historyError}<div class="work-alert" role="alert"><p>{delivery.historyError.message}</p><small>{delivery.historyError.code}</small></div>{/if}
    {#if delivery.error}<div class="work-alert" role="alert"><p>{delivery.error.message}</p><small>{delivery.error.code}</small></div>{/if}
    <form onsubmit={(event) => { event.preventDefault(); prepare(); }}>
      <fieldset disabled={blocked}>
        <label>Operation<select bind:value={kind}><option value="commit">Commit checkpoint</option><option value="push">Push branch</option><option value="draft_pull_request">GitHub draft PR</option><option value="branch">Create branch</option></select></label>
        {#if kind === "branch"}<label>New branch<input bind:value={branch} required maxlength="255" placeholder="feature/my-change" /></label>{/if}
        {#if needsCheckpoint}
          <label>Run<select bind:value={runId} required><option value="" disabled>Choose a Run</option>{#each runs as run}<option value={run.id}>{run.title || run.id} · {run.lifecycle}</option>{/each}</select></label>
          <label>Retained Turn<input type="number" bind:value={turn} min="1" max="4294967295" step="1" required /></label>
          <p class="muted">Uses this Turn's captured content. Current uncommitted edits are not substituted.</p>
        {/if}
        {#if kind === "push" || kind === "draft_pull_request"}<label>Remote name<input bind:value={remote} required maxlength="255" /></label>{/if}
        {#if kind === "draft_pull_request"}<label>Base branch<input bind:value={base} required maxlength="255" placeholder="main" /></label><p class="muted">GitHub only. Push the branch first. An existing draft for this task is updated.</p>{/if}
        <button type="submit">Review {operationLabel(kind).toLowerCase()}…</button>
      </fieldset>
    </form>

    {#if delivery.review?.kind === "ready"}
      {@const review = delivery.review.review}
      <section class="delivery-review" aria-label="Review delivery">
        <h3>{operationLabel(review.operation.kind)}</h3>
        <dl><dt>Working tree</dt><dd>{review.workingTree}</dd><dt>Task</dt><dd>{session.selectedConversationTitle}</dd>
          {#if "remote" in review.operation}<dt>Remote name</dt><dd>{review.operation.remote}</dd>{/if}
          {#if "base" in review.operation}<dt>PR base</dt><dd>{review.operation.base}</dd>{/if}
          {#if "name" in review.operation}<dt>New branch</dt><dd>{review.operation.name}</dd>{/if}
          {#if "run_id" in review.operation}<dt>Run</dt><dd><code>{review.operation.run_id}</code></dd>{/if}
          {#if "turn" in review.operation}<dt>Checkpoint</dt><dd>Turn {review.operation.turn} · {review.checkpointFiles} changed files</dd>{/if}
        </dl>
        {#if review.operation.kind !== "branch"}<p>The current branch will be used. Jet cannot yet preview its name or the remote URL. Verify both in the working tree before confirming.</p>{/if}
        {#if review.contentComplete === false}<p class="work-alert">This checkpoint has omitted content. The Plane may refuse delivery.</p>{/if}
        <p>Each operation is independent. A later failure leaves completed steps in place. Messages are generated by the Plane from the checkpoint.</p>
        <div class="delivery-actions"><button disabled={blocked} onclick={() => delivery.confirm()}>Confirm {operationLabel(review.operation.kind).toLowerCase()}</button><button onclick={() => delivery.cancel()}>Cancel</button></div>
      </section>
    {:else if delivery.review?.kind === "acknowledge"}
      <section class="delivery-review" aria-label="Acknowledge unknown outcome"><h3>Have you inspected Git and GitHub?</h3><p>This releases the delivery barrier for operation {delivery.review.deliveryId}. Its outcome stays unknown. No Git operation is retried.</p><div class="delivery-actions"><button disabled={blocked} onclick={() => delivery.confirm()}>I inspected it · acknowledge</button><button onclick={() => delivery.cancel()}>Cancel</button></div></section>
    {:else if delivery.review?.kind === "uncertain"}
      <div class="work-alert" role="alert"><h3>Request outcome is uncertain</h3><p>Jet may have accepted this request. Check its acknowledgement using the same request before starting another operation.</p><button disabled={session.connectionState !== "online" || delivery.busy} onclick={() => delivery.confirm()}>Retry same request</button></div>
    {:else if delivery.review?.kind === "sending"}<p role="status">Waiting for the Plane to acknowledge the request…</p>
    {:else if delivery.review?.kind === "queued"}<p role="status">Request accepted. Its delivery result appears in history; acceptance does not mean completion.</p>
    {:else if delivery.review?.kind === "acknowledged"}<p role="status">Acknowledgement recorded. The original outcome remains unknown.</p>{/if}

    <header class="work-heading"><h3>Delivery history</h3><button disabled={delivery.loading || session.connectionState !== "online"} onclick={() => delivery.refresh()}>Refresh</button></header>
    <p aria-live="polite">{deliverySummary(delivery.rows)}</p>
    <ol class="delivery-history">
      {#each delivery.rows as row (row.id)}
        <li><strong>{operationLabel(row.operation)}</strong><span class:delivery-failed={row.status === "failed" || row.status === "outcome_unknown"}>{statusLabel(row)}</span><p>{row.destination}</p>
          {#if row.checkpoint}<p>Checkpoint: <code>{row.checkpoint}</code></p>{/if}
          {#if row.branch}<p>Resulting branch: <code>{row.branch}</code></p>{/if}
          {#if row.head}<p>Commit: <code>{row.head}</code></p>{/if}
          {#if row.title}<p>{row.title}</p>{/if}
          {#if row.pullRequest}<label>GitHub PR URL<input readonly value={row.pullRequest} aria-label="GitHub pull request URL" /></label>{/if}
          {#if row.code}<p class="delivery-failed">{row.code}</p><p>Inspect the cause, then review a new operation above. Completed steps do not need repeating.</p>{/if}
          {#if row.status === "outcome_unknown" && !row.acknowledged}<button disabled={blocked} onclick={() => delivery.acknowledge(row.id)}>Review acknowledgement…</button>{/if}
        </li>
      {/each}
    </ol>
    <p class="muted">Newest 100 operations. Notifications can be configured in Settings.</p>
  {/if}
</section>

<style>
  .delivery-panel { padding: 16px; font-size: 13px; line-height: 1.5; }
  fieldset { border: 0; padding: 0; margin: 0; display: grid; gap: 12px; }
  label { display: grid; gap: 5px; }
  input, select { width: 100%; min-width: 0; color: var(--text); background: var(--raised); border: 1px solid var(--border); padding: 8px; border-radius: 5px; font: inherit; }
  button { background: var(--raised); border: 1px solid var(--border); border-radius: 5px; padding: 8px 10px; cursor: pointer; }
  button:hover:not(:disabled) { background: var(--hover); }
  button:disabled { opacity: .5; cursor: default; }
  .muted { color: var(--muted); margin: 0; }
  h3 { font-size: 14px; margin: 0 0 8px; }
  .delivery-review { border-block: 1px solid var(--border); padding-block: 18px; margin-block: 24px; }
  .delivery-actions { display: flex; flex-wrap: wrap; gap: 8px; }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 6px 12px; }
  dt { color: var(--muted); }
  dd { margin: 0; overflow-wrap: anywhere; }
  .delivery-history { list-style: none; padding: 0; }
  li { padding: 16px 0; border-bottom: 1px solid var(--border-soft); overflow-wrap: anywhere; }
  li > span { display: block; color: var(--muted); }
  li p { margin: 6px 0; }
  .delivery-failed { color: var(--warning); }
  .work-heading { padding: 0; margin-top: 24px; }
  .work-heading:first-child { margin-top: 0; margin-bottom: 20px; }
  .work-heading p { white-space: normal; overflow: visible; max-width: none; }
</style>
