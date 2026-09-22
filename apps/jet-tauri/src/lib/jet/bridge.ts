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
  id: string;
  title: string;
  createdAtUnixMs: string;
  projectId: string | null;
};

export type ConversationPage = {
  cursor: string;
  conversations: ConversationRow[];
  nextPage: string | null;
  restoredId: string | null;
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
  workspaceRoot: string | null;
  runs: RunSummary[];
};

export type ConversationSearchResult = {
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

/** The only direct Tauri IPC adapter used by presentation code. */
export async function openPlaneFeed(
  receive: (update: PlaneUpdate) => void,
  after: string,
): Promise<ConnectionSnapshot> {
  const onUpdate = new Channel<PlaneUpdate>();
  onUpdate.onmessage = receive;
  return invoke<ConnectionSnapshot>("open_plane_feed", { onUpdate, after });
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

export function loadConversations(nextPage: string | null = null): Promise<ConversationPage> {
  return invoke<ConversationPage>("load_conversations", { nextPage });
}

export function searchConversations(text: string): Promise<ConversationSearchResult> {
  return invoke<ConversationSearchResult>("search_conversations", { text });
}

export function loadConversation(conversationId: string): Promise<ConversationDetail> {
  return invoke<ConversationDetail>("load_conversation", { conversationId });
}

export function createConversation(projectId: string): Promise<ConversationRow> {
  return invoke<ConversationRow>("create_conversation", { projectId });
}

export function startRun(
  conversationId: string,
  craft: string,
  prompt: string,
): Promise<StartResult> {
  return invoke<StartResult>("start_run", { conversationId, craft, prompt });
}

export function submitTurn(conversationId: string, prompt: string): Promise<TurnResult> {
  return invoke<TurnResult>("submit_turn", { conversationId, prompt });
}

export function loadRunSupervision(
  conversationId: string,
  runId: string | null,
): Promise<RunSupervision> {
  return invoke<RunSupervision>("load_run_supervision", { conversationId, runId });
}

export function withdrawTurn(conversationId: string, turnId: string): Promise<TurnQueueItem> {
  return invoke<TurnQueueItem>("withdraw_turn", { conversationId, turnId });
}

export function interruptTurn(runId: string): Promise<RunControlAccepted> {
  return invoke<RunControlAccepted>("interrupt_turn", { runId });
}

export function stopRun(runId: string): Promise<RunControlAccepted> {
  return invoke<RunControlAccepted>("stop_run", { runId });
}

export function authorizeApprovalRetry(
  runId: string,
  reviewId: string,
): Promise<ApprovalRetryAccepted> {
  return invoke<ApprovalRetryAccepted>("authorize_approval_retry", { runId, reviewId });
}
