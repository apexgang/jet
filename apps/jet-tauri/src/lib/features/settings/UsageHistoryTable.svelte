<script lang="ts">
  import type { AccountView, HistoryDays, UsageHistoryView } from "$lib/jet/agents";
  import type { AgentsSession } from "./agents-session.svelte";
  import { bucketLabel, formatCount, historyRows } from "./agents-model";
  import SectionState from "./SectionState.svelte";

  let {
    agents,
    accounts,
    planeLabel,
  }: {
    agents: AgentsSession;
    accounts: ReadonlyArray<AccountView>;
    planeLabel: string;
  } = $props();

  const RANGES: ReadonlyArray<{ days: HistoryDays; label: string }> = [
    { days: 1, label: "Last 24 hours" },
    { days: 7, label: "Last 7 days" },
    { days: 30, label: "Last 30 days" },
    { days: 90, label: "Last 90 days" },
  ];
  const id = $props.id();

  const rangeLabel = $derived(RANGES.find((range) => range.days === agents.historyDays)?.label ?? "");

  function chooseRange(value: string): void {
    const days = Number(value) as HistoryDays;
    if (RANGES.some((range) => range.days === days)) void agents.loadHistory(days);
  }

  function chooseAccount(value: string): void {
    void agents.loadHistory(agents.historyDays, value === "" ? null : value);
  }
</script>

<div class="history-controls">
  <label for={`${id}-range`}>Range</label>
  <select id={`${id}-range`} value={String(agents.historyDays)} onchange={(event) => chooseRange(event.currentTarget.value)}>
    {#each RANGES as range (range.days)}
      <option value={String(range.days)}>{range.label}</option>
    {/each}
  </select>
  {#if accounts.length > 0}
    <label for={`${id}-account`}>Account</label>
    <select
      id={`${id}-account`}
      value={agents.historyBinding ?? ""}
      onchange={(event) => chooseAccount(event.currentTarget.value)}
    >
      <option value="">Whole Plane</option>
      {#each accounts as account (account.id)}
        <option value={account.id}>{account.label} · {account.provider}</option>
      {/each}
    </select>
  {/if}
</div>

<SectionState
  state={agents.history}
  title="Usage history"
  {planeLabel}
  onretry={() => void agents.loadHistory()}
>
  {#snippet children(history: UsageHistoryView)}
    {@const rows = historyRows(history)}
    {#if rows.length === 0}
      <p>No usage recorded on {planeLabel} yet.</p>
    {:else}
      {#if history.truncated}<p>Showing the most recent data only.</p>{/if}
      <div class="table-scroll">
        <table>
          <caption>{history.resolution === "hour" ? "Hourly" : "Daily"} usage, {rangeLabel.toLowerCase()}</caption>
          <thead>
            <tr>
              <th scope="col">{history.resolution === "hour" ? "Hour" : "Day"}</th>
              <th scope="col">Input</th>
              <th scope="col">Cached input</th>
              <th scope="col">Output</th>
              <th scope="col">Reasoning</th>
              <th scope="col">Measurements</th>
            </tr>
          </thead>
          <tbody>
            {#each rows as row (row.startUnixMs)}
              <tr>
                <th scope="row">{bucketLabel(row.startUnixMs, history.resolution)}</th>
                <td>{formatCount(row.tokens.input)}</td>
                <td>{formatCount(row.tokens.cachedInput)}</td>
                <td>{formatCount(row.tokens.output)}</td>
                <td>{formatCount(row.tokens.reasoning)}</td>
                <td>
                  {formatCount(row.measurements)}{#if row.estimated !== "0"}<span class="estimated">
                      ({formatCount(row.estimated)} estimated)</span
                    >{/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/snippet}
</SectionState>

<style>
  .history-controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 10px;
  }

  .history-controls label {
    color: var(--muted);
    font-size: 13px;
  }

  select {
    max-width: 100%;
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--raised);
    color: var(--text);
    font: inherit;
  }

  .table-scroll {
    overflow-x: auto;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  caption {
    padding-bottom: 6px;
    color: var(--muted);
    text-align: left;
  }

  th,
  td {
    padding: 6px 8px;
    border-top: 1px solid var(--border-soft);
    text-align: right;
    white-space: nowrap;
  }

  thead th {
    border-top: 0;
    color: var(--muted);
    font-weight: 600;
  }

  th:first-child {
    text-align: left;
  }

  tbody th {
    font-weight: 500;
  }

  .estimated {
    color: var(--muted);
  }
</style>
