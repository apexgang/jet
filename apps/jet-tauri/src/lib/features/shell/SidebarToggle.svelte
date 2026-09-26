<script lang="ts">
  import type { DesktopSession } from "./session.svelte";
  import { currentPlatform, shortcutAria, shortcutLabel } from "./shortcuts";

  let { session }: { session: DesktopSession } = $props();
  const platform = currentPlatform();
  const action = $derived(session.sidebarPresented ? "Hide sidebar" : "Show sidebar");
</script>

<!-- Every main-window destination header carries this, so a hidden sidebar can always come back. -->
<button
  class="icon-button sidebar-toggle"
  aria-label={action}
  aria-keyshortcuts={shortcutAria("toggle-sidebar", platform)}
  title={`${action} (${shortcutLabel("toggle-sidebar", platform)})`}
  onclick={() => session.toggleSidebar()}
>
  <svg width="16" height="16" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true"><rect x="2" y="3" width="16" height="14" rx="2" /><path d="M7 3v14" /></svg>
</button>
