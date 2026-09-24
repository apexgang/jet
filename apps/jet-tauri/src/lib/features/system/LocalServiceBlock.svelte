<script lang="ts">
  import { onMount } from "svelte";

  import SettingsDialog from "$lib/features/settings/SettingsDialog.svelte";
  import type { LocalServiceSession } from "./local-service.svelte";
  import {
    actionText,
    channelText,
    managerText,
    phaseText,
    provisioningText,
    rollbackLines,
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

<div class="local-service">
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
      <p class="notice critical">{serviceErrorText(view.error)} <code>{view.error.code}</code></p>
    {/if}
    {#if service.repairError}
      <p class="notice critical" role="status">
        {service.repairError.message} <code>{service.repairError.code}</code>
      </p>
    {/if}

    <div class="actions">
      {#if view.canRepair}
        <button class="secondary-button" disabled={working} onclick={() => void service.repair()}>
          {service.repairing ? "Repairing…" : "Repair"}
        </button>
      {:else if view.phase !== "running" && !service.provisioning}
        <button class="secondary-button" disabled={working} onclick={() => void service.repair()}>
          {service.repairing ? "Checking…" : "Check again"}
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
      <p class="dialog-note">Nothing was changed.</p>
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
