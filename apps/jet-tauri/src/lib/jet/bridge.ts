import { Channel, invoke } from "@tauri-apps/api/core";

import type { PlaneHealth, PlaneId } from "./planes";

export type PublicError = {
  category: string;
  code: string;
  message: string;
  retryable: boolean;
  recoveryActions: PublicRecoveryAction[];
  restart: PublicRestart | null;
  revisionConflict: PublicRevisionConflict | null;
  /** Set only for `protocol.feature_unavailable`. Shown only in Plane detail. */
  protocolLimit: { requiredMinor: number; negotiatedMinor: number } | null;
  /** The Plane whose command failed; recovery actions apply to it only. */
  planeId: PlaneId | null;
};

export type PublicRecoveryAction =
  | { type: "refresh_file" }
  | { type: "refresh_conversation" }
  | { type: "refresh_run" }
  | { type: "resume_events"; after: string };

export type PublicRestart =
  | {
      reason: "cursor_expired";
      minimumAvailableCursor: string;
      currentSnapshotRevision: string;
    }
  | { reason: "cursor_ahead" | "pagination_stale"; currentSnapshotRevision: string };

export type PublicRevisionConflict = {
  currentRevision: string;
  safeState:
    | { type: "conversation"; conversationId: string; revision: string | null }
    | {
        type: "run";
        runId: string;
        conversationId: string;
        revision: string;
        lifecycle: RunSummary["lifecycle"];
      };
};

export type ConnectionSnapshot = {
  state: "online" | "reconnecting";
  feedId: string;
  planeId: PlaneId;
  planeIdentity: string | null;
  health: PlaneHealth;
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
      conversation_id: string | null;
      run_id: string | null;
      timeline: TimelineItem[];
    }
  | { type: "reconnecting"; error: PublicError }
  | { type: "failed"; error: PublicError };

export type TimelineItem = {
  kind: "user" | "agent" | "activity" | "approval" | "result";
  text: string;
  itemId: string | null;
  approval: ApprovalPresentation | null;
};

export type ApprovalPresentation = {
  requestId: string;
  reviewId: string | null;
  runId: string | null;
  tool: string;
  action: string;
  target: string;
  scope: string;
  consequence: string;
  rationale: string | null;
  state: "requested" | "allowed" | "denied" | "unavailable";
  canAuthorizeRetry: boolean;
};

export type SetupSnapshot = {
  plane: {
    coreVersion: string;
    daemonStarts: string;
    platform: string;
  };
  capabilities: {
    harnesses: string[];
    crafts: Array<{
      id: string;
      version: string;
      harnesses: string[];
    }>;
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

export type ConversationRow = {
  planeId: PlaneId;
  id: string;
  revision: string | null;
  title: string;
  createdAtUnixMs: string;
  projectId: string | null;
};

export type ConversationPage = {
  planeId: PlaneId;
  cursor: string;
  conversations: ConversationRow[];
  nextPage: string | null;
};

export type RunSummary = {
  id: string;
  conversationId: string;
  revision: string;
  lifecycle: "created" | "starting" | "active" | "stopping" | "completed" | "failed" | "canceled" | "lost";
  title: string;
  createdAtUnixMs: string;
  endedAtUnixMs: string | null;
};

export type ConversationDetail = {
  conversation: ConversationRow;
  cursor: string;
  workspaceId: string | null;
  workspaceRoot: string | null;
  runs: RunSummary[];
  /** Set at creation; no Command changes it, so it is disclosed read-only. */
  retention: "retain" | "forget_after_final_run";
};

export type ChangedFile = {
  id: string;
  path: string;
  beforeSize: string | null;
  afterSize: string | null;
  status: "added" | "modified" | "deleted";
  origin: "user edit" | "terminal" | "agent" | "mixed" | "external or unknown";
  contentAvailable: boolean;
};

export type WorkArtifact = {
  availability: "stored" | "disk_pressure" | "run_budget_exceeded" | "artifact_size_exceeded";
  sha256: string;
  size: string;
};

export type WorkspaceTerminal = {
  id: string;
  workspaceId: string;
  state: "opening" | "open" | "closing" | "closed" | "unavailable";
};

export type WorkPanelSnapshot = {
  runId: string;
  scope: "Current" | "Final" | "Historical" | "Turn";
  cursor: string;
  totalFiles: number;
  files: ChangedFile[];
  nextPage: string | null;
  patch: string;
  patchTruncated: boolean;
  artifact: WorkArtifact;
  artifactReadId: string | null;
  contentComplete: boolean;
  latestTurn: number;
  workspaceId: string | null;
  terminals: WorkspaceTerminal[];
  terminalIssue: PublicError | null;
};

export type WorkCheckpoint =
  | { kind: "current" }
  | { kind: "final" }
  | { kind: "turn"; turn: number }
  | { kind: "historical"; fromTurn: number; toTurn: number };

export type ChangePage = {
  files: ChangedFile[];
  nextPage: string | null;
};

export type ArtifactChunk = {
  bytes: number[];
  offset: string;
  nextOffset: string;
  complete: boolean;
  verified: boolean;
};

export type EditableFile = {
  fileId: string;
  path: string;
  content: string | null;
  contentBytes: number;
  revision: string;
};

export type FileSaved = {
  fileId: string;
  revision: string;
  message: string;
};

export type ReviewSubmitted = {
  turnId: string;
  state: string;
  message: string;
};

export type TerminalUpdate =
  | { type: "attached"; terminalId: string; after: string }
  | { type: "output"; terminalId: string; offset: string; bytes: number[]; nextOffset: string }
  | {
      type: "gap";
      terminalId: string;
      firstMissingOffset: string;
      missingBytes: string;
      nextOffset: string;
    }
  | { type: "resized"; terminalId: string }
  | { type: "finished"; terminalId: string; totalBytes: string }
  | { type: "failed"; terminalId: string; error: PublicError };

export type ConversationSearchResult = {
  planeId: PlaneId;
  cursor: string;
  indexedThrough: string;
  hits: Array<{
    conversationId: string;
    sequence: string;
    field: "name" | "path" | "branch";
    excerpt: string;
  }>;
};

export type StartResult = {
  run: RunSummary;
  prompt: string;
};

export type TurnResult = {
  id: string;
  sequence: string;
  state: string;
  prompt: string;
};

export type TurnQueueItem = {
  id: string;
  sequence: string;
  position: number;
  source: "user" | "schedule" | "auto_continue";
  state:
    | "queued"
    | "active"
    | "completed"
    | "superseded"
    | "canceled"
    | "withdrawn"
    | "failed"
    | "outcome_unknown";
  runId: string | null;
  target: "Current Run" | "Next Run";
  withdrawable: boolean;
};

export type RunSupervision = {
  cursor: string;
  maximumEntries: number;
  maximumPromptBytes: number;
  turns: TurnQueueItem[];
  execution: {
    cursor: string;
    run: RunSummary;
    activity:
      | "working"
      | "waiting_for_user"
      | "waiting_for_approval"
      | "waiting_for_auth"
      | "waiting_for_quota"
      | "reconnecting"
      | null;
    needsAttention: boolean;
    termination: {
      control: "interrupt_turn" | "stop_run";
      stage: "native_cancellation" | "interrupt" | "terminate" | "kill" | "unobserved";
      summary: string;
    } | null;
  } | null;
};

export type RunControlAccepted = {
  run: RunSummary;
  control: "interrupt_turn" | "stop_run";
  message: string;
};

export type ApprovalRetryAccepted = {
  reviewId: string;
  message: string;
};

/**
 * The core Tauri IPC adapter used by presentation code. Every Conversation- and
 * Run-scoped call takes the opaque Plane handle of the row it acts on; `null`
 * means this computer.
 */
export async function openPlaneFeed(
  receive: (update: PlaneUpdate) => void,
  after: string | null,
  planeId: PlaneId | null = null,
  reset = false,
): Promise<ConnectionSnapshot> {
  const onUpdate = new Channel<PlaneUpdate>();
  onUpdate.onmessage = receive;
  return invoke<ConnectionSnapshot>("open_plane_feed", { planeId, onUpdate, after, reset });
}

/** Stops a Plane feed natively; a stale feed ID never stops a newer feed. */
export function closePlaneFeed(feedId: string): Promise<void> {
  return invoke<void>("close_plane_feed", { feedId });
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

export function loadConversations(
  nextPage: string | null = null,
  planeId: PlaneId | null = null,
): Promise<ConversationPage> {
  return invoke<ConversationPage>("load_conversations", { planeId, nextPage });
}

export function searchConversations(
  text: string,
  planeId: PlaneId | null = null,
): Promise<ConversationSearchResult> {
  return invoke<ConversationSearchResult>("search_conversations", { planeId, text });
}

export function loadConversation(
  conversationId: string,
  planeId: PlaneId | null = null,
): Promise<ConversationDetail> {
  return invoke<ConversationDetail>("load_conversation", { conversationId, planeId });
}

/**
 * New tasks run on this computer in Wave 3.1. `attempt` (a UUID) names one
 * Send of the composer: the shell resends an uncertain request only for the
 * same attempt, so a retry is answered with what the Plane already did and a
 * later Send is new work.
 */
export function createConversation(projectId: string, attempt: string): Promise<ConversationRow> {
  return invoke<ConversationRow>("create_conversation", { projectId, attempt });
}

/** `attempt` names one Send of the composer, as for `createConversation`. */
export function startRun(
  conversationId: string,
  craft: string,
  prompt: string,
  attempt: string,
  planeId: PlaneId | null = null,
): Promise<StartResult> {
  return invoke<StartResult>("start_run", { conversationId, craft, prompt, attempt, planeId });
}

/** `attempt` names one Send of the composer, as for `createConversation`. */
export function submitTurn(
  conversationId: string,
  prompt: string,
  attempt: string,
  planeId: PlaneId | null = null,
): Promise<TurnResult> {
  return invoke<TurnResult>("submit_turn", { conversationId, prompt, attempt, planeId });
}

export function loadRunSupervision(
  conversationId: string,
  runId: string | null,
  planeId: PlaneId | null = null,
): Promise<RunSupervision> {
  return invoke<RunSupervision>("load_run_supervision", { conversationId, runId, planeId });
}

export function withdrawTurn(
  conversationId: string,
  turnId: string,
  planeId: PlaneId | null = null,
): Promise<TurnQueueItem> {
  return invoke<TurnQueueItem>("withdraw_turn", { conversationId, turnId, planeId });
}

export function interruptTurn(
  runId: string,
  planeId: PlaneId | null = null,
): Promise<RunControlAccepted> {
  return invoke<RunControlAccepted>("interrupt_turn", { runId, planeId });
}

export function stopRun(runId: string, planeId: PlaneId | null = null): Promise<RunControlAccepted> {
  return invoke<RunControlAccepted>("stop_run", { runId, planeId });
}

export function authorizeApprovalRetry(
  runId: string,
  reviewId: string,
  planeId: PlaneId | null = null,
): Promise<ApprovalRetryAccepted> {
  return invoke<ApprovalRetryAccepted>("authorize_approval_retry", { runId, reviewId, planeId });
}

export function loadWorkPanel(
  conversationId: string,
  runId: string,
  scope: WorkCheckpoint,
  planeId: PlaneId | null = null,
): Promise<WorkPanelSnapshot> {
  return invoke<WorkPanelSnapshot>("load_work_panel", {
    conversationId,
    runId,
    scopeKind: scope.kind,
    turn: scope.kind === "turn" ? scope.turn : null,
    fromTurn: scope.kind === "historical" ? scope.fromTurn : null,
    toTurn: scope.kind === "historical" ? scope.toTurn : null,
    planeId,
  });
}

// Follow-up Work panel commands carry only native IDs. The shell executes them
// on the Plane the Work panel was loaded from.
export function loadMoreChanges(pageId: string): Promise<ChangePage> {
  return invoke<ChangePage>("load_more_changes", { pageId });
}

export function loadPatchChunk(artifactReadId: string): Promise<ArtifactChunk> {
  return invoke<ArtifactChunk>("load_patch_chunk", { artifactReadId });
}

export function loadWorkFile(fileId: string): Promise<EditableFile> {
  return invoke<EditableFile>("load_work_file", { fileId });
}

export function saveWorkFile(fileId: string, content: string): Promise<FileSaved> {
  return invoke<FileSaved>("save_work_file", { fileId, content });
}

export function submitFileReview(
  fileId: string,
  line: number,
  comment: string,
): Promise<ReviewSubmitted> {
  return invoke<ReviewSubmitted>("submit_file_review", { fileId, line, comment });
}

export function openWorkspaceTerminal(
  conversationId: string,
  rows = 24,
  columns = 80,
  planeId: PlaneId | null = null,
): Promise<WorkspaceTerminal> {
  return invoke<WorkspaceTerminal>("open_workspace_terminal", {
    conversationId,
    rows,
    columns,
    planeId,
  });
}
export function closeWorkspaceTerminal(terminalId: string): Promise<WorkspaceTerminal> {
  return invoke<WorkspaceTerminal>("close_workspace_terminal", { terminalId });
}

export function attachWorkspaceTerminal(
  terminalId: string,
  receive: (update: TerminalUpdate) => void,
): Promise<void> {
  const onUpdate = new Channel<TerminalUpdate>();
  onUpdate.onmessage = receive;
  return invoke<void>("attach_workspace_terminal", { terminalId, onUpdate });
}

export function sendTerminalInput(terminalId: string, input: string): Promise<void> {
  return invoke<void>("send_terminal_input", { terminalId, input });
}

export function resizeWorkspaceTerminal(
  terminalId: string,
  rows: number,
  columns: number,
): Promise<void> {
  return invoke<void>("resize_workspace_terminal", { terminalId, rows, columns });
}

export function detachWorkspaceTerminal(terminalId: string): Promise<void> {
  return invoke<void>("detach_workspace_terminal", { terminalId });
}
