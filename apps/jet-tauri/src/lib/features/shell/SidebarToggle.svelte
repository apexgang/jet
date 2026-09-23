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
  Sidebar
</button>
