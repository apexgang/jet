<script lang="ts">
  import { watchDesktopCommands } from "$lib/jet/desktop-menu";
  import { onMount } from "svelte";

  import AppShell from "$lib/features/shell/AppShell.svelte";
  import { DesktopSession } from "$lib/features/shell/session.svelte";
  import { ENABLED_SHORTCUTS, currentPlatform } from "$lib/features/shell/shortcuts";
  import "$lib/features/shell/theme.css";

  const session = new DesktopSession();
  /** Layout changes settle for this long before the window layout is saved. */
  const PERSIST_DELAY_MS = 300;

  // Saves the window layout natively once it stops changing. Nothing is
  // written while the saved layout is still loading (the key is null).
  $effect(() => {
    if (session.presentationKey === null) return;
    const timer = setTimeout(() => void session.persistPresentation(), PERSIST_DELAY_MS);
    return () => clearTimeout(timer);
  });

  onMount(() => {
    session.connect();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    watchDesktopCommands((command) => {
      if (document.querySelector("dialog[open]")) return;
      switch (command) {
        case "settings": void session.openSettings(); break;
        case "sidebar": session.sidebarPresented = !session.sidebarPresented; break;
        case "details": session.toggleWorkPanel("shortcut"); break;
        case "changes": case "files": case "terminal": case "run": case "delivery": session.showPanel(command, "shortcut"); break;
        case "search": session.perform({ kind: "search" }); break;
        case "project": session.perform({ kind: "add-project" }); break;
        case "new-task": case "planes": case "trash": session.select(command); break;
      }
    }).then((stop) => { if (disposed) stop(); else unlisten = stop; }).catch(() => undefined);
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
      disposed = true;
      unlisten?.();
      session.disconnect();
    };
  });
</script>

<svelte:head>
  <title>Jet</title>
  <meta name="description" content="Jet desktop workspace" />
</svelte:head>

<AppShell {session} />
