/**
 * The main window's keyboard shortcuts as a pure model: which key press
 * means which shell intent, when a press must be ignored, and how a binding
 * is written for the current platform. No DOM access, so it runs in node.
 *
 * The macOS bindings mirror the Swift client's menu commands
 * (`apps/jet/jet/App/JetCommands.swift`); Linux uses Ctrl and function keys.
 */
import type { WorkPanelTab } from "./session.svelte";

export type Platform = "mac" | "other";

export type ShellIntent =
  | { kind: "new-task" }
  | { kind: "search" }
  | { kind: "add-project" }
  | { kind: "settings" }
  | { kind: "toggle-sidebar" }
  | { kind: "toggle-work-panel" }
  | { kind: "work-panel-tab"; tab: WorkPanelTab }
  | { kind: "toggle-fullscreen" }
  | { kind: "close-window" }
  | { kind: "quit" }
  /** Escape: closes the open Run-control confirmation. */
  | { kind: "dismiss" };

export type ShellIntentKind = ShellIntent["kind"];

export type KeyInput = Pick<
  KeyboardEvent,
  "key" | "code" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey" | "isComposing" | "repeat"
>;

export type ShortcutContext = {
  platform: Platform;
  /** A native `<dialog>` is open; it handles its own keys, Escape included. */
  modalOpen: boolean;
  /** Something Escape can close is open. */
  dismissible: boolean;
  /** Intents whose action has shipped. Others never resolve. */
  enabled: ReadonlySet<ShellIntentKind>;
};

/** Work panel tabs in shortcut order: Ctrl+Alt+1 is Changes. */
export const WORK_PANEL_TAB_ORDER: readonly WorkPanelTab[] = [
  "changes",
  "files",
  "terminal",
  "run",
  "delivery",
];

/** The shortcuts the main window offers, in the order Settings lists them. */
export const SHORTCUTS: ReadonlyArray<{
  intent: ShellIntentKind;
  description: string;
  since: "3.2" | "S1" | "S3" | "S4";
}> = [
  { intent: "new-task", description: "New task", since: "S1" },
  { intent: "search", description: "Search Jet", since: "S1" },
  { intent: "add-project", description: "Add a Project", since: "S1" },
  { intent: "settings", description: "Open Settings", since: "3.2" },
  { intent: "toggle-sidebar", description: "Show or hide the sidebar", since: "S1" },
  { intent: "toggle-work-panel", description: "Show or hide the work panel", since: "S1" },
  { intent: "work-panel-tab", description: "Choose a work panel tab", since: "S1" },
  { intent: "dismiss", description: "Cancel an Interrupt Turn or Stop Run confirmation", since: "S1" },
];

/** Intents whose action ships in this build; `SHORTCUTS` lists exactly these. */
export const ENABLED_SHORTCUTS: ReadonlySet<ShellIntentKind> = new Set(
  SHORTCUTS.map((shortcut) => shortcut.intent),
);

/** Intents that flip state; a held key must not flip it back and forth. */
const TOGGLES: ReadonlySet<ShellIntentKind> = new Set([
  "toggle-sidebar",
  "toggle-work-panel",
  "toggle-fullscreen",
  "close-window",
  "quit",
]);

export function detectPlatform(userAgent: string): Platform {
  return /Macintosh|Mac OS X/.test(userAgent) ? "mac" : "other";
}

/** The platform of this webview, or "other" outside a browser. */
export function currentPlatform(): Platform {
  return typeof navigator === "undefined" ? "other" : detectPlatform(navigator.userAgent ?? "");
}

type Modifiers = { primary: boolean; shift: boolean; alt: boolean; control: boolean };

/**
 * Which modifiers are held, normalised per platform. On macOS the primary
 * modifier is Command and Control stays separate; elsewhere the primary
 * modifier is Ctrl and the Super/Meta key never counts as one.
 */
function modifiers(input: KeyInput, platform: Platform): Modifiers | null {
  if (platform === "mac") {
    return { primary: input.metaKey, shift: input.shiftKey, alt: input.altKey, control: input.ctrlKey };
  }
  if (input.metaKey) return null;
  return { primary: input.ctrlKey, shift: input.shiftKey, alt: input.altKey, control: false };
}

/** A Latin letter from `key`, or from the physical key on other layouts. */
function letter(input: KeyInput): string | null {
  if (/^[a-z]$/i.test(input.key)) return input.key.toLowerCase();
  if (/^Key[A-Z]$/.test(input.code)) return input.code.slice(3).toLowerCase();
  return null;
}

/** A top-row digit from the physical key: Alt changes `key` on some layouts. */
function digit(input: KeyInput): number | null {
  const match = /^Digit([0-9])$/.exec(input.code);
  return match ? Number(match[1]) : null;
}

function isComma(input: KeyInput): boolean {
  return input.key === "," || (!/^[ -~]$/.test(input.key) && input.code === "Comma");
}

function exactly(held: Modifiers, wanted: Partial<Modifiers>): boolean {
  return (
    held.primary === (wanted.primary ?? false) &&
    held.shift === (wanted.shift ?? false) &&
    held.alt === (wanted.alt ?? false) &&
    held.control === (wanted.control ?? false)
  );
}

function none(held: Modifiers): boolean {
  return exactly(held, {});
}

function match(input: KeyInput, platform: Platform): ShellIntent | null {
  const held = modifiers(input, platform);
  if (!held) return null;
  const mac = platform === "mac";

  if (input.key === "Escape") return none(held) ? { kind: "dismiss" } : null;
  if (input.key === "F9") return !mac && none(held) ? { kind: "toggle-sidebar" } : null;
  if (input.key === "F11") return !mac && none(held) ? { kind: "toggle-fullscreen" } : null;

  const number = digit(input);
  if (number !== null && exactly(held, { primary: true, alt: true })) {
    if (number === 0) return { kind: "toggle-work-panel" };
    const tab = WORK_PANEL_TAB_ORDER[number - 1];
    return tab ? { kind: "work-panel-tab", tab } : null;
  }

  if (isComma(input)) return exactly(held, { primary: true }) ? { kind: "settings" } : null;

  const character = letter(input);
  if (character === null) return null;
  if (mac && exactly(held, { primary: true, control: true })) {
    if (character === "s") return { kind: "toggle-sidebar" };
    if (character === "f") return { kind: "toggle-fullscreen" };
    return null;
  }
  if (exactly(held, { primary: true, shift: true })) {
    return character === "o" ? { kind: "add-project" } : null;
  }
  if (!exactly(held, { primary: true })) return null;
  switch (character) {
    case "n":
      return { kind: "new-task" };
    case "k":
      return { kind: "search" };
    case "w":
      return { kind: "close-window" };
    case "q":
      return { kind: "quit" };
    default:
      return null;
  }
}

/**
 * The shell intent a key press asks for, or null when the press belongs to
 * someone else: an input method composing text, an open modal dialog, a
 * held-down toggle, an intent that has not shipped, or an unbound key.
 * Unmodified letters and digits never resolve, so typing is never shadowed.
 */
export function resolveShortcut(input: KeyInput, context: ShortcutContext): ShellIntent | null {
  if (input.isComposing || context.modalOpen) return null;
  const intent = match(input, context.platform);
  if (!intent || !context.enabled.has(intent.kind)) return null;
  if (input.repeat && TOGGLES.has(intent.kind)) return null;
  if (intent.kind === "dismiss" && !context.dismissible) return null;
  return intent;
}

type Binding = {
  /** Visible text, e.g. "Ctrl+N" or "⌘N". */
  label: string;
  /** `aria-keyshortcuts` value, in UI Events key names. */
  aria: string;
};

function binding(intent: ShellIntentKind, platform: Platform, tab: WorkPanelTab = "changes"): Binding {
  const mac = platform === "mac";
  const primary = (key: string, extra: { shift?: boolean; alt?: boolean } = {}): Binding => {
    if (mac) {
      return {
        label: `${extra.alt ? "⌥" : ""}${extra.shift ? "⇧" : ""}⌘${key}`,
        aria: `Meta+${extra.alt ? "Alt+" : ""}${extra.shift ? "Shift+" : ""}${key}`,
      };
    }
    const parts = ["Ctrl", ...(extra.alt ? ["Alt"] : []), ...(extra.shift ? ["Shift"] : []), key];
    return { label: parts.join("+"), aria: ["Control", ...parts.slice(1)].join("+") };
  };
  switch (intent) {
    case "new-task":
      return primary("N");
    case "search":
      return primary("K");
    case "add-project":
      return primary("O", { shift: true });
    case "settings":
      return primary(",");
    case "toggle-sidebar":
      return mac ? { label: "⌃⌘S", aria: "Control+Meta+S" } : { label: "F9", aria: "F9" };
    case "toggle-work-panel":
      return primary("0", { alt: true });
    case "work-panel-tab":
      return primary(String(WORK_PANEL_TAB_ORDER.indexOf(tab) + 1), { alt: true });
    case "toggle-fullscreen":
      return mac ? { label: "⌃⌘F", aria: "Control+Meta+F" } : { label: "F11", aria: "F11" };
    case "close-window":
      return primary("W");
    case "quit":
      return primary("Q");
    case "dismiss":
      return { label: "Esc", aria: "Escape" };
  }
}

/** The binding as shown to people: "Ctrl+N" on Linux, "⌘N" on macOS. */
export function shortcutLabel(intent: ShellIntentKind, platform: Platform, tab?: WorkPanelTab): string {
  return binding(intent, platform, tab).label;
}

/** The binding as an `aria-keyshortcuts` value, for example "Control+N". */
export function shortcutAria(intent: ShellIntentKind, platform: Platform, tab?: WorkPanelTab): string {
  return binding(intent, platform, tab).aria;
}
