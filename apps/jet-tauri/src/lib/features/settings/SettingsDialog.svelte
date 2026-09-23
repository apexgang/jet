<script lang="ts">
  import { onMount, tick, type Snippet } from "svelte";

  import { focusedElement, returnFocusWhenReady } from "./focus";

  let {
    title,
    lead,
    focus = "primary",
    returnFocus = [],
    oncancel,
    children,
    footer,
  }: {
    title: string;
    /** One line under the title: what confirming means. */
    lead?: string;
    /**
     * Where focus starts: the primary action, or Cancel for destructive,
     * irreversible or blocked steps. Mark the buttons with
     * `data-dialog-primary` and `data-dialog-cancel`.
     */
    focus?: "primary" | "cancel";
    /**
     * Where focus goes when the dialog closes and the element focused when
     * it opened is gone or never existed (a dialog opened after an async
     * prepare): element IDs, tried in order once they can take focus.
     */
    returnFocus?: readonly string[];
    /** Escape and Cancel both go through this. */
    oncancel: () => void;
    children: Snippet;
    footer: Snippet;
  } = $props();

  let dialog = $state<HTMLDialogElement>();
  const titleId = $props.id();

  onMount(() => {
    const trigger = focusedElement();
    const fallbacks = [...returnFocus];
    void tick().then(() => {
      dialog?.showModal();
      const selector = focus === "cancel" ? "[data-dialog-cancel]" : "[data-dialog-primary]";
      const target = dialog?.querySelector<HTMLElement>(selector) ?? dialog?.querySelector<HTMLElement>("button");
      target?.focus();
    });
    return () => {
      if (dialog?.open) dialog.close();
      returnFocusWhenReady(trigger, fallbacks);
    };
  });
</script>

<dialog
  bind:this={dialog}
  class="removal-dialog settings-dialog"
  aria-labelledby={titleId}
  oncancel={(event) => {
    event.preventDefault();
    oncancel();
  }}
>
  <form method="dialog" onsubmit={(event) => event.preventDefault()}>
    <header>
      <h2 id={titleId}>{title}</h2>
      {#if lead}<p>{lead}</p>{/if}
    </header>
    {@render children()}
    <footer>
      {@render footer()}
    </footer>
  </form>
</dialog>

<style>
  .settings-dialog :global(.removal-facts dd) {
    overflow: visible;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .settings-dialog :global(.dialog-note) {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }

  .settings-dialog :global(.dialog-alert) {
    margin: 0;
    padding: 10px 12px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--danger) 10%, var(--background));
    font-size: 12px;
  }

  .settings-dialog :global(code) {
    color: var(--quiet);
    font-size: 11px;
  }
</style>
