<script lang="ts">
  import { onMount } from "svelte";

  import Conversation from "$lib/features/shell/Conversation.svelte";
  import Sidebar from "$lib/features/shell/Sidebar.svelte";
  import { DesktopSession } from "$lib/features/shell/session.svelte";
  import "$lib/features/shell/theme.css";
  import WorkPanel from "$lib/features/shell/WorkPanel.svelte";

  const session = new DesktopSession();

  onMount(() => {
    session.connect();
    const handleShortcut = (event: KeyboardEvent) => session.handleShortcut(event);
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
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
