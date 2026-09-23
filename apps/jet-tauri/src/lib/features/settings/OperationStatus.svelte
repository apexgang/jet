<script lang="ts">
  import type { AgentsSession, OperationTarget } from "./agents-session.svelte";
  import { receiptText } from "./agents-model";
  import { refusalText } from "./model";

  let {
    agents,
    matches,
    planeLabel,
    oncheck,
  }: {
    agents: AgentsSession;
    /** Which operations this line reports. */
    matches: (target: OperationTarget) => boolean;
    planeLabel: string;
    /** Reloads what the change touched, after an uncertain outcome. */
    oncheck: () => void;
  } = $props();

  const operation = $derived(
    agents.operation.kind !== "idle" && agents.operation.kind !== "confirm" && matches(agents.operation.target)
      ? agents.operation
      : null,
  );
</script>

<div class="operation-status" role="status">
  {#if operation?.kind === "preparing"}
    <p>Checking with {planeLabel}…</p>
  {:else if operation?.kind === "applying"}
    <p>Saving…</p>
  {:else if operation?.kind === "uncertain"}
    <div class="operation-notice">
      <p>Jet couldn't confirm this change. It may have been saved. <code>{operation.error.code}</code></p>
      <button class="text-button" onclick={() => void agents.retry()}>Retry same change</button>
      <button class="text-button" onclick={oncheck}>Check again</button>
    </div>
  {:else if operation?.kind === "refused"}
    <div class="operation-notice critical">
      <p>{refusalText(operation.error)} <code>{operation.error.code}</code></p>
      <button class="text-button" onclick={() => agents.dismiss()}>Dismiss</button>
    </div>
  {:else if operation?.kind === "done"}
    <div class="operation-notice done">
      <p>{receiptText(operation.detail)}</p>
      <button class="text-button" onclick={() => agents.dismiss()}>Dismiss</button>
    </div>
  {/if}
</div>

<style>
  .operation-status {
    font-size: 12px;
  }

  .operation-status:empty {
    display: none;
  }

  .operation-status p {
    margin: 0;
    color: var(--muted);
  }

  .operation-notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--warning) 8%, var(--raised));
  }

  .operation-notice p {
    flex: 1 1 240px;
    color: var(--text) !important;
  }

  .operation-notice.critical {
    background: color-mix(in srgb, var(--danger) 10%, var(--raised));
  }

  .operation-notice.done {
    background: color-mix(in srgb, var(--success) 10%, var(--raised));
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
