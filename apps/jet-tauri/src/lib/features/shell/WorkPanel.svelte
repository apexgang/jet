<script lang="ts">
  import DeliveryPanel from "$lib/features/delivery/DeliveryPanel.svelte";
  import type { PublicRecoveryAction } from "$lib/jet/bridge";
  import type { DesktopSession, WorkPanelTab } from "./session.svelte";
  import { LOST_RUN_TEXT, needsRunRecovery } from "$lib/features/system/model";
  import { currentPlatform, shortcutAria } from "./shortcuts";

  let {
    session,
    mode = "column",
  }: {
    session: DesktopSession;
    /**
     * A column beside the conversation, or the compact overlay (a modal
     * dialog over it). Only the root's attributes change: the panel is
     * never remounted, so its tab, file, draft and terminal survive.
     */
    mode?: "column" | "overlay";
  } = $props();
  const platform = currentPlatform();
  const tabButtons: Record<string, HTMLButtonElement | undefined> = $state({});
  let runControlCancel = $state<HTMLButtonElement>();

  /**
   * What the panel body shows. Only Deliver and the Run views are real tab
   * panels; a tab points at its panel (`aria-controls`) only while it exists.
   */
  const body = $derived.by(() => {
    if (session.selectedWorkPanel === "delivery") return "delivery";
    if (session.workPanelError && session.selectedWorkPanel !== "run") return "error";
    if (session.workPanelBusy && !session.workPanel && session.selectedWorkPanel !== "run") return "loading";
    if (!session.selectedRun) return "no-run";
    return "views";
  });

  function panelExists(tab: WorkPanelTab): boolean {
    return tab === "delivery" || body === "views";
  }

  /** APG tabs with automatic activation: arrows wrap, Home and End jump. */
  function handleTabKey(event: KeyboardEvent, index: number) {
    let next: number;
    switch (event.key) {
      case "ArrowRight":
        next = (index + 1) % tabs.length;
        break;
      case "ArrowLeft":
        next = (index - 1 + tabs.length) % tabs.length;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = tabs.length - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    const tab = tabs[next].id;
    session.showPanel(tab, "tab");
    tabButtons[tab]?.focus();
  }

  // The overlay opened, or a Run-control confirmation closed: the selected tab takes focus.
  $effect(() => {
    if (session.workPanelPresented && session.takeFocusRequest("work-panel")) {
      tabButtons[session.selectedWorkPanel]?.focus();
    }
  });

  // The Interrupt Turn / Stop Run confirmation opened: its Cancel takes focus.
  $effect(() => {
    if (runControlCancel && session.takeFocusRequest("run-control")) runControlCancel.focus();
  });

  const tabs: Array<{ id: WorkPanelTab; label: string }> = [
    { id: "changes", label: "Changes" },
    { id: "files", label: "Files" },
    { id: "terminal", label: "Terminal" },
    { id: "run", label: "Run" },
    { id: "delivery", label: "Deliver" },
  ];

  function formatBytes(value: string | null): string {
    if (value === null) return "—";
    const bytes = Number(value);
    if (!Number.isFinite(bytes)) return value;
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  function artifactMessage(availability: string): string {
    switch (availability) {
      case "disk_pressure": return "The full patch was not retained because storage is constrained.";
      case "run_budget_exceeded": return "The Run reached its retained-artifact budget.";
      case "artifact_size_exceeded": return "The complete patch exceeds the artifact size limit.";
      default: return "The complete patch can be loaded in verified chunks.";
    }
  }

  function handleTerminalKey(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      void session.sendTerminalLine();
    }
  }

  function recoveryLabel(action: PublicRecoveryAction): string {
    switch (action.type) {
      case "refresh_file": return "Reload File";
      case "refresh_conversation": return "Refresh Task";
      case "refresh_run": return "Refresh Run";
      case "resume_events": return "Reconnect Activity";
    }
  }

  function observeTerminalSize(node: HTMLElement) {
    const measure = () => {
      const style = getComputedStyle(node);
      const fontSize = Number.parseFloat(style.fontSize) || 12;
      const lineHeight = Number.parseFloat(style.lineHeight) || fontSize * 1.4;
      session.setTerminalGeometry(
        Math.max(1, Math.floor(node.clientHeight / lineHeight)),
        Math.max(1, Math.floor(node.clientWidth / (fontSize * 0.62))),
      );
    };
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    measure();
    return { destroy: () => observer.disconnect() };
  }
</script>

<div
  class="work-panel"
  class:hidden={!session.workPanelPresented}
  class:overlay={mode === "overlay"}
  role={mode === "overlay" ? "dialog" : "complementary"}
  aria-modal={mode === "overlay" ? "true" : undefined}
  aria-label="Work panel"
>
  <div class="panel-navigation">
    <div class="panel-title"><strong>Work details</strong><button class="icon-button panel-close" aria-label="Hide work panel" onclick={() => session.hideWorkPanel()}>Close</button></div>
  <div class="panel-tabs" role="tablist" aria-label="Work panel views">
    {#each tabs as tab, index (tab.id)}
      <button
        bind:this={tabButtons[tab.id]}
        id={`work-tab-${tab.id}`}
        role="tab"
        tabindex={session.selectedWorkPanel === tab.id ? 0 : -1}
        aria-controls={panelExists(tab.id) ? `work-${tab.id}` : undefined}
        aria-selected={session.selectedWorkPanel === tab.id}
        aria-keyshortcuts={shortcutAria("work-panel-tab", platform, tab.id)}
        class:active={session.selectedWorkPanel === tab.id}
        onclick={() => session.showPanel(tab.id, "tab")}
        onkeydown={(event) => handleTabKey(event, index)}
      >
        {tab.label}
      </button>
    {/each}
  </div>
  </div>

  <div class="panel-content" aria-busy={session.workPanelBusy}>
    <div id="work-delivery" class="work-view" class:active={session.selectedWorkPanel === "delivery"} role="tabpanel" aria-labelledby="work-tab-delivery">
      <DeliveryPanel {session} />
    </div>
    {#if body !== "delivery"}
    {#if body === "error" && session.workPanelError}
      <div class="work-alert" role="alert">
        <strong>Work details unavailable</strong>
        <p>{session.workPanelError.message}</p>
        <div class="recovery-actions">
          {#each session.workPanelError.recoveryActions as action}
            <button onclick={() => session.applyWorkRecovery(action, session.workPanelError?.planeId ?? null)}>{recoveryLabel(action)}</button>
          {/each}
          {#if session.pairAgainTarget(session.workPanelError)}
            {@const target = session.pairAgainTarget(session.workPanelError)}
            <button onclick={() => target && session.pairAgain(target)}>Pair again</button>
          {/if}
          {#if session.workPanelError.recoveryActions.length === 0 && session.workPanelError.retryable}
            <button onclick={() => session.refreshWorkPanel()}>Try Again</button>
          {/if}
        </div>
      </div>
    {:else if body === "loading"}
      <div class="panel-empty" role="status">
        <h2>Loading work details</h2>
        <p>Getting the latest files and activity.</p>
      </div>
    {:else if body === "no-run"}
      <div class="panel-empty">
        <h2>Work details will appear here</h2>
        <p>Start a task to inspect its changes, files, terminals, and recovery state.</p>
      </div>
    {:else}
      <div id="work-changes" class="work-view" class:active={session.selectedWorkPanel === "changes"} role="tabpanel" aria-labelledby="work-tab-changes">
        <div class="work-heading">
          <div>
            <h2>Changed files</h2>
            <p>{session.workPanel?.scope ?? "Current"} checkpoint · {session.workPanel?.totalFiles ?? 0} files</p>
          </div>
          <button disabled={session.workPanelBusy} onclick={() => session.refreshWorkPanel()}>Refresh</button>
        </div>

        <details class="checkpoint-picker"><summary>Choose a different checkpoint</summary>
        <div class="checkpoint-controls" role="group" aria-label="Change checkpoint">
          <label>
            Checkpoint
            <select bind:value={session.checkpointKind}>
              <option value="current">Current</option>
              <option value="final" disabled={session.selectedRun?.lifecycle === "active" || session.selectedRun?.lifecycle === "starting" || session.selectedRun?.lifecycle === "stopping"}>Final</option>
              <option value="turn">Turn</option>
              <option value="historical">Historical range</option>
            </select>
          </label>
          {#if session.checkpointKind === "turn"}
            <label>Turn <input type="number" min="1" max={session.workPanel?.latestTurn ?? 1} bind:value={session.checkpointTurn} /></label>
          {:else if session.checkpointKind === "historical"}
            <label>From <input type="number" min="0" max={session.workPanel?.latestTurn ?? 1} bind:value={session.checkpointFromTurn} /></label>
            <label>To <input type="number" min="1" max={session.workPanel?.latestTurn ?? 1} bind:value={session.checkpointToTurn} /></label>
          {/if}
          <button disabled={!session.canApplyWorkCheckpoint} onclick={() => session.applyWorkCheckpoint()}>Apply</button>
        </div>

        </details>
        {#if !session.workPanel || session.workPanel.files.length === 0}
          <div class="work-empty">
            <strong>No changes recorded</strong>
            <p>The selected Run has not produced a file change at this checkpoint.</p>
          </div>
        {:else}
          <ol class="changed-files" aria-label="Changed files">
            {#each session.workPanel.files as file (file.id)}
              <li>
                <button class:selected={session.selectedWorkFileId === file.id} onclick={() => session.selectWorkFile(file.id)} title={`Open ${file.path}`}>
                  <span class={`file-status ${file.status}`}>{file.status.slice(0, 1).toUpperCase()}</span>
                  <span class="file-path">{file.path}</span>
                  <small>{file.origin}</small>
                </button>
              </li>
            {/each}
          </ol>
          {#if session.workPanel.nextPage}
            <button class="load-row" disabled={session.workPanelBusy} onclick={() => session.loadMoreWorkFiles()}>Load more files</button>
          {/if}
        {/if}

        <div class="patch-heading">
          <div>
            <h2>Patch</h2>
            <p>{formatBytes(session.workPanel?.artifact.size ?? null)} retained artifact</p>
          </div>
        </div>

        {#if session.workPanel?.patch}
          {#if session.workPanel.patch.includes("GIT binary patch") || session.workPanel.patch.includes("Binary files")}
            <div class="work-empty compact">
              <strong>Binary change</strong>
              <p>Jet records the file change, but this patch is not readable as text.</p>
            </div>
          {:else}
            <textarea class="patch-view" readonly aria-label="Patch content" value={session.workPanel.patch}></textarea>
          {/if}
        {:else}
          <div class="work-empty compact"><p>No text patch is available.</p></div>
        {/if}

        {#if session.workPanel?.patchTruncated}
          <div class="artifact-state">
            <p>{artifactMessage(session.workPanel.artifact.availability)}</p>
            {#if session.workPanel.artifactReadId}
              <button disabled={session.workPanelBusy} onclick={() => session.loadMorePatch()}>Load next chunk</button>
            {/if}
          </div>
        {/if}
      </div>

      <div id="work-files" class="work-view" class:active={session.selectedWorkPanel === "files"} role="tabpanel" aria-labelledby="work-tab-files">
        <div class="work-heading">
          <div>
            <h2>Workspace file</h2>
            <p>{session.editableFile?.path ?? "Choose a changed file"}</p>
          </div>
        </div>

        {#if !session.selectedWorkFileId}
          <div class="work-empty">
            <strong>No file selected</strong>
            <p>Choose a file in Changes. Only files from this Run can be opened here.</p>
          </div>
        {:else if !session.editableFile}
          <div class="work-empty">
            <strong>File not loaded</strong>
            <p>The file may be binary, oversized, deleted, or no longer available at this revision.</p>
            <button disabled={session.workPanelBusy} onclick={() => session.selectWorkFile(session.selectedWorkFileId!)}>Try Again</button>
          </div>
        {:else if session.editableFile.content === null}
          <div class="work-empty">
            <strong>Content unavailable</strong>
            <p>Jet can show the change record, but this file cannot be edited as bounded UTF-8 text.</p>
          </div>
        {:else}
          <div class="file-meta">
            <span>{session.editableFile.contentBytes.toLocaleString()} bytes</span>
            <span title={session.editableFile.revision}>Revision bound</span>
          </div>
          <textarea class="file-editor" bind:value={session.fileDraft} maxlength="131072" spellcheck="false" aria-label={`Edit ${session.editableFile.path}`}></textarea>
          <div class="file-actions">
            <button disabled={session.workPanelBusy} onclick={() => session.selectWorkFile(session.selectedWorkFileId!)}>Reload</button>
            <button class="primary-action" disabled={session.workPanelBusy || session.fileDraft === session.editableFile.content} onclick={() => session.saveSelectedFile()}>Save Edit</button>
          </div>

          <section class="review-form" aria-labelledby="review-heading">
            <h3 id="review-heading">Review comment</h3>
            <label>Line <input type="number" min="1" max="4294967295" bind:value={session.reviewLine} /></label>
            <textarea rows="3" maxlength="8192" bind:value={session.reviewComment} placeholder="Describe the issue or requested change" aria-label="Review comment"></textarea>
            <button disabled={session.workPanelBusy || session.reviewComment.trim().length === 0 || session.reviewLine < 1} onclick={() => session.submitSelectedFileReview()}>Add Review Comment</button>
          </section>
        {/if}
      </div>

      <div id="work-terminal" class="work-view" class:active={session.selectedWorkPanel === "terminal"} role="tabpanel" aria-labelledby="work-tab-terminal">
        <div class="work-heading">
          <div>
            <h2>Workspace terminal</h2>
            <p>{session.workPanel?.workspaceId ? "Scoped to this managed Workspace" : "Requires a managed Workspace"}</p>
          </div>
          <button disabled={session.workPanelBusy || !session.workPanel?.workspaceId} onclick={() => session.createTerminal()}>New</button>
        </div>

        {#if !session.workPanel?.workspaceId}
          <div class="work-empty"><strong>No managed Workspace</strong><p>This Run does not expose a terminal-capable Workspace.</p></div>
        {:else if session.workPanel.terminals.length === 0}
          <div class="work-empty">
            <strong>No terminal open</strong>
            <p>Create a terminal owned by this Workspace. Jet never launches a host shell from the web view.</p>
            <button disabled={session.workPanelBusy} onclick={() => session.createTerminal()}>New Terminal</button>
          </div>
        {:else}
          <label class="terminal-picker">
            Session
            <select value={session.selectedTerminalId ?? ""} onchange={(event) => session.selectTerminal(event.currentTarget.value)}>
              {#each session.workPanel.terminals as terminal}
                <option value={terminal.id}>{terminal.id.slice(0, 8)} · {terminal.state}</option>
              {/each}
            </select>
          </label>
          <textarea use:observeTerminalSize class="terminal-output" readonly aria-label="Terminal output" value={session.selectedTerminalId ? (session.terminalOutput[session.selectedTerminalId] ?? "Terminal output will appear here.") : "Choose a terminal."}></textarea>
          <div class="terminal-input">
            <input bind:value={session.terminalInput} disabled={!session.attachedTerminalId} onkeydown={handleTerminalKey} autocomplete="off" spellcheck="false" aria-label="Terminal input" placeholder={session.attachedTerminalId ? "Send input to attached terminal" : "Attach to send input"} />
            <button disabled={!session.attachedTerminalId || !session.terminalInput} onclick={() => session.sendTerminalLine()}>Send</button>
          </div>
          <div class="terminal-actions">
            {#if session.attachedTerminalId === session.selectedTerminalId}
              <button onclick={() => session.detachSelectedTerminal()}>Detach</button>
            {:else}
              <button disabled={session.workPanelBusy || !session.selectedTerminalId || session.workPanel.terminals.find((item) => item.id === session.selectedTerminalId)?.state !== "open"} onclick={() => session.attachSelectedTerminal()}>Attach</button>
            {/if}
            <button class="danger-action" disabled={session.workPanelBusy || !session.selectedTerminalId} onclick={() => session.closeSelectedTerminal()}>Close</button>
          </div>
        {/if}
      </div>

      <div id="work-run" class="work-view" class:active={session.selectedWorkPanel === "run"} role="tabpanel" aria-labelledby="work-tab-run">
        <section class="run-summary">
          <div class="work-heading inline-heading">
            <div><h2>Current Run</h2><p>Authoritative lifecycle and recovery state</p></div>
            <button disabled={session.workPanelBusy} onclick={() => session.refreshWorkPanel()}>Refresh</button>
          </div>
          <dl>
            <div><dt>Lifecycle</dt><dd>{session.selectedRun?.lifecycle ?? "Not started"}</dd></div>
            <div><dt>Activity</dt><dd>{session.supervision?.execution?.activity?.replaceAll("_", " ") ?? (session.hasLiveRun ? "Starting" : "Idle")}</dd></div>
            <div><dt>Runs on</dt><dd class="plane-label">{session.runsOnLabel}</dd></div>
            <div><dt>Checkpoint</dt><dd>{session.workPanel?.scope ?? "Unavailable"}</dd></div>
            <div><dt>Latest Turn</dt><dd>{session.workPanel?.latestTurn ?? "—"}</dd></div>
            <div><dt>Changed files</dt><dd>{session.workPanel?.totalFiles ?? 0}</dd></div>
            <div><dt>Cursor</dt><dd>{session.conversationDetail?.cursor ?? session.selectedPlaneCursor ?? "Unknown"}</dd></div>
            <div><dt>Revision</dt><dd>{session.selectedRun?.revision ?? "Unavailable"}</dd></div>
          </dl>

          {#if session.supervision?.execution?.termination}<p class="termination-result" role="status">{session.supervision.execution.termination.summary}</p>{/if}

          {#if needsRunRecovery(session.selectedRun?.lifecycle, session.supervision?.execution?.needsAttention)}
            <section class="notice critical lost-run" aria-labelledby="lost-run-title">
              <strong id="lost-run-title">Recovery needed</strong>
              <p>{LOST_RUN_TEXT}</p>
            </section>
          {/if}

          <div class="run-controls" role="group" aria-label="Run controls">
            <button disabled={!session.canInterruptTurn || session.controlBusy !== null} onclick={(event) => session.requestRunControl("interrupt_turn", event.currentTarget)}>Interrupt Turn…</button>
            <button class="danger-action" disabled={!session.canStopRun || session.controlBusy !== null} onclick={(event) => session.requestRunControl("stop_run", event.currentTarget)}>Stop Run…</button>
          </div>

          {#if session.runControlConfirmation}
            <section class="control-confirmation" aria-labelledby="control-title">
              <h3 id="control-title">{session.runControlConfirmation === "interrupt_turn" ? "Interrupt this Turn?" : "Stop this Run?"}</h3>
              <p>{session.runControlConfirmation === "interrupt_turn" ? "Jet will end the active Turn. The Run stays available for the next queued Turn when native cancellation succeeds." : "Jet will end the whole Run and its native processes. Recorded output and Workspace changes remain available."}</p>
              <div>
                <button bind:this={runControlCancel} onclick={() => session.cancelRunControl()}>Cancel</button>
                <button class:danger-action={session.runControlConfirmation === "stop_run"} class:primary-action={session.runControlConfirmation === "interrupt_turn"} onclick={() => session.confirmRunControl()}>{session.runControlConfirmation === "interrupt_turn" ? "Interrupt Turn" : "Stop Run"}</button>
              </div>
            </section>
          {/if}
        </section>

        <section class="queue-summary">
          <div class="queue-heading"><h2>Turn queue</h2><span>{session.supervision?.turns.length ?? 0} / {session.supervision?.maximumEntries ?? 128}</span></div>
          {#if session.supervisionBusy && !session.supervision}
            <p>Loading the authoritative queue…</p>
          {:else if !session.supervision || session.supervision.turns.length === 0}
            <p>No active or queued Turns.</p>
          {:else}
            <ol class="turn-queue">
              {#each session.supervision.turns as turn (turn.id)}
                <li>
                  <div><strong>{turn.state === "active" ? "Current Turn" : `Position ${turn.position}`}</strong><span>{turn.source.replaceAll("_", " ")} · {turn.target}</span><small>Turn {turn.sequence} · {turn.state.replaceAll("_", " ")}</small></div>
                  {#if turn.withdrawable}<button disabled={session.controlBusy !== null} onclick={() => session.withdrawQueuedTurn(turn)}>Withdraw</button>{/if}
                </li>
              {/each}
            </ol>
          {/if}
          <p class="queue-limit">Each prompt can contain up to {(session.supervision?.maximumPromptBytes ?? 65536).toLocaleString()} UTF-8 bytes.</p>
        </section>
      </div>
    {/if}

    {#if session.workPanelNotice}<p class="work-notice" role="status">{session.workPanelNotice}</p>{/if}
    {#if session.workPanelNoticeError && (session.workPanelNoticeError.recoveryActions.length > 0 || session.pairAgainTarget(session.workPanelNoticeError))}
      <div class="recovery-actions panel-recovery" role="group" aria-label="Recovery actions">
        {#each session.workPanelNoticeError.recoveryActions as action}
          <button onclick={() => session.applyWorkRecovery(action, session.workPanelNoticeError?.planeId ?? null)}>{recoveryLabel(action)}</button>
        {/each}
        {#if session.pairAgainTarget(session.workPanelNoticeError)}
          {@const target = session.pairAgainTarget(session.workPanelNoticeError)}
          <button onclick={() => target && session.pairAgain(target)}>Pair again</button>
        {/if}
        {#if session.workPanelNoticeError.revisionConflict}
          <span>Current revision {session.workPanelNoticeError.revisionConflict.currentRevision}</span>
        {/if}
      </div>
    {/if}
    {/if}
  </div>
</div>
