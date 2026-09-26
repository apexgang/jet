import { open } from "@tauri-apps/plugin-dialog";

/** The OS folder picker is isolated from presentation and exposes no filesystem reads. */
export async function chooseProjectFolder(): Promise<string | null> {
  const selected = await open({ directory: true, multiple: false, title: "Choose a Project folder" });
  return typeof selected === "string" ? selected : null;
}
