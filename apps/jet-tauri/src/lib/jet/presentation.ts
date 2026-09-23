import { invoke } from "@tauri-apps/api/core";

/**
 * Client-local window layout and window modes (wave 3.4 §6.1). Nothing
 * here is Jet state: it is never sent to a Plane, holds no Jet identifier,
 * and lives natively in the app data directory, never in browser storage.
 *
 * Keep the two literal arrays in exactly this form: a Rust test reads them
 * and checks that they match the shell's enums.
 */
export const WORK_PANEL_TABS = ["changes", "files", "terminal", "run", "delivery"] as const;
export const RESTORABLE_DESTINATIONS = ["conversation", "new-task", "project", "schedules", "planes"] as const;

export type WorkPanelTab = (typeof WORK_PANEL_TABS)[number];
export type RestorableDestination = (typeof RESTORABLE_DESTINATIONS)[number];

export type ShellPresentation = {
  version: 1;
  destination: RestorableDestination;
  sidebarPresented: boolean;
  /** The regular-window column preference; the compact overlay is never saved. */
  workPanelPresented: boolean;
  workPanelTab: WorkPanelTab;
  sidebarWidth: number;
  workPanelWidth: number;
};

export type PresentationIssue = "presentation.read_failed" | "presentation.write_failed";
export type ShellPresentationView = { presentation: ShellPresentation; issue: PresentationIssue | null };
export type WindowMode = { fullscreen: boolean };

export const DEFAULT_PRESENTATION: ShellPresentation = {
  version: 1,
  destination: "conversation",
  sidebarPresented: true,
  workPanelPresented: true,
  workPanelTab: "run",
  sidebarWidth: 244,
  workPanelWidth: 340,
};

function member<T extends string>(values: readonly T[], value: unknown): value is T {
  return typeof value === "string" && (values as readonly string[]).includes(value);
}

/** Destinations that reopen as themselves; everything else reopens the task view. */
export function toRestorable(destination: string): RestorableDestination {
  return member(RESTORABLE_DESTINATIONS, destination) ? destination : "conversation";
}

export function isWorkPanelTab(value: unknown): value is WorkPanelTab {
  return member(WORK_PANEL_TABS, value);
}

function isPresentation(value: unknown): value is ShellPresentation {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return (
    candidate.version === 1 &&
    member(RESTORABLE_DESTINATIONS, candidate.destination) &&
    typeof candidate.sidebarPresented === "boolean" &&
    typeof candidate.workPanelPresented === "boolean" &&
    isWorkPanelTab(candidate.workPanelTab) &&
    Number.isInteger(candidate.sidebarWidth) &&
    Number.isInteger(candidate.workPanelWidth)
  );
}

/** The shell's reply, checked; a malformed reply is refused like a failed call. */
function presentationView(value: unknown): ShellPresentationView {
  const candidate = (typeof value === "object" && value !== null ? value : {}) as Record<string, unknown>;
  const issue = candidate.issue;
  if (
    !isPresentation(candidate.presentation) ||
    !(issue === null || issue === "presentation.read_failed" || issue === "presentation.write_failed")
  ) {
    throw {
      category: "invalid_response",
      code: "presentation.invalid_response",
      message: "Jet couldn't read the saved window layout.",
      retryable: false,
    };
  }
  return { presentation: candidate.presentation, issue };
}

export const loadShellPresentation = async (): Promise<ShellPresentationView> =>
  presentationView(await invoke<unknown>("load_shell_presentation"));

export const saveShellPresentation = async (presentation: ShellPresentation): Promise<ShellPresentationView> =>
  presentationView(await invoke<unknown>("save_shell_presentation", { presentation }));

export const toggleMainWindowFullscreen = () => invoke<WindowMode>("toggle_main_window_fullscreen");
export const closeMainWindow = () => invoke<void>("close_main_window");
export const quitJet = () => invoke<void>("quit_jet");

/** UI copy keyed by stable code, never by the shell's message. */
export function presentationErrorCopy(code: string): string {
  switch (code) {
    case "presentation.read_failed":
      return "Jet couldn't read the saved window layout, so it started with the default layout.";
    case "presentation.write_failed":
      return "Jet couldn't save the window layout. It'll try again when the layout changes.";
    case "presentation.out_of_range":
      return "Jet couldn't save the window layout.";
    case "window.mode_unavailable":
      return "Full screen isn't available in this window.";
    default:
      return "Jet couldn't complete that window action.";
  }
}
