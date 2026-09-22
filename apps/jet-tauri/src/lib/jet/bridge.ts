import { Channel, invoke } from "@tauri-apps/api/core";

export type PublicError = {
  category: string;
  code: string;
  message: string;
  retryable: boolean;
};

export type ConnectionSnapshot = {
  state: "online" | "reconnecting";
  coreVersion: string | null;
  daemonStarts: string | null;
  startedAtUnixMs: string | null;
  cursor: string | null;
};

export type PlaneUpdate =
  | { type: "connected"; connection: ConnectionSnapshot }
  | { type: "resumed"; after: string }
  | {
      type: "event";
      sequence: string;
      recorded_at_unix_ms: string;
      kind: string;
    }
  | { type: "reconnecting"; error: PublicError }
  | { type: "failed"; error: PublicError };

/** The only direct Tauri IPC adapter used by presentation code. */
export async function openPlaneFeed(
  receive: (update: PlaneUpdate) => void,
): Promise<ConnectionSnapshot> {
  const onUpdate = new Channel<PlaneUpdate>();
  onUpdate.onmessage = receive;
  return invoke<ConnectionSnapshot>("open_plane_feed", { onUpdate });
}
