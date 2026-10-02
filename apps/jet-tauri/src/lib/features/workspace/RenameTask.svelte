<script lang="ts">
  import { tick } from "svelte";
  import type { DesktopSession } from "$lib/features/shell/session.svelte";
  let { session }: { session: DesktopSession } = $props();
  let dialog = $state<HTMLDialogElement>();
  let input = $state<HTMLInputElement>();
  let name = $state("");
  let attempt = crypto.randomUUID();
  let revision = $state("");
  let returnFocus: HTMLElement | null = null;
  const valid = $derived(name.trim().length > 0 && new TextEncoder().encode(name).length <= 256);
  async function open(event: MouseEvent) {
    returnFocus = event.currentTarget as HTMLElement;
    name = session.selectedConversationTitle;
    revision = session.conversationDetail?.conversation.revision ?? "";
    attempt = crypto.randomUUID();
    session.actionNotice = null;
    dialog?.showModal();
    await tick();
    input?.focus(); input?.select();
  }
  function close() { dialog?.close(); returnFocus?.focus(); }
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (valid && await session.renameTask(name, attempt, revision)) close();
  }
</script>
<button onclick={open} disabled={!session.conversationDetail?.conversation.revision || !session.selectedPlaneOnline}>Rename task…</button>
<dialog class="task-name-dialog" bind:this={dialog} aria-labelledby="rename-task-title" oncancel={(event) => { event.preventDefault(); if (!session.conversationBusy) close(); }}>
  <form onsubmit={save}>
    <h2 id="rename-task-title">Rename task</h2>
    <label for="task-name">Name</label>
    <input id="task-name" bind:this={input} bind:value={name} oninput={() => { attempt = crypto.randomUUID(); }} disabled={session.conversationBusy} maxlength="256" autocomplete="off" />
    {#if session.actionNotice}<p role="alert">{session.actionNotice}</p>{/if}
    {#if session.conversationDetail?.conversation.revision && session.conversationDetail.conversation.revision !== revision}
      <p>Current name: {session.selectedConversationTitle}</p>
      <button type="button" disabled={session.conversationBusy} onclick={() => {
        revision = session.conversationDetail?.conversation.revision ?? revision;
        attempt = crypto.randomUUID();
        session.actionNotice = null;
      }}>Use latest version</button>
    {/if}
    <div class="dialog-actions">
      <button type="button" onclick={close} disabled={session.conversationBusy}>Cancel</button>
      <button class="primary" type="submit" disabled={!valid || session.conversationBusy}>{session.conversationBusy ? "Saving…" : "Save"}</button>
    </div>
  </form>
</dialog>
<style>
  dialog { width: 400px; padding: 24px; border: 1px solid var(--border); border-radius: 8px; background: var(--raised); color: var(--text); }
  form { display: grid; gap: 12px; }
  h2 { margin: 0 0 8px; font-size: 17px; }
  p { margin: 0; color: var(--danger-text); }
  .dialog-actions { display: flex; justify-content: flex-end; gap: 8px; padding-top: 12px; }
</style>
