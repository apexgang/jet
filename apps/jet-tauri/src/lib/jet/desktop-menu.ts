import { listen, type UnlistenFn } from "@tauri-apps/api/event";
export type DesktopCommand = "settings" | "new-task" | "project" | "search" | "sidebar" | "details" | "changes" | "files" | "terminal" | "run" | "delivery" | "planes" | "trash";
export function watchDesktopCommands(receive: (command: DesktopCommand) => void): Promise<UnlistenFn> {
  return listen<DesktopCommand>("jet:desktop-command", (event) => receive(event.payload));
}
