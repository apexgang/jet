<script lang="ts">
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  import { currentPlatform } from "$lib/features/shell/shortcuts";
  let { session, starting = false }: { session: DesktopSession; starting?: boolean } = $props();
  let composer = $state<HTMLTextAreaElement>();
  const shortcut = currentPlatform() === "mac" ? "⌘ Enter" : "Ctrl Enter";
  const ready = $derived(session.canSubmitDraft && !session.conversationBusy && session.selectedPlaneOnline && (!starting || session.canStartTask));
  $effect(() => { if (session.composerFocusRequest > 0) queueMicrotask(() => composer?.focus()); });
  function send(event: KeyboardEvent) {
    if (event.isComposing) return;
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      if (ready) void session.submitDraft();
    }
  }
</script>
<div class="compose" class:starting>
  {#if session.actionNotice}<p class="compose-notice" role="status">{session.actionNotice}</p>{/if}
  {#if session.queueIsFull}<p class="compose-warning" role="alert">There are too many messages waiting. Remove one from the queue or wait for work to finish.</p>{/if}
  <div class="writing-field">
    <textarea bind:this={composer} bind:value={session.draft} rows={starting ? 4 : 2} maxlength="65536" aria-label="Task message"
      placeholder={starting ? "What would you like to work on?" : "Reply or describe the next step…"} onkeydown={send}></textarea>
    <div class="writing-actions">
      {#if starting}
        <label class="context-choice"><span>Project</span>
          <select aria-label="Project for new task" value={session.selectedProjectId ?? ""} disabled={session.conversationBusy} onchange={(event) => { session.selectedProjectId = event.currentTarget.value; }}>
            {#if !session.selectedProjectId}<option value="">Choose a Project</option>{/if}
            {#each session.setupSnapshot?.projects ?? [] as project (project.id)}<option value={project.id}>{project.name}</option>{/each}
          </select>
        </label>
        <label class="context-choice"><span>Use</span>
          <select aria-label="Harness for new task" value={session.selectedCraftId ?? ""} disabled={session.conversationBusy} onchange={(event) => session.chooseCraft(event.currentTarget.value)}>
            {#if !session.selectedCraftId}<option value="">Choose a Harness</option>{/if}
            {#each session.setupSnapshot?.capabilities.crafts ?? [] as craft (craft.id)}
              <option value={craft.id}>{craft.harnesses[0] === "codex" ? "Codex" : craft.harnesses[0] === "claude-code" ? "Claude Code" : craft.harnesses[0] ?? craft.id}</option>
            {/each}
          </select>
        </label>
      {:else}<span class="send-hint">{session.hasLiveRun ? "Messages are sent in order" : "Continue this task"}</span>{/if}
      <button class="send-button" disabled={!ready} onclick={() => session.submitDraft()} title={`Send · ${shortcut}`}>
        {session.conversationBusy ? "Sending…" : starting ? "Start task" : "Send"}<span aria-hidden="true">↑</span>
      </button>
    </div>
  </div>
  <div class="compose-footnote">
    <span>{starting ? `Separate working copy · ${session.runsOnLabel}` : `Runs on ${session.runsOnLabel}`}</span>
    {#if session.draftBytes > session.maximumPromptBytes * 0.8}
      <span class:over-limit={session.draftBytes > session.maximumPromptBytes}>{session.draftBytes.toLocaleString()} / {session.maximumPromptBytes.toLocaleString()} bytes</span>
    {:else}<kbd>{shortcut} to send</kbd>{/if}
  </div>
</div>
