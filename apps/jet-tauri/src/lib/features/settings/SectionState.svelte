<script lang="ts" generics="T">
  import type { Snippet } from "svelte";

  import { recoveryFor, sectionData, type SectionState } from "./model";

  let {
    state,
    title,
    planeLabel,
    onretry,
    children,
    empty,
  }: {
    state: SectionState<T>;
    /** The section's title, for the whole-section unsupported copy. */
    title: string;
    planeLabel: string;
    /** Try again, Check again and Show current values all reload the section. */
    onretry?: () => void;
    children: Snippet<[T]>;
    empty?: Snippet;
  } = $props();

  const data = $derived(sectionData(state));
  const recovery = $derived(recoveryFor(state));
</script>

<div class="section-state" aria-busy={state.kind === "loading"}>
  {#if state.kind === "loading" && data === null}
    <span class="visually-hidden">Loading {title}</span>
    <span class="loading-bar" aria-hidden="true"></span>
    <span class="loading-bar short" aria-hidden="true"></span>
  {:else if state.kind === "empty"}
    {@render empty?.()}
  {:else if state.kind === "unsupported"}
    <p class="notice" role="status">
      {title} isn't available on {planeLabel} yet. Other settings still work.
      <code>{state.error.code}</code>
    </p>
  {:else}
    {#if state.kind === "ready" && state.freshness === "changed"}
      <div class="notice" role="status">
        <p>Changed on {planeLabel} since you opened this page.</p>
        {#if onretry}<button class="text-button" onclick={onretry}>Show current values</button>{/if}
      </div>
    {:else if state.kind === "ready" && state.freshness === "stale"}
      <p class="notice" role="status">Showing the last values {planeLabel} reported. Reconnecting…</p>
    {:else if state.kind === "offline"}
      <div class="notice" role="status">
        <p>
          Jet can't reach {planeLabel} right now.
          {#if data !== null}Showing the last values it reported. Changes are paused until it reconnects.{/if}
          <code>{state.error.code}</code>
        </p>
        {#if onretry}<button class="text-button" onclick={onretry}>Try again</button>{/if}
      </div>
    {:else if state.kind === "denied"}
      <p class="notice critical" role="status">
        This device isn't allowed to change these settings on {planeLabel}.
        <code>{state.error.code}</code>
      </p>
    {:else if state.kind === "failed"}
      <div class="notice critical" role="status">
        <p>{state.error.message} <code>{state.error.code}</code></p>
        {#if onretry && (recovery === "try_again" || recovery === "check_again")}
          <button class="text-button" onclick={onretry}>
            {recovery === "try_again" ? "Try again" : "Check again"}
          </button>
        {/if}
      </div>
    {/if}
    {#if state.kind === "ready"}
      {#each state.issues as issue (issue.section)}
        <p class="notice" role="status">{issue.error.message} <code>{issue.error.code}</code></p>
      {/each}
    {/if}
    {#if data !== null}
      {@render children(data)}
    {/if}
  {/if}
</div>

<style>
  .section-state {
    display: grid;
    gap: 12px;
  }

  .notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin: 0;
    color: var(--text);
    font-size: 13px;
    line-height: 1.55;
  }

  .notice p {
    margin: 0;
    color: var(--text);
  }

  code {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
