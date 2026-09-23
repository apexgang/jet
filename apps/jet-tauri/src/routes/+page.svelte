<script lang="ts">
  import { onMount } from "svelte";

  import Conversation from "$lib/features/shell/Conversation.svelte";
  import Sidebar from "$lib/features/shell/Sidebar.svelte";
  import { DesktopSession } from "$lib/features/shell/session.svelte";
  import { ENABLED_SHORTCUTS, currentPlatform } from "$lib/features/shell/shortcuts";
  import "$lib/features/shell/theme.css";
  import WorkPanel from "$lib/features/shell/WorkPanel.svelte";

  const session = new DesktopSession();

  onMount(() => {
    session.connect();
    const platform = currentPlatform();
    const handleShortcut = (event: KeyboardEvent) =>
      session.handleShortcut(event, {
        platform,
        // A native modal dialog owns the keyboard, Escape included.
        modalOpen: document.querySelector("dialog[open]") !== null,
        enabled: ENABLED_SHORTCUTS,
      });
    window.addEventListener("keydown", handleShortcut);
    return () => {
      window.removeEventListener("keydown", handleShortcut);
      session.disconnect();
    };
  });
</script>

<svelte:head>
  <title>Jet</title>
  <meta name="description" content="Jet desktop workspace" />
</svelte:head>

<div
  class:panel-hidden={!session.workPanelPresented}
  class:sidebar-hidden={!session.sidebarPresented}
  class="app-shell"
>
  <Sidebar {session} />
  <Conversation {session} />
  <WorkPanel {session} />
</div>
