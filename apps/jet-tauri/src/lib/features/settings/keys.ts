/**
 * The Settings window's keyboard shortcuts. They resolve through the main
 * window's shortcut model, so Ctrl+W, Ctrl+Q and Ctrl+, behave the same in
 * both windows, including on non-Latin layouts (the physical key decides).
 */
import { quitJet } from "$lib/jet/presentation";
import { closeSettings } from "$lib/jet/settings-window";
import {
  resolveShortcut,
  type KeyInput,
  type Platform,
  type ShellIntentKind,
} from "$lib/features/shell/shortcuts";

/** The main-window intents Settings answers; the rest belong to the main window. */
const SETTINGS_INTENTS: ReadonlySet<ShellIntentKind> = new Set(["close-window", "quit", "settings"]);

export type SettingsKeyAction = "close" | "quit" | "focus-navigation";

/** What a key press asks the Settings window to do, or null to leave it alone. */
export function settingsKeyAction(input: KeyInput, platform: Platform): SettingsKeyAction | null {
  const intent = resolveShortcut(input, {
    platform,
    modalOpen: false,
    dismissible: false,
    enabled: SETTINGS_INTENTS,
  });
  switch (intent?.kind) {
    case "close-window":
      return "close";
    case "quit":
      return "quit";
    case "settings":
      return "focus-navigation";
    default:
      return null;
  }
}

/**
 * Handles one key press in the Settings window. Ctrl+W closes Settings,
 * Ctrl+Q quits Jet (Runs keep going on their Planes), and Ctrl+, moves
 * focus to the pane list.
 */
export function handleSettingsKey(
  event: KeyInput & Pick<KeyboardEvent, "preventDefault">,
  platform: Platform,
  focusNavigation: () => void,
): void {
  const action = settingsKeyAction(event, platform);
  if (!action) return;
  event.preventDefault();
  if (action === "close") closeSettings().catch(() => undefined);
  else if (action === "quit") quitJet().catch(() => undefined);
  else focusNavigation();
}
