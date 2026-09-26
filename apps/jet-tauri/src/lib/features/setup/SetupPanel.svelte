<script lang="ts">
  import { chooseProjectFolder as pickProjectFolder } from "$lib/jet/project-folder";
  import { tick } from "svelte";
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import SidebarToggle from "$lib/features/shell/SidebarToggle.svelte";
  import { landedTarget } from "$lib/features/settings/model";
  import { actionText, provisioningText, serviceProblem } from "$lib/features/system/service-model";
  import { LOCAL_PLANE } from "$lib/jet/planes";

  let { session }: { session: DesktopSession } = $props();
  let removalName = $state("");
  let permanentRemoval = $state(false);
  let removalDialog = $state<HTMLDialogElement>();
  let removalNameInput = $state<HTMLInputElement>();
  let removalCancelButton = $state<HTMLButtonElement>();
  let removalReturnFocus: HTMLElement | null = null;
  let chooseFolderButton = $state<HTMLButtonElement>();

  // Ctrl+Shift+O: "Choose Folder…" takes focus once Setup has loaded.
  $effect(() => {
    if (chooseFolderButton && session.takeFocusRequest("project-folder")) chooseFolderButton.focus();
  });

  /** Setup is this computer's Plane; its accounts are managed in Settings. */
  const accountsTarget = landedTarget("accounts", LOCAL_PLANE);

  /** The local Jet service (Wave 4 §A), as this window's watcher last saw it. */
  const service = $derived(session.service.view);
  /** While the shell installs, updates or starts the service, Setup says so instead of "unavailable". */
  const provisioning = $derived(
    session.setup.kind !== "ready" && service !== null ? provisioningText(service.phase) : null,
  );
  const problem = $derived(service !== null ? serviceProblem(service) : null);
  /**
   * Repair re-runs the native decision table. Offered when it can install or
   * start the service, and when a service this app or Homebrew manages
   * stopped answering after it was set up.
   */
  const repairable = $derived(
    service !== null &&
      !session.service.provisioning &&
      (service.canRepair ||
        (session.setup.kind === "failed" &&
          session.setup.error.category === "offline" &&
          (service.channel === "gui" || service.channel === "homebrew"))),
  );
  const serviceNotice = $derived(service !== null ? actionText(service) : null);

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
    const path = await pickProjectFolder();
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
    <SidebarToggle {session} />
    <div>
      <h1 id="setup-title">Set up Jet</h1>
      <p>Choose where to work and which Harness to use. You can change these for every new task.</p>
    </div>
    <button
      class="secondary-button"
      disabled={session.setup.kind === "loading" || session.setupBusy !== null}
      onclick={() => session.refreshSetup()}
    >
      Refresh
    </button>
  </header>

  {#if provisioning}
    <div class="setup-provisioning" role="status" aria-live="polite">
      <span class="loading-bar" aria-hidden="true"></span>
      <div>
        <h2>Connecting to the local Jet service</h2>
        <p>{provisioning}</p>
      </div>
    </div>
  {:else if session.setup.kind === "loading"}
    <div class="setup-loading" aria-live="polite">
      <span class="loading-bar"></span>
      <span class="loading-bar short"></span>
      <span class="loading-bar"></span>
    </div>
  {:else if session.setup.kind === "failed"}
    {@const code = problem ? problem.code : session.setup.error.code}
    <div class="setup-failure" role="alert">
      <div>
        <h2>{problem?.title ?? "Local Plane unavailable"}</h2>
        <p>{problem?.detail ?? session.setup.error.message}</p>
        {#if code}<code>{code}</code>{/if}
        {#if session.service.repairError}
          <p class="section-error">{session.service.repairError.message} <code>{session.service.repairError.code}</code></p>
        {/if}
      </div>
      <div class="failure-actions">
        {#if repairable}
          <button
            class="primary-button"
            disabled={session.service.repairing}
            onclick={() => void session.service.repair()}
          >
            {session.service.repairing ? "Repairing…" : "Repair"}
          </button>
          <button class="secondary-button" onclick={() => session.refreshSetup()}>Try again</button>
        {:else if problem}
          <!-- Nothing to install or start: check the service again (a jetd started by hand is found). -->
          <button
            class="primary-button"
            disabled={session.service.repairing}
            onclick={() => void session.service.repair()}
          >
            {session.service.repairing ? "Checking…" : "Check again"}
          </button>
        {:else}
          <button class="primary-button" onclick={() => session.refreshSetup()}>Try again</button>
        {/if}
      </div>
    </div>
  {:else}
    {@const setup = session.setup.snapshot}
    {@const capabilitiesIssue = session.setupIssue("capabilities")}
    {@const projectsIssue = session.setupIssue("projects")}
    {@const accountsIssue = session.setupIssue("accounts")}
    {@const pairingIssue = session.setupIssue("pairing")}
    {@const serviceNeedsAttention = capabilitiesIssue !== null || setup.capabilities.degraded.length > 0}
    <div class="setup-content setup-workflow">
      <section class="onboarding-step" aria-labelledby="projects-heading">
        <span class="step-number" aria-hidden="true">01</span>
        <div>
          <h2 id="projects-heading">Give your work a home</h2>
          <p class="step-description">A Project is a folder tracked with Git. Jet gives each task a separate working copy, so changes stay out of your way until you review them.</p>
          {#if projectsIssue}<p class="section-error">{projectsIssue.error.message}</p>{/if}
          <div class="project-list">
            {#each setup.projects as project (project.id)}
              <div class="project-line" class:selected={session.selectedProjectId === project.id}>
                <button class="project-select" onclick={() => session.selectProject(project.id)}>
                  <strong>{project.name}</strong><small title={project.root}>{project.root}</small>
                </button>
                <span>{session.selectedProjectId === project.id ? "Selected" : ""}</span>
                <button class="text-button danger" aria-label={`Remove ${project.name}`} disabled={session.setupBusy !== null} onclick={(event) => openRemoval(project.id, event.currentTarget)}>Remove…</button>
              </div>
            {/each}
          </div>
          <button bind:this={chooseFolderButton} class="secondary-button choose-folder" disabled={session.setupBusy !== null} onclick={chooseProjectFolder}>Choose Folder…</button>
          <details class="expert-path">
            <summary>Enter a folder path</summary>
            <form class="path-form" onsubmit={(event) => { event.preventDefault(); session.previewProjectPath(); }}>
              <label for="project-path">Project folder</label>
              <div class="field-action"><input id="project-path" bind:value={session.projectPath} maxlength="4096" spellcheck="false" /><button class="secondary-button" disabled={!session.projectPath.trim() || session.setupBusy !== null}>Review folder</button></div>
            </form>
          </details>
          {#if session.projectPreview}
            <div class="project-preview" class:warning={session.projectPreview.previewId === null}>
              <div><strong>{session.projectPreview.previewId ? "Ready to add" : "Choose another folder"}</strong><p class="path">{session.projectPreview.root}</p><p>{session.projectPreview.detail}</p></div>
              {#if session.projectPreview.previewId}<button class="primary-button" disabled={session.setupBusy !== null} onclick={() => session.registerPreviewedProject()}>Add Project</button>{/if}
            </div>
          {/if}
        </div>
      </section>
      <section class="onboarding-step" aria-labelledby="accounts-heading">
        <span class="step-number" aria-hidden="true">02</span>
        <div>
          <h2 id="accounts-heading">Choose who to work with</h2>
          <p class="step-description">Use Codex or Claude Code with the login on this computer. Your sign-in stays with that Harness.</p>
          {#if accountsIssue}<p class="section-error">{accountsIssue.error.message}</p>{/if}
          <div class="harness-options">
            {#each setup.capabilities.crafts as craft (craft.id)}
              <label class:chosen={session.selectedCraftId === craft.id}>
                <input type="radio" name="harness" value={craft.id} checked={session.selectedCraftId === craft.id} onchange={() => session.chooseCraft(craft.id)} />
                <span><strong>{craft.harnesses[0] === "codex" ? "Codex" : craft.harnesses[0] === "claude-code" ? "Claude Code" : craft.harnesses[0] ?? craft.id}</strong><small>Installed on this computer</small></span>
              </label>
            {:else}
              <p class="step-description">No Harness is available yet. Open Agents settings to install its Jet Craft adapter.</p>
            {/each}
          </div>
          <div class="provider-actions">
            {#each setup.capabilities.authProviders as option (option.provider)}
              {#if !setup.accounts.some((account) => account.provider === option.provider)}
                <button class="secondary-button" disabled={session.setupBusy !== null} onclick={() => session.connectHarness(option.provider)}>Use {option.harness} login</button>
              {/if}
            {/each}
            {#if accountsTarget}<button class="text-button" onclick={() => void session.openSettings(accountsTarget)}>Manage Harnesses and accounts</button>{/if}
          </div>
        </div>
      </section>
      {#if session.setupNotice}<p class="setup-notice" role="status">{session.setupNotice}</p>{/if}
      <div class="setup-continue">
        <span>{session.canStartTask ? "Ready when you are." : "Choose a Project and an available Harness to continue."}</span>
        <button class="primary-button" disabled={!session.canStartTask} onclick={() => session.select("new-task")}>Start a task <span aria-hidden="true">→</span></button>
      </div>
      <details class="setup-advanced">
        <summary>Other computers and connection details</summary>
        <section aria-label="Jet service on this computer">

        <div class="setup-service-line"><span id="service-heading">This computer</span><span class:warning={serviceNeedsAttention} class:success={!serviceNeedsAttention} class="status-text" role="status" aria-label="Connection status">{serviceNeedsAttention ? "Needs attention" : "Connected"}</span></div>
        <p class="step-description">{setup.plane.platform} · Jet core {setup.plane.coreVersion}</p>
        {#if capabilitiesIssue}<p class="service-warning">{capabilitiesIssue.error.message}</p>{/if}
        {#if setup.capabilities.degraded.length}<p class="service-warning">{setup.capabilities.degraded.join(" · ")}</p>{/if}
        {#if serviceNotice}<p class="service-notice" role="status">{serviceNotice}</p>{/if}
        {#if pairingIssue}<p class="section-error">{pairingIssue.error.message}</p>{/if}
        <p class="step-description">A Plane is a computer running Jet. Pair another computer when you want to use it from here.</p>
        <button class="secondary-button" onclick={() => session.openAddPlane()}>Add a Plane…</button>
        </section>
      </details>
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
