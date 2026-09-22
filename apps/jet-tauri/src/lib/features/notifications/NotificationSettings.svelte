<script lang="ts">
  import { publicError } from "$lib/jet/errors";
  import { onMount } from "svelte";
  import { loadNotificationSettings, setNotificationSettings, type NotificationPreferences } from "$lib/jet/notifications";
  let preferences = $state<NotificationPreferences>({ enabled: false, approvals: true, completion: true, failure: true });
  let busy = $state(true);
  let loaded = $state(false);
  let notice = $state<string | null>(null);
  let mounted = false;
  onMount(() => {
    mounted = true;
    void loadNotificationSettings().then((settings) => {
      if (!mounted) return;
      preferences = settings.preferences;
      notice = settings.error;
      loaded = true;
    }).catch(() => { if (mounted) notice = "Could not load notification settings. Reopen Settings to try again."; })
      .finally(() => { if (mounted) busy = false; });
    return () => { mounted = false; };
  });
  async function save() {
    if (busy || !loaded) return;
    busy = true;
    try {
      const result = await setNotificationSettings({ ...preferences });
      if (!mounted) return;
      preferences = result.preferences;
      notice = result.error ?? "Notification preferences saved on this device.";
    } catch (error: unknown) {
      if (mounted) notice = publicError(error).message;
    } finally { if (mounted) busy = false; }
  }
</script>
<section class="notification-settings" aria-label="Desktop notifications">
  <h1>Notifications</h1>
  <p>Choose what Jet sends to this desktop while the app is open. Notifications use generic text without task content.</p>
  <form onsubmit={(event) => { event.preventDefault(); void save(); }}>
    <fieldset disabled={busy || !loaded}>
      <legend>On this device</legend>
      <label><input type="checkbox" bind:checked={preferences.enabled} /> Enable desktop notifications</label>
      <label><input type="checkbox" bind:checked={preferences.approvals} /> Approval needed</label>
      <label><input type="checkbox" bind:checked={preferences.completion} /> Run completed</label>
      <label><input type="checkbox" bind:checked={preferences.failure} /> Run failed or lost</label>
      <button type="submit">{busy ? "Saving…" : "Save preferences"}</button>
    </fieldset>
  </form>
  <p>Enabling notifications needs a connected Plane. You can turn them off while offline. Earlier events are not replayed as notifications. Your desktop's notification and Do Not Disturb settings still apply.</p>
  {#if notice}<p role="status">{notice}</p>{/if}
</section>
<style>
  .notification-settings { padding: 32px; overflow: auto; max-width: 720px; line-height: 1.6; }
  h1 { font-size: 24px; }
  p { color: var(--muted); }
  fieldset { display: grid; gap: 18px; border: 1px solid var(--border); border-radius: 8px; padding: 20px; }
  legend { padding: 0 8px; }
  label { display: flex; align-items: center; gap: 10px; }
  input { accent-color: var(--accent); width: 18px; height: 18px; }
  button { justify-self: start; color: var(--text); background: var(--raised); border: 1px solid var(--border); border-radius: 5px; padding: 10px 14px; cursor: pointer; }
  button:hover:not(:disabled) { background: var(--hover); }
  button:disabled { opacity: .5; cursor: default; }
</style>
