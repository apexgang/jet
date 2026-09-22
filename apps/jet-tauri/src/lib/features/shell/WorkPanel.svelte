<script lang="ts">
  import type { DesktopSession, WorkPanelTab } from "./session.svelte";

  let { session }: { session: DesktopSession } = $props();

  const tabs: Array<{ id: WorkPanelTab; label: string }> = [
    { id: "changes", label: "Changes" },
    { id: "files", label: "Files" },
    { id: "terminal", label: "Terminal" },
    { id: "run", label: "Run" },
  ];

  const emptyCopy: Record<Exclude<WorkPanelTab, "run">, { title: string; message: string }> = {
    changes: {
      title: "No change details yet",
      message: "Changes will load here when the Run exposes a diff.",
    },
    files: {
      title: "No file selected",
      message: "Choose a file from a Run to inspect it without leaving the task.",
    },
    terminal: {
      title: "No terminal open",
      message: "Workspace terminals will appear here when the Plane provides one.",
    },
  };
</script>

<aside class="work-panel" class:hidden={!session.workPanelPresented} aria-label="Work panel">
  <div class="panel-tabs" role="tablist" aria-label="Work panel views">
    {#each tabs as tab}
      <button
        role="tab"
        aria-selected={session.selectedWorkPanel === tab.id}
        class:active={session.selectedWorkPanel === tab.id}
        onclick={() => session.showPanel(tab.id)}
      >
        {tab.label}
      </button>
    {/each}
  </div>

  <div class="panel-content">
    {#if session.selectedWorkPanel === "run"}
      <section class="run-summary">
        <h2>Current Run</h2>
        <dl>
          <div><dt>Lifecycle</dt><dd>{session.selectedRun?.lifecycle ?? "Not started"}</dd></div>
          <div><dt>Activity</dt><dd>{session.supervision?.execution?.activity?.replaceAll("_", " ") ?? (session.hasLiveRun ? "Starting" : "Idle")}</dd></div>
          <div><dt>Runs on</dt><dd>{session.scenario.plane.name}</dd></div>
          <div><dt>Cursor</dt><dd>{session.conversationDetail?.cursor ?? session.conversationCursor}</dd></div>
          <div><dt>Revision</dt><dd>{session.selectedRun?.revision ?? "Unavailable"}</dd></div>
        </dl>

        {#if session.supervision?.execution?.termination}
          <p class="termination-result" role="status">{session.supervision.execution.termination.summary}</p>
        {/if}

        <div class="run-controls" aria-label="Run controls">
          <button
            disabled={!session.canInterruptTurn || session.controlBusy !== null}
            onclick={() => session.requestRunControl("interrupt_turn")}
          >Interrupt Turn…</button>
          <button
            class="danger-action"
            disabled={!session.canStopRun || session.controlBusy !== null}
            onclick={() => session.requestRunControl("stop_run")}
          >Stop Run…</button>
        </div>

        {#if session.runControlConfirmation}
          <section class="control-confirmation" aria-labelledby="control-title">
            <h3 id="control-title">
              {session.runControlConfirmation === "interrupt_turn" ? "Interrupt this Turn?" : "Stop this Run?"}
            </h3>
            <p>
              {session.runControlConfirmation === "interrupt_turn"
                ? "Jet will end the active Turn. The Run stays available for the next queued Turn when native cancellation succeeds."
                : "Jet will end the whole Run and its native processes. Recorded output and Workspace changes remain available."}
            </p>
            <div>
              <button onclick={() => session.cancelRunControl()}>Cancel</button>
              <button
                class:danger-action={session.runControlConfirmation === "stop_run"}
                class:primary-action={session.runControlConfirmation === "interrupt_turn"}
                onclick={() => session.confirmRunControl()}
              >{session.runControlConfirmation === "interrupt_turn" ? "Interrupt Turn" : "Stop Run"}</button>
            </div>
          </section>
        {/if}
      </section>

      <section class="queue-summary">
        <div class="queue-heading">
          <h2>Turn queue</h2>
          <span>{session.supervision?.turns.length ?? 0} / {session.supervision?.maximumEntries ?? 128}</span>
        </div>
        {#if session.supervisionBusy && !session.supervision}
          <p>Loading the authoritative queue…</p>
        {:else if !session.supervision || session.supervision.turns.length === 0}
          <p>No active or queued Turns.</p>
        {:else}
          <ol class="turn-queue">
            {#each session.supervision.turns as turn (turn.id)}
              <li>
                <div>
                  <strong>{turn.state === "active" ? "Current Turn" : `Position ${turn.position}`}</strong>
                  <span>{turn.source.replaceAll("_", " ")} · {turn.target}</span>
                  <small>Turn {turn.sequence} · {turn.state.replaceAll("_", " ")}</small>
                </div>
                {#if turn.withdrawable}
                  <button
                    disabled={session.controlBusy !== null}
                    onclick={() => session.withdrawQueuedTurn(turn)}
                  >Withdraw</button>
                {/if}
              </li>
            {/each}
          </ol>
        {/if}
        <p class="queue-limit">Each prompt can contain up to {(session.supervision?.maximumPromptBytes ?? 65536).toLocaleString()} UTF-8 bytes.</p>
      </section>
    {:else}
      <div class="panel-empty">
        <h2>{emptyCopy[session.selectedWorkPanel].title}</h2>
        <p>{emptyCopy[session.selectedWorkPanel].message}</p>
      </div>
    {/if}
  </div>
</aside>
