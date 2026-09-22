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

export type SetupSnapshot = {
  plane: {
    coreVersion: string;
    daemonStarts: string;
    platform: string;
  };
  capabilities: {
    harnesses: string[];
    credentialStore: "available" | "locked" | "unavailable";
    credentialStoreLabel: string;
    degraded: string[];
    authProviders: Array<{
      provider: string;
      harness: string;
      label: string;
    }>;
  };
  projects: ProjectSummary[];
  accounts: AccountSummary[];
  pairing: {
    gate: "open" | "closed";
    pairedClients: number;
    offerPending: boolean;
  };
  issues: Array<{
    section: "capabilities" | "projects" | "accounts" | "pairing";
    error: PublicError;
  }>;
};

export type ProjectSummary = {
  id: string;
  name: string;
  root: string;
};

export type AccountSummary = {
  id: string;
  label: string;
  provider: string;
  state: string;
  stateLabel: string;
};

export type ProjectPreview = {
  previewId: string | null;
  root: string;
  verdict: string;
  detail: string;
};

export type ProjectRemovalPreview = {
  previewId: string;
  projectId: string;
  name: string;
  root: string;
  diskUseBytes: string;
  liveRuns: string;
  schedules: string;
  dirtyFiles: string;
  unpushedCommits: string;
  workspaceCount: number;
  obstacles: string[];
  permanentWarning: string;
};

export type MutationResult = {
  id: string;
  name: string;
};

/** The only direct Tauri IPC adapter used by presentation code. */
export async function openPlaneFeed(
  receive: (update: PlaneUpdate) => void,
): Promise<ConnectionSnapshot> {
  const onUpdate = new Channel<PlaneUpdate>();
  onUpdate.onmessage = receive;
  return invoke<ConnectionSnapshot>("open_plane_feed", { onUpdate });
}

export function loadSetup(): Promise<SetupSnapshot> {
  return invoke<SetupSnapshot>("load_setup");
}

export function previewProject(path: string): Promise<ProjectPreview> {
  return invoke<ProjectPreview>("preview_project", { path });
}

export function registerProject(previewId: string): Promise<MutationResult> {
  return invoke<MutationResult>("register_project", { previewId });
}

export function previewProjectRemoval(projectId: string): Promise<ProjectRemovalPreview> {
  return invoke<ProjectRemovalPreview>("preview_project_removal", { projectId });
}

export function removeProject(
  previewId: string,
  typedName: string,
  permanent: boolean,
): Promise<MutationResult> {
  return invoke<MutationResult>("remove_project", { previewId, typedName, permanent });
}

export function bindHarnessAccount(provider: string): Promise<MutationResult> {
  return invoke<MutationResult>("bind_harness_account", { provider });
}
