<script lang="ts">
  import { onMount } from "svelte";

  import NotificationSettings from "$lib/features/notifications/NotificationSettings.svelte";
  import { publicError } from "$lib/jet/errors";
  import { loadDesktopPreferences, setDesktopPreferences } from "$lib/jet/preferences";

  type Restoration =
    | { kind: "loading" }
    | { kind: "ready"; reopenLastTask: boolean; saving: boolean; notice: string | null }
    | { kind: "failed"; message: string; code: string };

  let restoration = $state<Restoration>({ kind: "loading" });
  let mounted = false;
  let request = 0;

  async function load() {
    const current = ++request;
    restoration = { kind: "loading" };
    try {
      const preferences = await loadDesktopPreferences();
      if (!mounted || current !== request) return;
      restoration = { kind: "ready", reopenLastTask: preferences.reopenLastTask, saving: false, notice: null };
    } catch (error: unknown) {
      if (!mounted || current !== request) return;
      const failure = publicError(error);
      restoration = { kind: "failed", message: failure.message, code: failure.code };
    }
  }

  async function setReopen(reopenLastTask: boolean) {
    if (restoration.kind !== "ready" || restoration.saving) return;
    const previous = restoration.reopenLastTask;
    const current = ++request;
    restoration = { kind: "ready", reopenLastTask, saving: true, notice: null };
    try {
      const saved = await setDesktopPreferences({ reopenLastTask });
      if (!mounted || current !== request) return;
      restoration = { kind: "ready", reopenLastTask: saved.reopenLastTask, saving: false, notice: "Saved on this computer." };
    } catch (error: unknown) {
      if (!mounted || current !== request) return;
      restoration = { kind: "ready", reopenLastTask: previous, saving: false, notice: publicError(error).message };
    }
  }

  onMount(() => {
    mounted = true;
    void load();
    return () => {
      mounted = false;
    };
  });
</script>

<section class="settings-section" aria-labelledby="section-appearance">
  <h2 id="section-appearance" tabindex="-1">Appearance</h2>
  <p>Jet follows your system's light or dark setting.</p>
</section>

<NotificationSettings />

<section class="settings-section" aria-labelledby="section-restoration">
  <h2 id="section-restoration" tabindex="-1">Restoration</h2>
  {#if restoration.kind === "loading"}
    <span class="loading-bar short" aria-hidden="true"></span>
  {:else if restoration.kind === "failed"}
    <div class="notice critical" role="status">
      <p>{restoration.message} <code>{restoration.code}</code></p>
      <button class="text-button" onclick={() => void load()}>Try again</button>
    </div>
  {:else}
    <label class="toggle">
      <input
        type="checkbox"
        checked={restoration.reopenLastTask}
        disabled={restoration.saving}
        onchange={(event) => void setReopen(event.currentTarget.checked)}
      />
      Reopen the last task when Jet starts
    </label>
    <p>This choice is kept on this computer only. Your tasks stay on their Planes either way.</p>
    {#if restoration.notice}<p role="status">{restoration.notice}</p>{/if}
  {/if}
  <h3>Launch at login</h3>
  <p>Starting Jet at login isn't available on Linux yet.</p>
</section>
