<script lang="ts">
  import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
  import { tick } from "svelte";
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import { landedTarget } from "$lib/features/settings/model";
  import { LOCAL_PLANE } from "$lib/jet/planes";

  let { session }: { session: DesktopSession } = $props();
  let removalName = $state("");
  let permanentRemoval = $state(false);
  let removalDialog = $state<HTMLDialogElement>();
  let removalNameInput = $state<HTMLInputElement>();
  let removalCancelButton = $state<HTMLButtonElement>();
  let removalReturnFocus: HTMLElement | null = null;

  /** Setup is this computer's Plane; its accounts are managed in Settings. */
  const accountsTarget = landedTarget("accounts", LOCAL_PLANE);

  const removalReady = $derived(
    session.removalPreview !== null &&
      session.removalPreview.obstacles.length === 0 &&
      removalName === session.removalPreview.name,
  );

  function closeRemoval(returnFocus = true): void {
    if (removalDialog?.open) removalDialog.close();
    removalName = "";
    permanentRemoval = false;
    session.cancelProjectRemoval();
    if (returnFocus) removalReturnFocus?.focus();
    removalReturnFocus = null;
  }

  /** Leaves this panel for Schedules: its trigger goes away, so focus moves to the destination's heading. */
  async function openSchedules(): Promise<void> {
    closeRemoval(false);
    session.select("schedules");
    await tick();
    document.getElementById("schedules-title")?.focus();
  }

  async function confirmRemoval(): Promise<void> {
    if (!removalReady) return;
    await session.confirmProjectRemoval(removalName, permanentRemoval);
    if (!session.removalPreview) {
      if (removalDialog?.open) removalDialog.close();
      removalName = "";
      permanentRemoval = false;
      removalReturnFocus?.focus();
      removalReturnFocus = null;
    }
  }

  async function chooseProjectFolder(): Promise<void> {
    const path = await openFolderDialog({
      directory: true,
      multiple: false,
      title: "Choose a Project folder",
    });
    if (typeof path !== "string") return;
    session.projectPath = path;
    await session.previewProjectPath();
  }

  async function openRemoval(projectId: string, trigger: EventTarget | null): Promise<void> {
    removalReturnFocus = trigger instanceof HTMLElement ? trigger : null;
    await session.prepareProjectRemoval(projectId);
    if (!session.removalPreview) return;
    await tick();
    removalDialog?.showModal();
    if (session.removalPreview.obstacles.length > 0) {
      removalCancelButton?.focus();
    } else {
      removalNameInput?.focus();
    }
  }

  function formatBytes(value: string): string {
    const bytes = Number(value);
    if (!Number.isFinite(bytes) || bytes < 0) return "Unknown size";
    if (bytes < 1024) return `${bytes} B`;
    const units = ["KB", "MB", "GB", "TB"];
    let amount = bytes;
    let unit = -1;
    do {
      amount /= 1024;
      unit += 1;
    } while (amount >= 1024 && unit < units.length - 1);
    return `${amount < 10 ? amount.toFixed(1) : amount.toFixed(0)} ${units[unit]}`;
  }
</script>

<section class="setup-panel" aria-labelledby="setup-title">
  <header class="setup-header">
    <div>
      <h1 id="setup-title">Set up this workspace</h1>
      <p>Connect the local Plane, choose a Project, and use a Harness login already available here.</p>
    </div>
    <button
      class="secondary-button"
      disabled={session.setup.kind === "loading" || session.setupBusy !== null}
      onclick={() => session.refreshSetup()}
    >
      Refresh
    </button>
  </header>

  {#if session.setup.kind === "loading"}
    <div class="setup-loading" aria-live="polite">
      <span class="loading-bar"></span>
      <span class="loading-bar short"></span>
      <span class="loading-bar"></span>
    </div>
  {:else if session.setup.kind === "failed"}
    <div class="setup-failure" role="alert">
      <div>
        <h2>Local Plane unavailable</h2>
        <p>{session.setup.error.message}</p>
        <code>{session.setup.error.code}</code>
      </div>
      <button class="primary-button" onclick={() => session.refreshSetup()}>Try again</button>
    </div>
  {:else}
    {@const setup = session.setup.snapshot}
    {@const capabilitiesIssue = session.setupIssue("capabilities")}
    {@const projectsIssue = session.setupIssue("projects")}
    {@const accountsIssue = session.setupIssue("accounts")}
    {@const pairingIssue = session.setupIssue("pairing")}
    {@const serviceNeedsAttention = capabilitiesIssue !== null || setup.capabilities.degraded.length > 0}
    <div class="setup-content">
      <section class="setup-row service-row" aria-labelledby="service-heading">
        <div class:warning={serviceNeedsAttention} class="setup-icon online" aria-hidden="true"></div>
        <div class="setup-copy">
          <h2 id="service-heading">Local Plane</h2>
          <p>{setup.plane.platform} · Core {setup.plane.coreVersion}</p>
          {#if setup.capabilities.degraded.length > 0}
              <p class="service-warning">{setup.capabilities.degraded.join(" · ")}</p>
          {/if}
          {#if capabilitiesIssue}
            <p class="service-warning">{capabilitiesIssue.error.message} <code>{capabilitiesIssue.error.code}</code></p>
          {/if}
        </div>
        <span class:warning={serviceNeedsAttention} class:success={!serviceNeedsAttention} class="status-text">
          {serviceNeedsAttention ? "Needs attention" : "Connected"}
        </span>
      </section>

      <section class="setup-row expanded" aria-labelledby="projects-heading">
        <div class="setup-icon folder" aria-hidden="true"></div>
        <div class="setup-copy setup-wide">
          <div class="setup-row-heading">
            <div>
              <h2 id="projects-heading">Projects</h2>
              <p>Jet registers the Git working tree only after you review its resolved path.</p>
            </div>
            <span class="status-text">{projectsIssue ? "Unavailable" : `${setup.projects.length} registered`}</span>
          </div>

          {#if projectsIssue}
            <p class="section-error">{projectsIssue.error.message} <code>{projectsIssue.error.code}</code></p>
          {:else if setup.projects.length > 0}
            <div class="project-list">
              {#each setup.projects as project (project.id)}
                <div class:selected={session.selectedProjectId === project.id} class="project-line">
                  <button class="project-select" onclick={() => session.selectProject(project.id)}>
                    <strong>{project.name}</strong>
                    <small title={project.root}>{project.root}</small>
                  </button>
                  <span>{session.selectedProjectId === project.id ? "Selected" : ""}</span>
                  <button
                    aria-label={`Remove ${project.name}`}
                    class="text-button danger"
                    disabled={session.setupBusy !== null}
                    onclick={(event) => openRemoval(project.id, event.currentTarget)}
                  >
                    Remove
                  </button>
                </div>
              {/each}
            </div>
          {:else}
            <p class="inline-empty">No Projects yet. Add the root folder of a Git working tree.</p>
          {/if}

          <button
            class="secondary-button choose-folder"
            disabled={session.setupBusy !== null}
            onclick={chooseProjectFolder}
          >
            Choose Folder…
          </button>

          <details class="expert-path">
            <summary>Enter a path instead</summary>
            <form class="path-form" onsubmit={(event) => { event.preventDefault(); session.previewProjectPath(); }}>
              <label for="project-path">Absolute Project path</label>
              <div class="field-action">
                <input
                  id="project-path"
                  bind:value={session.projectPath}
                  maxlength="4096"
                  placeholder="/home/you/code/project"
                  spellcheck="false"
                />
                <button
                  class="secondary-button"
                  disabled={!session.projectPath.trim() || session.setupBusy !== null}
                  type="submit"
                >
                  Review
                </button>
              </div>
            </form>
          </details>

          {#if session.projectPreview}
            <div class:warning={session.projectPreview.previewId === null} class="project-preview">
              <div>
                <strong>{session.projectPreview.previewId ? "Ready to add" : "Choose another folder"}</strong>
                <p class="path" title={session.projectPreview.root}>{session.projectPreview.root}</p>
                <p>{session.projectPreview.detail}</p>
              </div>
              {#if session.projectPreview.previewId}
                <button
                  class="primary-button"
                  disabled={session.setupBusy !== null}
                  onclick={() => session.registerPreviewedProject()}
                >
                  Add Project
                </button>
              {/if}
            </div>
          {/if}
        </div>
      </section>

      <section class="setup-row expanded" aria-labelledby="accounts-heading">
        <div class="setup-icon account" aria-hidden="true"></div>
        <div class="setup-copy setup-wide">
          <div class="setup-row-heading">
            <div>
              <h2 id="accounts-heading">Harness access</h2>
              <p>Jet records a non-secret binding. Sign-in stays with the Harness when work starts.</p>
            </div>
            <span class:warning={setup.capabilities.credentialStore !== "available"} class="status-text">
              {setup.capabilities.credentialStoreLabel}
            </span>
          </div>
          {#if accountsTarget && (accountsIssue || setup.capabilities.credentialStore !== "available")}
            <button class="text-button" onclick={() => void session.openSettings(accountsTarget)}>
              Open Agents settings
            </button>
          {/if}

          {#if setup.accounts.length > 0}
            <div class="account-list">
              {#each setup.accounts as account (account.id)}
                <div class="account-line">
                  <span><strong>{account.label}</strong><small>{account.provider}</small></span>
                  <span class="status-text">{account.stateLabel}</span>
                </div>
              {/each}
            </div>
          {:else if accountsIssue}
            <p class="section-error">{accountsIssue.error.message} <code>{accountsIssue.error.code}</code></p>
          {/if}

          <div class="provider-actions">
            {#if capabilitiesIssue}
              <p class="inline-empty">Harness choices will return when Plane capabilities are available.</p>
            {:else}
              {#each setup.capabilities.authProviders as option (option.provider)}
                <button
                  class="secondary-button"
                  disabled={session.setupBusy !== null}
                  onclick={() => session.connectHarness(option.provider)}
                >
                  Use {option.harness} login
                </button>
              {:else}
                <p class="inline-empty">Install a supported Craft before connecting a Harness.</p>
              {/each}
            {/if}
          </div>
        </div>
      </section>

      <section class="setup-row" aria-labelledby="remote-heading">
        <div class="setup-icon remote" aria-hidden="true"></div>
        <div class="setup-copy">
          <h2 id="remote-heading">Remote Plane</h2>
          {#if pairingIssue}
            <p class="section-error">{pairingIssue.error.message} <code>{pairingIssue.error.code}</code></p>
          {:else}
            <p>
              {setup.pairing.pairedClients > 0
                ? `${setup.pairing.pairedClients} paired client${setup.pairing.pairedClients === 1 ? "" : "s"}`
                : "Pair another computer later from Connections."}
            </p>
          {/if}
        </div>
        {#if pairingIssue}
          <span class="status-text">Unavailable</span>
        {:else if session.remotePairingSkipped}
          <span class="status-text">Skipped</span>
        {:else}
          <div class="remote-actions">
            <button class="text-button" onclick={() => session.openAddPlane()}>Add a Plane</button>
            <button class="text-button" onclick={() => session.skipRemotePairing()}>Skip for now</button>
          </div>
        {/if}
      </section>

      {#if session.setupNotice}
        <p class="setup-notice" role="status">{session.setupNotice}</p>
      {/if}
    </div>
  {/if}
</section>

{#if session.removalPreview}
  {@const removal = session.removalPreview}
  <dialog
    aria-labelledby="removal-title"
    bind:this={removalDialog}
    class="removal-dialog"
    oncancel={(event) => { event.preventDefault(); closeRemoval(); }}
  >
    <form method="dialog" onsubmit={(event) => event.preventDefault()}>
      <header>
        <h2 id="removal-title">Remove {removal.name}?</h2>
        <p>This removes Jet's registration and the Project folder.</p>
      </header>

      <dl class="removal-facts">
        <div><dt>Folder</dt><dd title={removal.root}>{removal.root}</dd></div>
        <div><dt>On disk</dt><dd>{formatBytes(removal.diskUseBytes)}</dd></div>
        <div><dt>Changed files</dt><dd>{removal.dirtyFiles}</dd></div>
        <div><dt>Unpushed commits</dt><dd>{removal.unpushedCommits}</dd></div>
        <div><dt>Workspaces</dt><dd>{removal.workspaceCount}</dd></div>
      </dl>

      {#if removal.obstacles.length > 0}
        <div class="removal-obstacles" role="alert">
          <strong>This Project cannot be removed yet.</strong>
          <ul>
            {#each removal.obstacles as obstacle}
              <li>
                {obstacle}
                {#if obstacle === "Disable scheduled tasks first"}
                  <button class="text-button" type="button" onclick={() => void openSchedules()}>
                    Open Schedules
                  </button>
                {/if}
              </li>
            {/each}
          </ul>
        </div>
      {:else}
        <label for="confirm-project-name">Type {removal.name} to confirm</label>
        <input bind:this={removalNameInput} id="confirm-project-name" bind:value={removalName} autocomplete="off" />

        {#if session.permanentRemovalAllowed}
          <label class="checkbox-row">
            <input bind:checked={permanentRemoval} type="checkbox" />
            <span>
              Delete permanently
              <small>Trash is unavailable. {removal.permanentWarning}</small>
            </span>
          </label>
        {/if}
      {/if}

      <footer>
        <button bind:this={removalCancelButton} class="secondary-button" type="button" onclick={() => closeRemoval()}>Cancel</button>
        <button
          class="danger-button"
          disabled={!removalReady || session.setupBusy !== null}
          type="button"
          onclick={confirmRemoval}
        >
          {permanentRemoval ? "Delete permanently" : "Move to Trash"}
        </button>
      </footer>
    </form>
  </dialog>
{/if}
