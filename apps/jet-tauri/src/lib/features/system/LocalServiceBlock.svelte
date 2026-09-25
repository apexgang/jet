<script lang="ts">
  import { onMount } from "svelte";

  import SettingsDialog from "$lib/features/settings/SettingsDialog.svelte";
  import type { LocalServiceSession } from "./local-service.svelte";
  import { keepFocus } from "./keep-focus";
  import {
    actionText,
    channelText,
    managerText,
    phaseText,
    provisioningText,
    rollbackLines,
    rollbackRefusedNote,
    serviceErrorText,
  } from "./service-model";

  let { service }: { service: LocalServiceSession } = $props();

  const HEADING_ID = "local-service-heading";

  onMount(() => void service.start());

  const view = $derived(service.view);
  const dialog = $derived(service.rollback);
  const working = $derived(service.provisioning || service.repairing);
  /** Progress and outcomes are spoken without moving focus. */
  const liveText = $derived(view === null ? "" : (provisioningText(view.phase) ?? actionText(view) ?? ""));
  const manager = $derived(view === null ? null : managerText(view.manager));
  /** Which button started this window's pass, so it keeps its place and name. */
  let repairKind = $state<"repair" | "check">("repair");
  /**
   * Repair (or Check again) stays while its own pass runs, so focus stays on
   * it; it is `aria-disabled` then, since `disabled` would drop focus. When
   * it goes, `keepFocus` moves focus to the next control or the heading.
   */
  const repair = $derived.by((): { kind: "repair" | "check"; label: string } | null => {
    if (view === null) return null;
    if (service.repairing) return { kind: repairKind, label: repairKind === "repair" ? "Repairing…" : "Checking…" };
    if (view.canRepair) return { kind: "repair", label: "Repair" };
    if (view.phase !== "running" && !service.provisioning) return { kind: "check", label: "Check again" };
    return null;
  });

  const heading = () => document.getElementById(HEADING_ID);

  function dialogTitle(): string {
    switch (dialog.kind) {
      case "review":
      case "sending":
        return `Go back to Jet service ${dialog.review.previousVersion}?`;
      case "done":
        return "Jet service rolled back";
      case "refused":
        return "Jet didn't roll back the service";
      default:
        return "Roll back";
    }
  }
</script>

<div class="local-service" use:keepFocus={heading}>
  <h3 id={HEADING_ID} tabindex="-1">Jet service on this computer</h3>
  {#if view === null}
    {#if service.error}
      <p class="notice critical" role="status">
        Jet couldn't read the state of its service. <code>{service.error.code}</code>
      </p>
    {:else}
      <span class="loading-bar short" aria-hidden="true"></span>
    {/if}
  {:else}
    <dl class="facts">
      <dt>Managed by</dt>
      <dd>
        {channelText(view.channel)}
        {#if manager}<span class="quiet">{manager}</span>{/if}
      </dd>
      <dt>Status</dt>
      <dd>{phaseText(view.phase)}</dd>
      <dt>Version</dt>
      <dd>{view.currentVersion ?? view.runningVersion ?? "Not reported"}</dd>
      {#if view.runningVersion && view.currentVersion && view.runningVersion !== view.currentVersion}
        <dt>Running now</dt>
        <dd>{view.runningVersion}</dd>
      {/if}
      {#if view.previousVersion}
        <dt>Previous version</dt>
        <dd>{view.previousVersion}</dd>
      {/if}
      {#if view.bundledVersion && view.channel !== "homebrew"}
        <dt>Included with this app</dt>
        <dd>{view.bundledVersion}</dd>
      {/if}
    </dl>

    <p class="quiet" role="status" aria-live="polite">{liveText}</p>
    {#if view.error}
      <!-- A failed pass is announced when it appears, wherever focus is. -->
      <p class="notice critical" role="alert">{serviceErrorText(view.error)} <code>{view.error.code}</code></p>
    {/if}
    {#if service.repairError}
      <p class="notice critical" role="status">
        {service.repairError.message} <code>{service.repairError.code}</code>
      </p>
    {/if}

    <div class="actions">
      {#if repair}
        <button
          class="secondary-button"
          aria-disabled={working}
          aria-busy={service.repairing}
          onclick={() => {
            if (working || repair === null) return;
            repairKind = repair.kind;
            void service.repair();
          }}
        >
          {repair.label}
        </button>
      {/if}
      {#if view.canRollback && view.previousVersion}
        <button
          class="secondary-button"
          disabled={working || dialog.kind !== "closed"}
          aria-busy={dialog.kind === "preparing"}
          onclick={() => void service.prepareRollback()}
        >
          {dialog.kind === "preparing" ? "Checking…" : `Roll back to ${view.previousVersion}…`}
        </button>
      {/if}
    </div>
  {/if}
</div>

{#if dialog.kind !== "closed" && dialog.kind !== "preparing"}
  <SettingsDialog
    title={dialogTitle()}
    lead={dialog.kind === "review" || dialog.kind === "sending" ? "On this computer" : undefined}
    focus={dialog.kind === "review" ? "cancel" : "primary"}
    returnFocus={[HEADING_ID]}
    focusKey={dialog.kind}
    oncancel={() => service.closeRollback()}
  >
    {#if dialog.kind === "review" || dialog.kind === "sending"}
      <ul class="consequences">
        {#each rollbackLines(dialog.review.currentVersion, dialog.review.previousVersion) as line (line)}
          <li>{line}</li>
        {/each}
      </ul>
    {:else if dialog.kind === "done"}
      <p role="status">{actionText(dialog.view) ?? "The Jet service went back to the earlier version."}</p>
      {#if dialog.view.error}
        <p class="dialog-note">{serviceErrorText(dialog.view.error)} <code>{dialog.view.error.code}</code></p>
      {/if}
    {:else if dialog.kind === "refused"}
      <p role="alert">{serviceErrorText(dialog.error)} <code>{dialog.error.code}</code></p>
      <p class="dialog-note">{rollbackRefusedNote(dialog.error)}</p>
    {/if}
    {#snippet footer()}
      {#if dialog.kind === "review" || dialog.kind === "sending"}
        <button
          type="button"
          class="secondary-button"
          data-dialog-cancel
          disabled={dialog.kind === "sending"}
          onclick={() => service.closeRollback()}
        >
          Cancel
        </button>
        <button
          type="button"
          class="danger-button"
          data-dialog-primary
          disabled={dialog.kind === "sending"}
          onclick={() => void service.confirmRollback()}
        >
          {dialog.kind === "sending" ? "Rolling back…" : "Roll back"}
        </button>
      {:else}
        <button type="button" class="primary-button" data-dialog-primary onclick={() => service.closeRollback()}>
          Close
        </button>
      {/if}
    {/snippet}
  </SettingsDialog>
{/if}

<style>
  .local-service {
    display: grid;
    gap: 10px;
  }

  .local-service h3 {
    margin: 0;
  }

  .facts {
    display: grid;
    grid-template-columns: minmax(110px, max-content) minmax(0, 1fr);
    gap: 6px 16px;
    margin: 0;
    font-size: 13px;
  }

  .facts dt {
    color: var(--muted);
  }

  .facts dd {
    display: grid;
    margin: 0;
    color: var(--text);
    overflow-wrap: anywhere;
  }

  .quiet {
    color: var(--muted);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .consequences {
    display: grid;
    gap: 6px;
    margin: 0;
    padding-left: 18px;
    font-size: 13px;
  }
</style>
