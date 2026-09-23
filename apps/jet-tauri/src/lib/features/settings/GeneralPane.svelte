<script lang="ts">
  import { onMount } from "svelte";

  import NotificationSettings from "$lib/features/notifications/NotificationSettings.svelte";
  import { publicError } from "$lib/jet/errors";
  import { loadDesktopPreferences, setDesktopPreferences } from "$lib/jet/preferences";
  import {
    loadShellPresentation,
    presentationErrorCopy,
    type PresentationIssue,
  } from "$lib/jet/presentation";
  import {
    SHORTCUTS,
    WORK_PANEL_TAB_ORDER,
    currentPlatform,
    shortcutLabel,
    type ShellIntentKind,
  } from "$lib/features/shell/shortcuts";

  type Restoration =
    | { kind: "loading" }
    | { kind: "ready"; reopenLastTask: boolean; saving: boolean; notice: string | null }
    | { kind: "failed"; message: string; code: string };

  /** The main window's saved layout, read natively: this window can't see its session. */
  type WindowLayout =
    | { kind: "loading" }
    | { kind: "ready"; issue: PresentationIssue | null }
    | { kind: "failed" };

  let restoration = $state<Restoration>({ kind: "loading" });
  let windowLayout = $state<WindowLayout>({ kind: "loading" });
  let layoutRequest = 0;

  const platform = currentPlatform();

  /** The binding as written in the list; the tab shortcuts are one range. */
  function binding(intent: ShellIntentKind): string {
    if (intent !== "work-panel-tab") return shortcutLabel(intent, platform);
    const first = shortcutLabel(intent, platform, WORK_PANEL_TAB_ORDER[0]);
    const last = shortcutLabel(intent, platform, WORK_PANEL_TAB_ORDER[WORK_PANEL_TAB_ORDER.length - 1]);
    return `${first} to ${last}`;
  }

  const shortcuts = [
    ...SHORTCUTS.map((shortcut) => ({ description: shortcut.description, binding: binding(shortcut.intent) })),
    { description: "Send", binding: platform === "mac" ? "⌘↩" : "Ctrl+Enter" },
  ];

  async function loadWindowLayout() {
    const current = ++layoutRequest;
    try {
      const view = await loadShellPresentation();
      if (!mounted || current !== layoutRequest) return;
      windowLayout = { kind: "ready", issue: view.issue };
    } catch {
      if (!mounted || current !== layoutRequest) return;
      windowLayout = { kind: "failed" };
    }
  }
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
    void loadWindowLayout();
    return () => {
      mounted = false;
    };
  });
</script>

<!-- The main window may have saved (or failed to save) its layout meanwhile. -->
<svelte:window onfocus={() => void loadWindowLayout()} />

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
  <h3>Window layout</h3>
  {#if windowLayout.kind === "loading"}
    <span class="loading-bar short" aria-hidden="true"></span>
  {:else if windowLayout.kind === "failed"}
    <div class="notice critical" role="status">
      <p>Jet couldn't check the window layout.</p>
      <button class="text-button" onclick={() => void loadWindowLayout()}>Retry</button>
    </div>
  {:else if windowLayout.issue}
    <p class="notice" role="status">{presentationErrorCopy(windowLayout.issue)} <code>{windowLayout.issue}</code></p>
  {:else}
    <p>Jet remembers the window size, the sidebar and work panel, and their widths.</p>
  {/if}
  <h3>Launch at login</h3>
  <p>Starting Jet at login isn't available on Linux yet.</p>
</section>

<section class="settings-section" aria-labelledby="section-keyboard">
  <h2 id="section-keyboard" tabindex="-1">Keyboard shortcuts</h2>
  <p>
    These work in the main window. {shortcutLabel("close-window", platform)} and {shortcutLabel("quit", platform)} also
    work in Settings.
  </p>
  <dl class="shortcut-list">
    {#each shortcuts as shortcut (shortcut.description)}
      <div>
        <dt>{shortcut.description}</dt>
        <dd><kbd>{shortcut.binding}</kbd></dd>
      </div>
    {/each}
  </dl>
  <p>Some desktops use Ctrl+Alt with a number to switch workspaces. The work panel tabs also move with the arrow keys.</p>
</section>

<style>
  .shortcut-list {
    display: grid;
    gap: 6px;
    margin: 0;
  }

  .shortcut-list div {
    display: flex;
    justify-content: space-between;
    gap: 16px;
  }

  .shortcut-list dd {
    margin: 0;
    white-space: nowrap;
  }
</style>
