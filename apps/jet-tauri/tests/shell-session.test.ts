import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";

import type {
  ConversationPage,
  ConversationRow,
  PlaneUpdate,
  PublicError,
  RunSupervision,
  SetupSnapshot,
} from "../src/lib/jet/bridge";
import type { Plane, PlanesSnapshot } from "../src/lib/jet/planes";
import { DEFAULT_PRESENTATION, type ShellPresentation, type ShellPresentationView } from "../src/lib/jet/presentation";
import { DesktopSession, FLUSH_BEFORE_CLOSE_MS, type ShortcutEvent } from "../src/lib/features/shell/session.svelte";
import { ENABLED_SHORTCUTS } from "../src/lib/features/shell/shortcuts";
import { withActiveRun } from "./support/session";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

function failure(category: string, code: string): PublicError {
  return {
    category,
    code,
    message: `${code} message`,
    retryable: false,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: null,
  };
}

const LOCAL: Plane = {
  planeId: "local",
  kind: "local",
  label: "This computer",
  planeIdentity: null,
  connection: { state: "online" },
  coreVersion: "0.2.0",
  credential: null,
  security: "trusted",
  store: "serving",
  features: [],
  protocol: { exact: null, atLeast: 37, atMost: null },
};

function planes(restoredSelection: PlanesSnapshot["restoredSelection"] = null): PlanesSnapshot {
  return {
    planes: [LOCAL],
    identity: { clientId: "00000000-0000-4000-8000-00000000000c", key: "unknown", fingerprint: null, notice: null },
    restoredSelection,
    notice: null,
    maximumRemotePlanes: 16,
  };
}

function row(id: string, createdAtUnixMs = 1_000): ConversationRow {
  return { planeId: "local", id, revision: "1", title: `Task ${id}`, createdAtUnixMs: String(createdAtUnixMs), projectId: "p1" };
}

const SETUP: SetupSnapshot = {
  plane: { coreVersion: "0.2.0", daemonStarts: "1", platform: "linux" },
  capabilities: {
    harnesses: ["codex"],
    crafts: [{ id: "craft", version: "1", harnesses: ["codex"] }],
    credentialStore: "available",
    credentialStoreLabel: "Keyring",
    degraded: [],
    authProviders: [],
  },
  projects: [{ id: "p1", name: "Jet", root: "/work/jet" }],
  accounts: [{ id: "a1", label: "Codex", provider: "openai", state: "ready", stateLabel: "Ready" }],
  pairing: { gate: "closed", pairedClients: 0, offerPending: false },
  issues: [],
};

type Options = {
  restored?: PlanesSnapshot["restoredSelection"];
  reopenLastTask?: boolean | "fails";
  rows?: ConversationRow[];
  /** Holds the first Recent page until the test releases it. */
  holdRecent?: boolean;
  setup?: SetupSnapshot;
  /** The saved window layout, or a failed read. */
  presentation?: Partial<ShellPresentation> | "fails";
  /** Holds the saved layout until the test releases it. */
  holdPresentation?: boolean;
  /** Save results in order; `true` fails that save. */
  saveFails?: boolean[];
  /** Layout saves never answer. */
  saveHangs?: boolean;
  fullscreenFails?: boolean;
  /** `start_run` results in order; a failure is thrown, `null` succeeds. */
  startFails?: Array<PublicError | null>;
};

function harness(options: Options = {}) {
  const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
  const feeds: Array<Channel<PlaneUpdate>> = [];
  let rows = options.rows ?? [row("l1", 1_000), row("l2", 2_000)];
  let release: () => void = () => undefined;
  const held = options.holdRecent ? new Promise<void>((resolve) => (release = resolve)) : Promise.resolve();
  let created = 0;
  let fullscreen = false;
  let releasePresentation: () => void = () => undefined;
  const presentationHeld = options.holdPresentation
    ? new Promise<void>((resolve) => (releasePresentation = resolve))
    : Promise.resolve();
  const saveFails = [...(options.saveFails ?? [])];
  const startFails = [...(options.startFails ?? [])];
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC(async (command, args) => {
    const { onUpdate, ...plain } = (args ?? {}) as Record<string, unknown>;
    calls.push({ command, args: plain });
    switch (command) {
      case "open_plane_feed":
        feeds.push(onUpdate as Channel<PlaneUpdate>);
        return {
          state: "online",
          feedId: `feed-${feeds.length}`,
          planeId: "local",
          planeIdentity: null,
          health: { security: "trusted", store: "serving" },
          coreVersion: "0.2.0",
          daemonStarts: "1",
          startedAtUnixMs: "1",
          cursor: "40",
        };
      case "close_plane_feed":
        return null;
      case "list_planes":
        return planes(options.restored ?? null);
      case "load_desktop_preferences":
        if (options.reopenLastTask === "fails") throw failure("internal", "preferences.read_failed");
        return { reopenLastTask: options.reopenLastTask ?? true };
      case "load_setup":
        return options.setup ?? SETUP;
      case "load_shell_presentation": {
        await presentationHeld;
        if (options.presentation === "fails") throw failure("internal", "client.state_unavailable");
        const view: ShellPresentationView = {
          presentation: { ...DEFAULT_PRESENTATION, ...options.presentation },
          issue: null,
        };
        return view;
      }
      case "save_shell_presentation": {
        if (options.saveHangs) await new Promise(() => undefined);
        if (saveFails.shift()) throw failure("internal", "presentation.write_failed");
        return { presentation: plain.presentation, issue: null };
      }
      case "toggle_main_window_fullscreen":
        if (options.fullscreenFails) throw failure("internal", "window.mode_unavailable");
        fullscreen = !fullscreen;
        return { fullscreen };
      case "close_main_window":
      case "quit_jet":
        return null;
      case "load_conversations": {
        await held;
        const page: ConversationPage = { planeId: "local", cursor: "40", conversations: rows, nextPage: null };
        return page;
      }
      case "load_conversation":
        return { conversation: row(plain.conversationId as string), cursor: "40", workspaceId: null, workspaceRoot: null, runs: [] };
      case "load_run_supervision":
        return { cursor: "40", maximumEntries: 128, maximumPromptBytes: 65536, turns: [], execution: null };
      case "create_conversation": {
        created += 1;
        const conversation = row(`new${created}`, 9_000 + created);
        rows = [...rows, conversation];
        return conversation;
      }
      case "start_run": {
        const failed = startFails.shift();
        if (failed) throw failed;
        return { runId: "run-new", message: "Started" };
      }
      case "submit_turn":
        return { message: "Queued" };
      case "load_trash_banner":
      case "load_conversation_trash":
        return { kind: "absent" };
      default:
        return null;
    }
  });
  return { calls, feeds, release: () => release(), releasePresentation: () => releasePresentation() };
}

async function flush() {
  for (let tick = 0; tick < 30; tick += 1) await Promise.resolve();
}

async function settle(session: DesktopSession) {
  for (let round = 0; round < 3; round += 1) {
    for (let tick = 0; tick < 30; tick += 1) await Promise.resolve();
    await session.catalog.settled("local");
  }
  for (let tick = 0; tick < 30; tick += 1) await Promise.resolve();
}

function event(kind: string, conversationId: string | null = null): PlaneUpdate {
  return {
    type: "event",
    sequence: "41",
    recorded_at_unix_ms: "1",
    kind,
    conversation_id: conversationId,
    run_id: null,
    timeline: [],
  };
}

function keyEvent(input: Partial<ShortcutEvent> & { key: string }) {
  const prevented = { value: false };
  const event: ShortcutEvent = {
    code: "",
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    isComposing: false,
    repeat: false,
    defaultPrevented: false,
    preventDefault: () => (prevented.value = true),
    ...input,
  };
  return { event, prevented };
}

const linux = { platform: "other" as const, modalOpen: false, enabled: ENABLED_SHORTCUTS };

describe("D17: a Plane event never selects a task behind New task", () => {
  it("startup restoration yields to New task chosen before Recent loads", async () => {
    const { feeds, release } = harness({ holdRecent: true });
    const session = new DesktopSession();
    session.connect();
    for (let tick = 0; tick < 30; tick += 1) await Promise.resolve();
    session.select("new-task");
    release();
    await settle(session);
    expect(session.sidebarSelection).toBe("new-task");
    expect(session.selectedConversationId).toBeNull();

    feeds[0]?.onmessage({ type: "resumed" } as PlaneUpdate);
    await settle(session);
    expect(session.sidebarSelection).toBe("new-task");
    expect(session.selectedConversationId).toBeNull();
  });

  it("a conversation.created event on New task leaves nothing selected", async () => {
    vi.useFakeTimers();
    const { feeds, release } = harness({ holdRecent: true });
    const session = new DesktopSession();
    session.connect();
    for (let tick = 0; tick < 30; tick += 1) await Promise.resolve();
    session.select("new-task");
    feeds[0]?.onmessage(event("conversation.created", "l3"));
    release();
    await vi.advanceTimersByTimeAsync(1_000);
    await settle(session);
    expect(session.sidebarSelection).toBe("new-task");
    expect(session.selectedConversationId).toBeNull();
  });

  it("Send on New task creates a task after a resumed update", async () => {
    const { calls, feeds, release } = harness({ holdRecent: true });
    const session = new DesktopSession();
    session.connect();
    for (let tick = 0; tick < 30; tick += 1) await Promise.resolve();
    session.select("new-task");
    release();
    await settle(session);
    feeds[0]?.onmessage({ type: "resumed" } as PlaneUpdate);
    await settle(session);

    session.draft = "Fix the build";
    await session.submitDraft();
    expect(calls.filter((call) => call.command === "create_conversation")).toEqual([
      { command: "create_conversation", args: { projectId: "p1", attempt: expect.any(String) } },
    ]);
    const sends = calls.filter((call) => call.command === "start_run" || call.command === "submit_turn");
    expect(sends).toHaveLength(1);
    expect(sends[0]).toMatchObject({ command: "start_run", args: { conversationId: "new1" } });
    expect(session.selection).toEqual({ planeId: "local", conversationId: "new1" });
    expect(session.sidebarSelection).toBe("conversation");
  });

  it("Send on New task ignores a stale selected task", async () => {
    const { calls } = harness();
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.selectedConversationId).toBe("l2");
    session.sidebarSelection = "new-task";
    session.supervision = {
      cursor: "40",
      maximumEntries: 128,
      maximumPromptBytes: 65536,
      turns: [],
      execution: null,
    } satisfies RunSupervision;

    session.draft = "Another task";
    await session.submitDraft();
    expect(calls.filter((call) => call.command === "create_conversation")).toHaveLength(1);
    expect(calls.filter((call) => call.command === "submit_turn")).toEqual([]);
    expect(calls.filter((call) => call.command === "start_run").map((call) => call.args.conversationId)).toEqual([
      "new1",
    ]);
  });

  it("with Reopen the last task off, startup lands on New task and stays there", async () => {
    const { calls, feeds } = harness({ reopenLastTask: false, restored: { planeId: "local", conversationId: "l1" } });
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.sidebarSelection).toBe("new-task");
    expect(session.selectedConversationId).toBeNull();
    expect(session.workPanelPresented).toBe(false);

    feeds[0]?.onmessage({ type: "resumed" } as PlaneUpdate);
    await settle(session);
    expect(session.sidebarSelection).toBe("new-task");
    expect(session.selectedConversationId).toBeNull();
    expect(calls.filter((call) => call.command === "load_conversation")).toEqual([]);
  });

  it("with the preference on, the newest task is still selected in the task view", async () => {
    harness();
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.sidebarSelection).toBe("conversation");
    expect(session.selection).toEqual({ planeId: "local", conversationId: "l2" });
  });

  it("an unreadable preference keeps the Swift default (reopen)", async () => {
    harness({ reopenLastTask: "fails", restored: { planeId: "local", conversationId: "l1" } });
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.selection).toEqual({ planeId: "local", conversationId: "l1" });
  });
});

describe("a Plane feed that dials again after a drop", () => {
  it("reads the selected task again: the feed says resumed, not connected", async () => {
    const { calls, feeds } = harness();
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.selection).toEqual({ planeId: "local", conversationId: "l2" });
    const reads = () => calls.filter((call) => call.command === "load_conversation").length;
    feeds[0]?.onmessage({ type: "resumed", after: "40" });
    await settle(session);
    const opened = reads();
    expect(session.conversationFreshness).toBe("live");

    feeds[0]?.onmessage({ type: "reconnecting", error: { ...failure("offline", "transport.offline"), retryable: true } });
    await settle(session);
    expect(session.conversationFreshness).toBe("cached");
    feeds[0]?.onmessage({ type: "resumed", after: "40" });
    await settle(session);
    expect(reads()).toBe(opened + 1);
    expect(session.conversationFreshness).toBe("live");
    expect(session.connectionState).toBe("online");
  });
});

describe("D2: a composer Send keeps its attempt only while it is retried", () => {
  const uncertain = failure("outcome_unknown", "command.outcome_unknown");

  function attempts(calls: Array<{ command: string; args: Record<string, unknown> }>) {
    return calls.filter((call) => call.command === "start_run").map((call) => call.args.attempt);
  }

  it("retries a failed Send under its attempt and sends later work under a new one", async () => {
    const { calls } = harness({ startFails: [uncertain, null, null] });
    const session = new DesktopSession();
    session.connect();
    await settle(session);

    session.draft = "continue";
    await session.submitDraft();
    expect(session.draft).toBe("continue");
    await session.submitDraft();
    expect(session.draft).toBe("");
    session.draft = "continue";
    await session.submitDraft();

    const [failed, retried, later] = attempts(calls);
    expect(failed).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(retried).toBe(failed);
    expect(later).not.toBe(retried);
  });

  it("an edited draft is a new Send, even when it is edited back", async () => {
    const { calls } = harness({ startFails: [uncertain, uncertain, null] });
    const session = new DesktopSession();
    session.connect();
    await settle(session);

    session.draft = "continue";
    await session.submitDraft();
    session.draft = "continue.";
    await session.submitDraft();
    session.draft = "continue";
    await session.submitDraft();

    const sent = attempts(calls);
    expect(new Set(sent).size).toBe(3);
  });
});

describe("Run-control confirmation", () => {
  it("opens the Run tab, asks for focus and remembers the invoker", () => {
    harness();
    const session = new DesktopSession();
    withActiveRun(session);
    session.hideWorkPanel();
    session.selectedWorkPanel = "delivery";
    const invoker = { focus: vi.fn(), isConnected: true };

    session.requestRunControl("interrupt_turn", invoker);
    expect(session.runControlConfirmation).toBe("interrupt_turn");
    expect(session.workPanelPresented).toBe(true);
    expect(session.selectedWorkPanel).toBe("run");
    expect(session.runControlFocusRequest).toBe(1);
    expect(session.takeFocusRequest("run-control")).toBe(true);
    expect(session.takeFocusRequest("run-control")).toBe(false);

    session.cancelRunControl();
    expect(session.runControlConfirmation).toBeNull();
    expect(invoker.focus).toHaveBeenCalledTimes(1);
  });

  it("does nothing when the control is not available", () => {
    harness();
    const session = new DesktopSession();
    session.hideWorkPanel();
    session.requestRunControl("stop_run", { focus: vi.fn() });
    expect(session.runControlConfirmation).toBeNull();
    expect(session.workPanelPresented).toBe(false);
    expect(session.runControlFocusRequest).toBe(0);
  });

  it("dismiss (Escape) cancels the confirmation and returns focus", () => {
    harness();
    const session = new DesktopSession();
    withActiveRun(session);
    const invoker = { focus: vi.fn(), isConnected: true };
    session.requestRunControl("stop_run", invoker);
    expect(session.dismissible).toBe(true);

    const { event, prevented } = keyEvent({ key: "Escape", code: "Escape" });
    session.handleShortcut(event, linux);
    expect(prevented.value).toBe(true);
    expect(session.runControlConfirmation).toBeNull();
    expect(invoker.focus).toHaveBeenCalledTimes(1);
    expect(session.dismissible).toBe(false);
  });

  it("does not return focus to a control that left the page", () => {
    harness();
    const session = new DesktopSession();
    withActiveRun(session);
    const invoker = { focus: vi.fn(), isConnected: false };
    session.requestRunControl("stop_run", invoker);
    session.dismiss();
    expect(invoker.focus).not.toHaveBeenCalled();
  });

  it("leaves Escape alone when nothing can be dismissed or a dialog is open", () => {
    harness();
    const session = new DesktopSession();
    const idle = keyEvent({ key: "Escape", code: "Escape" });
    session.handleShortcut(idle.event, linux);
    expect(idle.prevented.value).toBe(false);

    withActiveRun(session);
    session.requestRunControl("stop_run", null);
    const modal = keyEvent({ key: "Escape", code: "Escape" });
    session.handleShortcut(modal.event, { ...linux, modalOpen: true });
    expect(modal.prevented.value).toBe(false);
    expect(session.runControlConfirmation).toBe("stop_run");
  });
});

describe("handleShortcut", () => {
  it("Ctrl+K opens Search and asks it to take focus (D5)", () => {
    harness();
    const session = new DesktopSession();
    const { event, prevented } = keyEvent({ key: "k", code: "KeyK", ctrlKey: true });
    session.handleShortcut(event, linux);
    expect(prevented.value).toBe(true);
    expect(session.sidebarSelection).toBe("search");
    expect(session.takeFocusRequest("search")).toBe(true);
  });

  it("Ctrl+Shift+O opens Setup and asks Choose Folder… to take focus", () => {
    harness();
    const session = new DesktopSession();
    session.handleShortcut(keyEvent({ key: "O", code: "KeyO", ctrlKey: true, shiftKey: true }).event, linux);
    expect(session.sidebarSelection).toBe("project");
    expect(session.takeFocusRequest("project-folder")).toBe(true);
  });

  it("a pending focus request is dropped when the user moves elsewhere", () => {
    harness();
    const session = new DesktopSession();
    session.handleShortcut(keyEvent({ key: "k", code: "KeyK", ctrlKey: true }).event, linux);
    session.select("conversation");
    session.select("search");
    expect(session.takeFocusRequest("search")).toBe(false);
  });

  it("Ctrl+N does nothing while a modal dialog is open (D4)", () => {
    harness();
    const session = new DesktopSession();
    session.select("project");
    const { event, prevented } = keyEvent({ key: "n", code: "KeyN", ctrlKey: true });
    session.handleShortcut(event, { ...linux, modalOpen: true });
    expect(prevented.value).toBe(false);
    expect(session.sidebarSelection).toBe("project");
  });

  it("ignores key presses during IME composition and already-handled presses", () => {
    harness();
    const session = new DesktopSession();
    session.select("project");
    session.handleShortcut(keyEvent({ key: "n", code: "KeyN", ctrlKey: true, isComposing: true }).event, linux);
    session.handleShortcut(keyEvent({ key: "n", code: "KeyN", ctrlKey: true, defaultPrevented: true }).event, linux);
    expect(session.sidebarSelection).toBe("project");
  });

  it("F9 toggles the sidebar and Ctrl+Alt+digits choose work panel tabs", () => {
    harness();
    const session = new DesktopSession();
    session.handleShortcut(keyEvent({ key: "F9", code: "F9" }).event, linux);
    expect(session.sidebarPresented).toBe(false);
    session.handleShortcut(keyEvent({ key: "F9", code: "F9" }).event, linux);
    expect(session.sidebarPresented).toBe(true);

    session.hideWorkPanel();
    session.handleShortcut(keyEvent({ key: "3", code: "Digit3", ctrlKey: true, altKey: true }).event, linux);
    expect(session.selectedWorkPanel).toBe("terminal");
    expect(session.workPanelPresented).toBe(true);
    session.handleShortcut(keyEvent({ key: "0", code: "Digit0", ctrlKey: true, altKey: true }).event, linux);
    expect(session.workPanelPresented).toBe(false);
  });

  it("Ctrl+, opens the Settings window (Wave 3.2)", async () => {
    const { calls } = harness();
    const session = new DesktopSession();
    session.handleShortcut(keyEvent({ key: ",", code: "Comma", ctrlKey: true }).event, linux);
    for (let tick = 0; tick < 10; tick += 1) await Promise.resolve();
    expect(calls.map((call) => call.command)).toContain("open_settings");
  });

  it("does not claim bindings that have not shipped", () => {
    harness();
    const session = new DesktopSession();
    const { event, prevented } = keyEvent({ key: "q", code: "KeyQ", ctrlKey: true });
    session.handleShortcut(event, { ...linux, enabled: new Set(["search"]) });
    expect(prevented.value).toBe(false);
  });

  it("F11 toggles full screen natively and announces the result", async () => {
    const { calls } = harness();
    const session = new DesktopSession();
    const { event, prevented } = keyEvent({ key: "F11", code: "F11" });
    session.handleShortcut(event, linux);
    expect(prevented.value).toBe(true);
    await flush();
    expect(session.shellStatus).toBe("Full screen on");
    session.handleShortcut(keyEvent({ key: "F11", code: "F11" }).event, linux);
    await flush();
    expect(session.shellStatus).toBe("Full screen off");
    expect(calls.filter((call) => call.command === "toggle_main_window_fullscreen")).toHaveLength(2);
  });

  it("a full-screen failure shows copy keyed on its code", async () => {
    harness({ fullscreenFails: true });
    const session = new DesktopSession();
    session.handleShortcut(keyEvent({ key: "F11", code: "F11" }).event, linux);
    await flush();
    expect(session.actionNotice).toBe("Full screen isn't available in this window.");
    expect(session.shellStatus).toBe("");
  });

  it("Ctrl+W closes the window and Ctrl+Q quits Jet", async () => {
    const { calls } = harness();
    const session = new DesktopSession();
    session.handleShortcut(keyEvent({ key: "w", code: "KeyW", ctrlKey: true }).event, linux);
    session.handleShortcut(keyEvent({ key: "q", code: "KeyQ", ctrlKey: true }).event, linux);
    await flush();
    expect(calls.map((call) => call.command)).toEqual(["close_main_window", "quit_jet"]);
    // A held Ctrl+Q repeats nothing.
    session.handleShortcut(keyEvent({ key: "q", code: "KeyQ", ctrlKey: true, repeat: true }).event, linux);
    await flush();
    expect(calls.filter((call) => call.command === "quit_jet")).toHaveLength(1);
  });
});

describe("work panel presentation (layout model)", () => {
  const closing = ["new-task", "search", "project", "schedules", "trash", "planes"] as const;

  it("maps every destination in a regular window", () => {
    harness();
    const session = new DesktopSession();
    for (const destination of closing) {
      session.select("conversation");
      expect(session.panel.presentation).toEqual({ kind: "column" });
      session.select(destination);
      expect(session.panel).toEqual({ mode: "regular", columnPreference: false, presentation: { kind: "hidden" } });
    }
    session.select("attention");
    expect(session.panel.presentation).toEqual({ kind: "column" });
    expect(session.selectedWorkPanel).toBe("run");
  });

  it("never opens the overlay on its own in a compact window (D12)", () => {
    harness();
    const session = new DesktopSession();
    session.setLayoutMode("compact");
    expect(session.workPanelPresented).toBe(false);
    session.select("conversation");
    expect(session.workPanelPresented).toBe(false);
    for (const destination of closing) {
      session.select(destination);
      expect(session.workPanelPresented).toBe(false);
    }
    // Compact destinations leave the regular-window preference alone.
    expect(session.panel.columnPreference).toBe(true);
    session.setLayoutMode("regular");
    expect(session.panel.presentation).toEqual({ kind: "column" });
  });

  it("Needs attention opens the overlay on the Run tab in a compact window", () => {
    harness();
    const session = new DesktopSession();
    session.setLayoutMode("compact");
    session.selectedWorkPanel = "files";
    session.select("attention");
    expect(session.panel.presentation).toEqual({ kind: "overlay", origin: "attention" });
    expect(session.selectedWorkPanel).toBe("run");
    expect(session.takeFocusRequest("work-panel")).toBe(true);
    session.select("new-task");
    expect(session.workPanelPresented).toBe(false);
    // Moving elsewhere is not a close by the user: focus goes to the composer.
    expect(session.takeFocusRequest("work-panel-return")).toBe(false);
  });

  it("the setup redirect and choosing a Project close the panel", async () => {
    harness({ setup: { ...SETUP, projects: [] } });
    const session = new DesktopSession();
    expect(session.workPanelPresented).toBe(true);
    await session.refreshSetup(true);
    expect(session.sidebarSelection).toBe("project");
    expect(session.panel.columnPreference).toBe(false);
    expect(session.workPanelPresented).toBe(false);
  });

  it("choosing a Project closes the panel", async () => {
    harness();
    const session = new DesktopSession();
    await session.refreshSetup();
    session.selectProject("p1");
    expect(session.panel).toEqual({ mode: "regular", columnPreference: false, presentation: { kind: "hidden" } });
  });

  it("opening a task shows the column, but not the overlay", async () => {
    harness();
    const session = new DesktopSession();
    session.select("new-task");
    await session.openConversation("l1", true, "local");
    expect(session.panel.presentation).toEqual({ kind: "column" });

    session.select("new-task");
    session.setLayoutMode("compact");
    await session.openConversation("l2", true, "local");
    expect(session.workPanelPresented).toBe(false);
  });

  it("a tab chosen from code never covers the conversation; a user's choice does", () => {
    harness();
    const session = new DesktopSession();
    session.setLayoutMode("compact");
    session.showPanel("changes");
    expect(session.workPanelPresented).toBe(false);
    expect(session.selectedWorkPanel).toBe("changes");
    session.handleShortcut(keyEvent({ key: "2", code: "Digit2", ctrlKey: true, altKey: true }).event, linux);
    expect(session.panel.presentation).toEqual({ kind: "overlay", origin: "shortcut" });
    expect(session.selectedWorkPanel).toBe("files");
  });

  it("the header toggle opens the overlay and closing it returns focus to the toggle", () => {
    harness();
    const session = new DesktopSession();
    session.setLayoutMode("compact");
    const toggle = { focus: vi.fn(), isConnected: true };
    session.toggleWorkPanel("toggle", toggle);
    expect(session.panel.presentation).toEqual({ kind: "overlay", origin: "toggle" });
    expect(session.dismissible).toBe(true);
    expect(session.takeFocusRequest("work-panel")).toBe(true);

    session.toggleWorkPanel("toggle", toggle);
    expect(session.workPanelPresented).toBe(false);
    expect(session.takeFocusRequest("work-panel-return")).toBe(true);
    expect(session.returnWorkPanelFocus()).toBe(true);
    expect(toggle.focus).toHaveBeenCalledTimes(1);
    // One return per close.
    expect(session.returnWorkPanelFocus()).toBe(false);
  });

  it("Escape closes the overlay opened by Ctrl+Alt+0 and returns to where the key was pressed", () => {
    harness();
    const session = new DesktopSession();
    session.setLayoutMode("compact");
    const composer = { focus: vi.fn(), isConnected: true };
    session.handleShortcut(keyEvent({ key: "0", code: "Digit0", ctrlKey: true, altKey: true, target: composer as unknown as EventTarget }).event, linux);
    expect(session.workPanelOverlay).toBe(true);

    const escape = keyEvent({ key: "Escape", code: "Escape" });
    session.handleShortcut(escape.event, linux);
    expect(escape.prevented.value).toBe(true);
    expect(session.workPanelOverlay).toBe(false);
    expect(session.panel.columnPreference).toBe(true);
    expect(session.takeFocusRequest("work-panel-return")).toBe(true);
    expect(session.returnWorkPanelFocus()).toBe(true);
    expect(composer.focus).toHaveBeenCalledTimes(1);
  });

  it("falls back when the control that opened the overlay left the page", () => {
    harness();
    const session = new DesktopSession();
    session.setLayoutMode("compact");
    session.toggleWorkPanel("toggle", { focus: vi.fn(), isConnected: false });
    session.hideWorkPanel();
    expect(session.takeFocusRequest("work-panel-return")).toBe(true);
    expect(session.returnWorkPanelFocus()).toBe(false);
  });

  it("a Run control opens the overlay on Run; Escape closes the overlay before the confirmation", () => {
    harness();
    const session = new DesktopSession();
    withActiveRun(session);
    session.setLayoutMode("compact");
    const card = { focus: vi.fn(), isConnected: true };
    session.requestRunControl("interrupt_turn", card);
    expect(session.panel.presentation).toEqual({ kind: "overlay", origin: "run-control" });
    expect(session.selectedWorkPanel).toBe("run");
    // Cancel takes focus, not the tab.
    expect(session.takeFocusRequest("run-control")).toBe(true);
    expect(session.takeFocusRequest("work-panel")).toBe(false);

    session.dismiss();
    expect(session.workPanelOverlay).toBe(false);
    expect(session.runControlConfirmation).toBe("interrupt_turn");
    expect(session.takeFocusRequest("work-panel-return")).toBe(true);
    expect(session.returnWorkPanelFocus()).toBe(true);
    expect(card.focus).toHaveBeenCalledTimes(1);

    session.dismiss();
    expect(session.runControlConfirmation).toBeNull();
    expect(card.focus).toHaveBeenCalledTimes(2);
  });

  it("cancelling a confirmation inside the overlay keeps focus in the overlay", () => {
    harness();
    const session = new DesktopSession();
    withActiveRun(session);
    session.setLayoutMode("compact");
    const card = { focus: vi.fn(), isConnected: true };
    session.requestRunControl("stop_run", card);
    session.cancelRunControl();
    expect(session.runControlConfirmation).toBeNull();
    expect(session.workPanelOverlay).toBe(true);
    expect(card.focus).not.toHaveBeenCalled();
    expect(session.takeFocusRequest("work-panel")).toBe(true);
    // The overlay still returns focus to the card when it closes.
    session.hideWorkPanel();
    expect(session.returnWorkPanelFocus()).toBe(true);
    expect(card.focus).toHaveBeenCalledTimes(1);
  });

  it("the sidebar changes only through applySidebar", () => {
    harness();
    const session = new DesktopSession();
    session.applySidebar(false);
    expect(session.sidebarPresented).toBe(false);
    session.toggleSidebar();
    expect(session.sidebarPresented).toBe(true);
  });

  it("keeps requested column widths inside the Swift ranges", () => {
    harness();
    const session = new DesktopSession();
    expect([session.sidebarWidth, session.workPanelWidth]).toEqual([244, 340]);
    session.setColumnWidth("sidebar", 900);
    session.setColumnWidth("work-panel", 12);
    expect([session.sidebarWidth, session.workPanelWidth]).toEqual([300, 280]);
  });
});

describe("window layout: startup ordering", () => {
  const INCOMPLETE: SetupSnapshot = { ...SETUP, projects: [] };

  it("restores destination, tab, panel, sidebar and widths when Reopen the last task is on", async () => {
    const { calls } = harness({
      restored: { planeId: "local", conversationId: "l1" },
      presentation: {
        destination: "schedules",
        sidebarPresented: false,
        workPanelPresented: false,
        workPanelTab: "files",
        sidebarWidth: 280,
        workPanelWidth: 400,
      },
    });
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.sidebarSelection).toBe("schedules");
    expect(session.sidebarPresented).toBe(false);
    expect(session.selectedWorkPanel).toBe("files");
    expect(session.panel.columnPreference).toBe(false);
    expect([session.sidebarWidth, session.workPanelWidth]).toEqual([280, 400]);
    expect(session.presentation.kind).toBe("ready");
    // Only the task view restores a task.
    expect(session.selectedConversationId).toBeNull();
    expect(calls.filter((call) => call.command === "load_conversation")).toEqual([]);
  });

  it("restores the task view with its task and a hidden panel", async () => {
    harness({
      restored: { planeId: "local", conversationId: "l1" },
      presentation: { destination: "conversation", workPanelPresented: false, workPanelTab: "changes" },
    });
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.sidebarSelection).toBe("conversation");
    expect(session.selection).toEqual({ planeId: "local", conversationId: "l1" });
    expect(session.workPanelPresented).toBe(false);
    expect(session.selectedWorkPanel).toBe("changes");
  });

  it("an incomplete setup wins over the restored destination", async () => {
    harness({ setup: INCOMPLETE, presentation: { destination: "planes", workPanelPresented: true } });
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.sidebarSelection).toBe("project");
    expect(session.workPanelPresented).toBe(false);
  });

  it("with Reopen the last task off: New task, default tab, panel hidden, nothing selected", async () => {
    const { calls } = harness({
      reopenLastTask: false,
      restored: { planeId: "local", conversationId: "l1" },
      presentation: {
        destination: "planes",
        workPanelTab: "terminal",
        workPanelPresented: true,
        sidebarPresented: false,
        sidebarWidth: 300,
      },
    });
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.sidebarSelection).toBe("new-task");
    expect(session.selectedWorkPanel).toBe("run");
    expect(session.workPanelPresented).toBe(false);
    expect(session.selectedConversationId).toBeNull();
    expect(calls.filter((call) => call.command === "load_conversation")).toEqual([]);
    // The sidebar and widths are layout, not task restoration.
    expect(session.sidebarPresented).toBe(false);
    expect(session.sidebarWidth).toBe(300);
  });

  it("an unreadable layout starts with the defaults", async () => {
    harness({ presentation: "fails" });
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    expect(session.presentation).toMatchObject({ kind: "defaulted", error: { code: "client.state_unavailable" } });
    expect(session.sidebarSelection).toBe("conversation");
    expect(session.sidebarPresented).toBe(true);
    expect([session.sidebarWidth, session.workPanelWidth]).toEqual([244, 340]);
  });

  it("a late layout does not overwrite what the user did meanwhile", async () => {
    const { calls, releasePresentation } = harness({
      holdPresentation: true,
      setup: INCOMPLETE,
      restored: { planeId: "local", conversationId: "l1" },
      presentation: { destination: "schedules", sidebarPresented: false, workPanelTab: "files", workPanelWidth: 420 },
    });
    const session = new DesktopSession();
    session.connect();
    await flush();
    expect(session.presentation.kind).toBe("loading");
    session.select("new-task");
    session.showPanel("delivery", "tab");
    releasePresentation();
    await settle(session);
    expect(session.sidebarSelection).toBe("new-task");
    expect(session.sidebarPresented).toBe(true);
    expect(session.selectedWorkPanel).toBe("delivery");
    // Untouched widths still come back, and the setup redirect yields too.
    expect(session.workPanelWidth).toBe(420);
    expect(session.selectedConversationId).toBeNull();
    expect(calls.filter((call) => call.command === "load_conversation")).toEqual([]);
  });

  it("a width the user set before the layout arrived is kept", async () => {
    const { releasePresentation } = harness({ holdPresentation: true, presentation: { sidebarWidth: 290 } });
    const session = new DesktopSession();
    session.connect();
    await flush();
    session.setColumnWidth("sidebar", 220);
    releasePresentation();
    await settle(session);
    expect(session.sidebarWidth).toBe(220);
  });
});

describe("window layout: persistence", () => {
  function saves(calls: Array<{ command: string; args: Record<string, unknown> }>) {
    return calls
      .filter((call) => call.command === "save_shell_presentation")
      .map((call) => call.args.presentation as ShellPresentation);
  }

  it("saves nothing while the layout is loading", async () => {
    const { calls } = harness({ holdPresentation: true });
    const session = new DesktopSession();
    session.connect();
    await flush();
    session.select("planes");
    expect(session.presentationKey).toBeNull();
    await session.persistPresentation();
    expect(saves(calls)).toEqual([]);
  });

  it("saves a changed layout once and never the overlay, Search or Needs attention", async () => {
    const { calls } = harness();
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    await session.persistPresentation();
    expect(saves(calls)).toEqual([]);

    // Search and Needs attention reopen as the task view.
    session.select("search");
    await session.persistPresentation();
    session.select("attention");
    await session.persistPresentation();
    expect(saves(calls)).toEqual([
      { ...DEFAULT_PRESENTATION, destination: "conversation", workPanelPresented: false },
      { ...DEFAULT_PRESENTATION, destination: "conversation", workPanelPresented: true },
    ]);

    session.select("planes");
    session.setColumnWidth("work-panel", 999);
    await session.persistPresentation();
    await session.persistPresentation();
    expect(saves(calls).slice(2)).toEqual([
      { ...DEFAULT_PRESENTATION, destination: "planes", workPanelPresented: false, workPanelWidth: 440 },
    ]);

    // The compact overlay leaves the column preference alone.
    session.select("conversation");
    session.setLayoutMode("compact");
    await session.persistPresentation();
    session.toggleWorkPanel("toggle");
    expect(session.workPanelOverlay).toBe(true);
    await session.persistPresentation();
    expect(saves(calls).slice(3)).toEqual([
      { ...DEFAULT_PRESENTATION, destination: "conversation", workPanelPresented: true, workPanelWidth: 440 },
    ]);
  });

  it("announces the first failure of a streak and retries only a changed layout", async () => {
    const { calls } = harness({ saveFails: [true, true, false] });
    const session = new DesktopSession();
    session.connect();
    await settle(session);

    session.select("planes");
    await session.persistPresentation();
    expect(session.shellStatus).toBe("Jet couldn't save the window layout. It'll try again when the layout changes.");
    await session.persistPresentation();
    expect(saves(calls)).toHaveLength(1);

    session.shellStatus = "";
    session.toggleSidebar();
    await session.persistPresentation();
    expect(saves(calls)).toHaveLength(2);
    expect(session.shellStatus).toBe("");

    session.toggleSidebar();
    session.select("schedules");
    await session.persistPresentation();
    expect(saves(calls)).toHaveLength(3);
    expect(session.presentation).toMatchObject({ kind: "ready", value: { destination: "schedules" } });
  });

  it("Ctrl+Q and Ctrl+W save a layout change still waiting for the page's debounce first", async () => {
    const { calls } = harness();
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    session.toggleSidebar();
    session.handleShortcut(keyEvent({ key: "q", code: "KeyQ", ctrlKey: true }).event, linux);
    await flush();
    const commands = calls.map((call) => call.command).filter((command) => command === "save_shell_presentation" || command === "quit_jet");
    expect(commands).toEqual(["save_shell_presentation", "quit_jet"]);
    expect(saves(calls)).toEqual([{ ...DEFAULT_PRESENTATION, sidebarPresented: false }]);

    // Nothing pending: Ctrl+W closes without another save.
    session.handleShortcut(keyEvent({ key: "w", code: "KeyW", ctrlKey: true }).event, linux);
    await flush();
    expect(saves(calls)).toHaveLength(1);
    expect(calls.some((call) => call.command === "close_main_window")).toBe(true);
  });

  it("a layout save that never answers does not block quitting", async () => {
    const { calls } = harness({ saveHangs: true });
    const session = new DesktopSession();
    session.connect();
    await settle(session);
    vi.useFakeTimers();
    session.select("planes");
    session.handleShortcut(keyEvent({ key: "q", code: "KeyQ", ctrlKey: true }).event, linux);
    await flush();
    expect(calls.some((call) => call.command === "quit_jet")).toBe(false);
    await vi.advanceTimersByTimeAsync(FLUSH_BEFORE_CLOSE_MS);
    await flush();
    expect(calls.some((call) => call.command === "quit_jet")).toBe(true);
  });
});
