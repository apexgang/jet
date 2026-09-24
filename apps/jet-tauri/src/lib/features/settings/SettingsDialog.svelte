<script lang="ts">
  import { onMount, tick, type Snippet } from "svelte";

  import { focusedElement, returnFocusWhenReady } from "./focus";

  let {
    title,
    lead,
    focus = "primary",
    returnFocus = [],
    focusKey,
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
    /**
     * The step the dialog shows. A dialog that stays open from review through
     * sending to a result swaps its buttons; each new step moves focus again
     * (per `focus`), so it never falls out of the modal.
     */
    focusKey?: unknown;
    /** Escape and Cancel both go through this. */
    oncancel: () => void;
    children: Snippet;
    footer: Snippet;
  } = $props();

  let dialog = $state<HTMLDialogElement>();
  const titleId = $props.id();

  /** The preferred button, else any enabled one, else the dialog itself (while every button is disabled). */
  function moveFocus(): void {
    if (!dialog) return;
    const selector = focus === "cancel" ? "[data-dialog-cancel]" : "[data-dialog-primary]";
    const enabled = (element: HTMLElement | null) =>
      element && !(element instanceof HTMLButtonElement && element.disabled) ? element : null;
    const target =
      enabled(dialog.querySelector<HTMLElement>(selector)) ??
      dialog.querySelector<HTMLElement>("button:not(:disabled)") ??
      dialog;
    target.focus();
  }

  let shownKey: unknown;
  let mounted = false;
  $effect(() => {
    const key = focusKey;
    if (!mounted || Object.is(key, shownKey)) return;
    shownKey = key;
    void tick().then(moveFocus);
  });

  onMount(() => {
    const trigger = focusedElement();
    const fallbacks = [...returnFocus];
    shownKey = focusKey;
    mounted = true;
    void tick().then(() => {
      dialog?.showModal();
      moveFocus();
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
  tabindex="-1"
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
