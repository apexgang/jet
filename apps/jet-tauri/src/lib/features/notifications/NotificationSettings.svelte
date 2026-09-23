<script lang="ts">
  import { publicError } from "$lib/jet/errors";
  import { onMount } from "svelte";
  import {
    loadNotificationSettings,
    setNotificationSettings,
    type NotificationPlane,
    type NotificationPreferences,
  } from "$lib/jet/notifications";
  let preferences = $state<NotificationPreferences>({ enabled: false, approvals: true, completion: true, failure: true, mutedPlanes: [] });
  let planes = $state<NotificationPlane[]>([]);
  let busy = $state(true);
  let loaded = $state(false);
  let notice = $state<string | null>(null);
  let mounted = false;
  onMount(() => {
    mounted = true;
    void loadNotificationSettings().then((settings) => {
      if (!mounted) return;
      preferences = settings.preferences;
      planes = settings.planes;
      notice = settings.error;
      loaded = true;
    }).catch(() => { if (mounted) notice = "Could not load notification settings. Reopen Settings to try again."; })
      .finally(() => { if (mounted) busy = false; });
    return () => { mounted = false; };
  });
  function setRouted(planeId: string, routed: boolean) {
    const others = preferences.mutedPlanes.filter((muted) => muted !== planeId);
    preferences.mutedPlanes = routed ? others : [...others, planeId];
  }
  async function save() {
    if (busy || !loaded) return;
    busy = true;
    try {
      const result = await setNotificationSettings({ ...preferences, mutedPlanes: [...preferences.mutedPlanes] });
      if (!mounted) return;
      preferences = result.preferences;
      planes = result.planes;
      notice = result.error ?? "Notification preferences saved on this device.";
    } catch (error: unknown) {
      if (mounted) notice = publicError(error).message;
    } finally { if (mounted) busy = false; }
  }
</script>
<section class="notification-settings settings-section" aria-labelledby="section-notifications">
  <h2 id="section-notifications" tabindex="-1">Notifications</h2>
  <p>Choose what Jet sends to this desktop while the app is open. Notifications use generic text without task content.</p>
  <form onsubmit={(event) => { event.preventDefault(); void save(); }}>
    <fieldset disabled={busy || !loaded}>
      <legend>On this device</legend>
      <label><input type="checkbox" bind:checked={preferences.enabled} /> Enable desktop notifications</label>
      <label><input type="checkbox" bind:checked={preferences.approvals} /> Approval needed</label>
      <label><input type="checkbox" bind:checked={preferences.completion} /> Run completed</label>
      <label><input type="checkbox" bind:checked={preferences.failure} /> Run failed or lost</label>
      {#if planes.length > 1}
        <fieldset class="routing" aria-describedby="notification-routing-help">
          <legend>Notify me about</legend>
          <p id="notification-routing-help">Turn a Plane off to keep its notifications off this computer. Its tasks keep running.</p>
          {#each planes as plane (plane.planeId)}
            <label>
              <input
                type="checkbox"
                checked={!preferences.mutedPlanes.includes(plane.planeId)}
                onchange={(event) => setRouted(plane.planeId, event.currentTarget.checked)}
              />
              {plane.label}
            </label>
          {/each}
        </fieldset>
      {/if}
      <button type="submit">{busy ? "Saving…" : "Save preferences"}</button>
    </fieldset>
  </form>
  <p>Enabling notifications needs a connected Plane. You can turn them off while offline. Earlier events are not replayed as notifications, including events from a Plane you turn back on. Your desktop's notification and Do Not Disturb settings still apply.</p>
  <p>Jet for Linux doesn't play sound cues yet.</p>
  {#if notice}<p role="status">{notice}</p>{/if}
</section>
<style>
  .notification-settings { line-height: 1.6; }
  p { color: var(--muted); }
  fieldset { display: grid; gap: 18px; border: 1px solid var(--border); border-radius: 8px; padding: 20px; }
  fieldset.routing { gap: 12px; padding: 14px 16px; }
  .routing p { margin: 0; }
  legend { padding: 0 8px; }
  label { display: flex; align-items: center; gap: 10px; overflow-wrap: anywhere; }
  input { accent-color: var(--accent); width: 18px; height: 18px; flex: none; }
  button { justify-self: start; color: var(--text); background: var(--raised); border: 1px solid var(--border); border-radius: 5px; padding: 10px 14px; cursor: pointer; }
  button:hover:not(:disabled) { background: var(--hover); }
  button:disabled { opacity: .5; cursor: default; }
</style>
