<script lang="ts">
  import SectionState from "$lib/features/settings/SectionState.svelte";
  import { sectionData, sectionHeadingId, withIssues } from "$lib/features/settings/model";
  import type { SystemHealth } from "$lib/jet/system";
  import { diagnosticSummary } from "./model";
  import type { SystemSession } from "./session.svelte";

  let { system, planeLabel }: { system: SystemSession; planeLabel: string } = $props();

  type Copy = { kind: "idle" } | { kind: "copied" } | { kind: "failed" };

  let copy = $state<Copy>({ kind: "idle" });
  let summaryElement = $state<HTMLPreElement>();

  const view = $derived(withIssues(system.health, ["retention"]));
  const summary = $derived(diagnosticSummary(sectionData(system.health), system.recentCodes));

  // A new summary makes an earlier "Copied" untrue.
  $effect(() => {
    void summary;
    copy = { kind: "idle" };
  });

  function selectSummary() {
    const selection = window.getSelection();
    if (summaryElement && selection) selection.selectAllChildren(summaryElement);
  }

  /** Called from the click handler: WebKitGTK allows clipboard writes only there. */
  function copySummary() {
    const text = summary;
    const clipboard = navigator.clipboard;
    if (!clipboard) {
      copy = { kind: "failed" };
      selectSummary();
      return;
    }
    clipboard.writeText(text).then(
      () => {
        if (text === summary) copy = { kind: "copied" };
      },
      () => {
        copy = { kind: "failed" };
        selectSummary();
      },
    );
  }
</script>

<section class="settings-section" aria-labelledby={sectionHeadingId("diagnostics")}>
  <h2 id={sectionHeadingId("diagnostics")} tabindex="-1">Diagnostics</h2>
  <p>
    Jet keeps a local diagnostic log on the computer running {planeLabel}. Nothing is sent anywhere. This app can't show
    that log yet.
  </p>
  <SectionState state={view} title="Diagnostics" {planeLabel} onretry={() => void system.load()}>
    {#snippet children(_health: SystemHealth)}{/snippet}
  </SectionState>
  <p>
    This summary has versions, states and error codes only. It never includes task content, file paths, names or
    identifiers.
  </p>
  <pre bind:this={summaryElement} class="summary" aria-label="Diagnostic summary">{summary}</pre>
  <div class="actions">
    <button class="secondary-button" onclick={copySummary}>Copy summary</button>
    <span role="status">
      {#if copy.kind === "copied"}Copied.{:else if copy.kind === "failed"}Select the text and copy it.{/if}
    </span>
  </div>
</section>

<style>
  .summary {
    margin: 0;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--raised);
    color: var(--text);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    user-select: text;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
    color: var(--muted);
    font-size: 13px;
  }
</style>
