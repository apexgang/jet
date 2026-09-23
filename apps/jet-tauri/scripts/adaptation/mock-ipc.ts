/**
 * Mocked IPC for the adaptation audit (wave 3.4 §8). Bundled per run with
 * `bun build --target browser --format iife` and loaded as a page init
 * script after `scene-prelude.js`, so it is in place before the app's first
 * `invoke`.
 *
 * Answers come from synthetic data derived from the shared fixture corpus
 * (`fixtures/desktop/presentation-v1.json`), shaped like the shell's own
 * replies. A command this file does not answer is refused with an
 * `unavailable` error, which the app discloses like an offline Plane; its
 * name is recorded for the report. Nothing here reaches a Plane or the disk.
 */

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

import corpus from "../../../../fixtures/desktop/presentation-v1.json";
import type {
  ConnectionSnapshot,
  ConversationDetail,
  ConversationPage,
  ConversationRow,
  PlaneUpdate,
  PublicError,
  RunSummary,
  RunSupervision,
  SetupSnapshot,
  TimelineItem,
  WorkPanelSnapshot,
} from "../../src/lib/jet/bridge";
import type { Delivery } from "../../src/lib/jet/delivery";
import type { NotificationSettings } from "../../src/lib/jet/notifications";
import type { Plane, PlanesSnapshot } from "../../src/lib/jet/planes";
import type { ShellPresentation, ShellPresentationView } from "../../src/lib/jet/presentation";
import type { TrashStatus, TrashView } from "../../src/lib/jet/retention";
import type { SettingsNavigation } from "../../src/lib/jet/settings-window";
import type { SceneName } from "./configurations";

type CorpusScenario = {
  id: string;
  project?: { id: string; name: string };
  conversation?: { id: string; title: string };
  run?: { id: string; lifecycle: string; activity?: string | null };
  timeline: Array<{ id: string; kind: string; text: string }>;
  capabilities: { harnesses: string[] };
};

type AdaptationWindow = Window & {
  __JET_ADAPTATION_SCENE__?: SceneName;
  __JET_ADAPTATION_UNMOCKED__?: string[];
};

const scenarios = (corpus as { scenarios: CorpusScenario[] }).scenarios;

function scenario(id: string): CorpusScenario {
  const found = scenarios.find((candidate) => candidate.id === id);
  if (!found) throw new Error(`The fixture corpus has no ${id} scenario.`);
  return found;
}

const approval = scenario("approval-needed");
const completed = scenario("completed-run");
const PROJECT = approval.project ?? { id: "00000000-0000-7000-8000-000000000010", name: "Jet" };
const CONVERSATION = approval.conversation ?? { id: "00000000-0000-7000-8000-000000000020", title: "Task" };
const RUN_ID = approval.run?.id ?? "00000000-0000-7000-8000-000000000030";
const HARNESSES = approval.capabilities.harnesses;
const CREATED = "1790000000000";

const adaptationWindow = window as AdaptationWindow;
const scene: SceneName = adaptationWindow.__JET_ADAPTATION_SCENE__ ?? "approval-run";
const unmocked: string[] = (adaptationWindow.__JET_ADAPTATION_UNMOCKED__ = []);

function unavailable(code: string): PublicError {
  return {
    category: "unavailable",
    code,
    message: "This isn't available in the adaptation audit.",
    retryable: false,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: null,
  };
}

const LOCAL_PLANE: Plane = {
  planeId: "local",
  kind: "local",
  label: "This computer",
  planeIdentity: null,
  connection: { state: "online" },
  coreVersion: "0.0.0-fixture",
  credential: null,
  security: "trusted",
  store: "serving",
  // The shell always lists every feature (`src-tauri/src/jet/planes/knowledge.rs`).
  features: (
    [
      "remote_login",
      "pairing",
      "capabilities",
      "conversation_pages",
      "projects",
      "workspaces",
      "search",
      "run_supervision",
      "workspace_terminals",
      "approval_retry",
      "git_delivery",
    ] as const
  ).map((feature) => ({ feature, requiredMinor: 1, support: "supported" as const })),
  protocol: { exact: 43, atLeast: 43, atMost: 43 },
};

const hasTask = scene === "approval-run" || scene === "changes";

function planes(): PlanesSnapshot {
  return {
    planes: [LOCAL_PLANE],
    identity: { clientId: "00000000-0000-4000-8000-00000000000c", key: "present", fingerprint: null },
    restoredSelection: hasTask ? { planeId: "local", conversationId: CONVERSATION.id } : null,
    notice: null,
    maximumRemotePlanes: 16,
  };
}

function presentation(): ShellPresentationView {
  const destination: ShellPresentation["destination"] =
    scene === "planes" || scene === "schedules" ? scene : scene === "new-task" ? "new-task" : "conversation";
  return {
    presentation: {
      version: 1,
      destination,
      sidebarPresented: true,
      workPanelPresented: true,
      workPanelTab: scene === "changes" ? "changes" : "run",
      sidebarWidth: 244,
      workPanelWidth: 340,
    },
    issue: null,
  };
}

function setup(): SetupSnapshot {
  const incomplete = scene === "setup";
  return {
    plane: { coreVersion: "0.0.0-fixture", daemonStarts: "1", platform: "linux" },
    capabilities: {
      harnesses: HARNESSES,
      crafts: HARNESSES.map((harness) => ({ id: harness, version: "1.0.0", harnesses: [harness] })),
      credentialStore: "available",
      credentialStoreLabel: "Secret Service",
      degraded: [],
      authProviders: HARNESSES.map((harness) => ({ provider: "openai", harness, label: "OpenAI" })),
    },
    projects: incomplete ? [] : [{ id: PROJECT.id, name: PROJECT.name, root: "/home/user/src/jet" }],
    accounts: incomplete
      ? []
      : [{ id: "00000000-0000-7000-8000-000000000040", label: "OpenAI", provider: "openai", state: "ready", stateLabel: "Ready" }],
    pairing: { gate: "closed", pairedClients: 0, offerPending: false },
    issues: [],
  };
}

function row(id: string, title: string, offset: number): ConversationRow {
  return {
    planeId: "local",
    id,
    revision: "1",
    title,
    createdAtUnixMs: String(Number(CREATED) - offset),
    projectId: PROJECT.id,
  };
}

const ROWS: ConversationRow[] = [
  row(CONVERSATION.id, scene === "changes" ? (completed.conversation?.title ?? CONVERSATION.title) : CONVERSATION.title, 0),
  row("00000000-0000-7000-8000-000000000021", "Tighten the Plane reconnect copy", 3_600_000),
  row("00000000-0000-7000-8000-000000000022", "Review the release checklist", 86_400_000),
];

function run(): RunSummary {
  return {
    id: RUN_ID,
    conversationId: CONVERSATION.id,
    revision: "3",
    lifecycle: scene === "changes" ? "completed" : "active",
    title: "Run 1",
    createdAtUnixMs: CREATED,
    endedAtUnixMs: scene === "changes" ? CREATED : null,
  };
}

function detail(conversationId: string): ConversationDetail {
  const conversation = ROWS.find((candidate) => candidate.id === conversationId) ?? ROWS[0];
  return {
    conversation,
    cursor: "140",
    workspaceId: "00000000-0000-7000-8000-000000000050",
    workspaceRoot: "/home/user/src/jet",
    runs: conversation.id === CONVERSATION.id ? [run()] : [],
    retention: "retain",
  };
}

function supervision(): RunSupervision {
  const active = scene === "approval-run";
  return {
    cursor: "140",
    maximumEntries: 128,
    maximumPromptBytes: 65536,
    turns: active
      ? [
          {
            id: "00000000-0000-7000-8000-000000000060",
            sequence: "1",
            position: 0,
            source: "user",
            state: "active",
            runId: RUN_ID,
            target: "Current Run",
            withdrawable: false,
          },
        ]
      : [],
    execution: {
      cursor: "140",
      run: run(),
      activity: active ? "waiting_for_approval" : null,
      needsAttention: false,
      termination: null,
    },
  };
}

const PATCH = [
  "diff --git a/src/onboarding.ts b/src/onboarding.ts",
  "--- a/src/onboarding.ts",
  "+++ b/src/onboarding.ts",
  "@@ -1,3 +1,3 @@",
  " export const title = \"Welcome to Jet\";",
  "-export const body = \"Connect a Plane to start.\";",
  "+export const body = \"Choose a Project folder to start your first task.\";",
  " export const action = \"Continue\";",
  "",
].join("\n");

function workPanel(): WorkPanelSnapshot {
  return {
    runId: RUN_ID,
    scope: scene === "changes" ? "Final" : "Current",
    cursor: "188",
    totalFiles: 3,
    files: [
      { id: "file-1", path: "src/onboarding.ts", beforeSize: "96", afterSize: "118", status: "modified", origin: "agent", contentAvailable: true },
      { id: "file-2", path: "src/setup/first-run.ts", beforeSize: null, afterSize: "412", status: "added", origin: "agent", contentAvailable: true },
      { id: "file-3", path: "docs/old-setup.md", beforeSize: "820", afterSize: null, status: "deleted", origin: "user edit", contentAvailable: false },
    ],
    nextPage: null,
    patch: PATCH,
    patchTruncated: false,
    artifact: { availability: "stored", sha256: "0".repeat(64), size: String(PATCH.length) },
    artifactReadId: null,
    contentComplete: true,
    latestTurn: 1,
    workspaceId: "00000000-0000-7000-8000-000000000050",
    terminals: [],
    terminalIssue: null,
  };
}

/** The approval scene's timeline, sent on the feed like live Plane events. */
function timelineEvents(): PlaneUpdate[] {
  const source = scene === "changes" ? completed : approval;
  const items: TimelineItem[] = [
    { kind: "user", text: "Polish the first-run flow so a new user reaches a task quickly.", itemId: "user-1", approval: null },
    { kind: "agent", text: "I'll update the onboarding copy and the setup checks.", itemId: "agent-1", approval: null },
  ];
  for (const entry of source.timeline) {
    if (entry.kind === "approval") {
      items.push({
        kind: "approval",
        text: entry.text,
        itemId: entry.id,
        approval: {
          requestId: entry.id,
          reviewId: null,
          runId: RUN_ID,
          tool: "apply_patch",
          action: "Replace the onboarding copy",
          target: "src/onboarding.ts",
          scope: `${PROJECT.name} project`,
          consequence: "The file changes in the task's Workspace. Nothing is committed.",
          rationale: entry.text,
          state: "requested",
          canAuthorizeRetry: false,
        },
      });
    } else if (entry.kind === "result" || entry.kind === "agent") {
      items.push({ kind: entry.kind, text: entry.text, itemId: entry.id, approval: null });
    }
  }
  return [
    {
      type: "event",
      sequence: "141",
      recorded_at_unix_ms: CREATED,
      kind: "run.activity_changed",
      conversation_id: CONVERSATION.id,
      run_id: RUN_ID,
      timeline: items,
    },
  ];
}

type Feed = { onmessage: (update: PlaneUpdate) => void };
let feed: Feed | null = null;
let timelineSent = false;

function connection(): ConnectionSnapshot {
  return {
    state: "online",
    feedId: "adaptation-feed",
    planeId: "local",
    planeIdentity: null,
    health: { security: "trusted", store: "serving", ledger: "verified" },
    coreVersion: "0.0.0-fixture",
    daemonStarts: "1",
    startedAtUnixMs: CREATED,
    cursor: "140",
  };
}

/** Sends the scene's timeline once the task is selected, as the Plane would. */
function sendTimeline(): void {
  if (timelineSent || !feed || !hasTask) return;
  timelineSent = true;
  const target = feed;
  setTimeout(() => {
    for (const update of timelineEvents()) target.onmessage(update);
  }, 0);
}

function trash(): TrashView {
  return {
    planeId: "local",
    planeLabel: LOCAL_PLANE.label,
    cursor: "140",
    graceDays: 30,
    capped: false,
    entries: [
      {
        conversationId: ROWS[2].id,
        reason: "manual_forget",
        trashedAtUnixMs: String(Number(CREATED) - 86_400_000),
        expiresAtUnixMs: String(Number(CREATED) + 29 * 86_400_000),
        restorable: true,
      },
    ],
  };
}

function notifications(): NotificationSettings {
  return {
    preferences: { enabled: true, approvals: true, completion: true, failure: true, mutedPlanes: [] },
    error: null,
    planes: [{ planeId: "local", label: LOCAL_PLANE.label }],
  };
}

async function answer(command: string, args: Record<string, unknown>): Promise<unknown> {
  switch (command) {
    case "list_planes":
      return planes();
    case "load_shell_presentation":
      return presentation();
    case "save_shell_presentation":
      return { presentation: args.presentation, issue: null };
    case "load_desktop_preferences":
      return { reopenLastTask: scene !== "new-task" };
    case "load_setup":
      return setup();
    case "open_plane_feed":
      feed = args.onUpdate as Feed;
      return connection();
    case "close_plane_feed":
      return null;
    case "load_conversations": {
      const page: ConversationPage = { planeId: "local", cursor: "140", conversations: ROWS, nextPage: null };
      return page;
    }
    case "load_conversation":
      return detail(String(args.conversationId));
    case "load_run_supervision":
      sendTimeline();
      return supervision();
    case "load_work_panel":
      return workPanel();
    case "load_deliveries":
      return [] satisfies Delivery[];
    case "load_trash":
      return trash();
    case "load_trash_status":
      return { trash: null } satisfies TrashStatus;
    case "resolve_conversation_names":
      return ((args.conversationIds as string[] | undefined) ?? []).map((conversationId) => ({
        conversationId,
        title: ROWS.find((candidate) => candidate.id === conversationId)?.title ?? null,
      }));
    case "load_notification_settings":
      return notifications();
    case "watch_settings_navigation":
      return { generation: "1", target: { pane: "general", section: null } } satisfies SettingsNavigation;
    case "remember_settings_pane":
      return null;
    default:
      unmocked.push(command);
      throw unavailable("adaptation.not_mocked");
  }
}

mockWindows(scene === "settings" ? "settings" : "main");
mockIPC((command, args) => answer(command, (args ?? {}) as Record<string, unknown>));
