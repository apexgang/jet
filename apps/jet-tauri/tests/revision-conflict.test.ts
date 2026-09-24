import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type {
  ConversationPage,
  ConversationRow,
  PublicError,
  PublicRevisionConflict,
  RunSummary,
} from "../src/lib/jet/bridge";
import type { PlanesSnapshot } from "../src/lib/jet/planes";
import { DEFAULT_PRESENTATION } from "../src/lib/jet/presentation";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";
import { withActiveRun } from "./support/session";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function row(id: string, revision: string | null = "1"): ConversationRow {
  return { planeId: "local", id, revision, title: `Task ${id}`, createdAtUnixMs: "1", projectId: "p1" };
}

function run(id: string, revision: string): RunSummary {
  return {
    id,
    conversationId: "l1",
    revision,
    lifecycle: "active",
    title: "Run",
    createdAtUnixMs: "1",
    endedAtUnixMs: null,
  };
}

function conflict(revisionConflict: PublicRevisionConflict): PublicError {
  return {
    category: "conflict",
    code: "run.revision_conflict",
    message: "The Plane changed before the request completed.",
    retryable: false,
    recoveryActions: [],
    restart: null,
    revisionConflict,
    protocolLimit: null,
    planeId: "local",
  };
}

const PLANES: PlanesSnapshot = {
  planes: [
    {
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
    },
  ],
  identity: { clientId: "00000000-0000-4000-8000-00000000000c", key: "unknown", fingerprint: null, notice: null },
  restoredSelection: null,
  notice: null,
  maximumRemotePlanes: 16,
};

/** A session showing task l1 with Run run-1 at revision 3 and one open file. */
async function session(saveFails: PublicError): Promise<DesktopSession> {
  vi.stubGlobal("window", { crypto: globalThis.crypto });
  mockIPC(async (command) => {
    switch (command) {
      case "open_plane_feed":
        return {
          state: "online",
          feedId: "feed-1",
          planeId: "local",
          planeIdentity: null,
          health: { security: "trusted", store: "serving" },
          coreVersion: "0.2.0",
          daemonStarts: "1",
          startedAtUnixMs: "1",
          cursor: "40",
        };
      case "list_planes":
        return PLANES;
      case "load_desktop_preferences":
        return { reopenLastTask: false };
      case "load_shell_presentation":
        return { presentation: DEFAULT_PRESENTATION, issue: null };
      case "load_conversations": {
        const page: ConversationPage = {
          planeId: "local",
          cursor: "40",
          conversations: [row("l1", "5"), row("l2", "8")],
          nextPage: null,
        };
        return page;
      }
      case "save_work_file":
        throw saveFails;
      default:
        return null;
    }
  });
  const desktop = new DesktopSession();
  desktop.connect();
  for (let round = 0; round < 3; round += 1) {
    for (let tick = 0; tick < 30; tick += 1) await Promise.resolve();
    await desktop.catalog.settled("local");
  }
  desktop.conversationDetail = {
    conversation: row("l1", "5"),
    cursor: "40",
    workspaceId: null,
    workspaceRoot: null,
    runs: [run("run-1", "3"), run("run-0", "2")],
    retention: "retain",
  };
  withActiveRun(desktop);
  desktop.selectedWorkFileId = "file-1";
  desktop.editableFile = { fileId: "file-1", path: "src/lib.rs", content: "old", contentBytes: 3, revision: "r1" };
  desktop.fileDraft = "new";
  return desktop;
}

describe("a refusal with a Revision conflict replaces the stale copy with the safe state", () => {
  it("a Run conflict updates that Run in the task and in supervision, and nothing else", async () => {
    const desktop = await session(
      conflict({
        currentRevision: "9",
        safeState: { type: "run", runId: "run-1", conversationId: "l1", revision: "9", lifecycle: "stopping" },
      }),
    );
    await desktop.saveSelectedFile();

    expect(desktop.conversationDetail?.runs).toEqual([
      { ...run("run-1", "9"), lifecycle: "stopping" },
      run("run-0", "2"),
    ]);
    expect(desktop.supervision?.execution?.run).toMatchObject({ id: "run-1", revision: "9", lifecycle: "stopping" });
    expect(desktop.workPanelNoticeError?.revisionConflict?.currentRevision).toBe("9");
    // The conflict names a Run, so the task's own revision is untouched.
    expect(desktop.conversationDetail?.conversation.revision).toBe("5");
    // The draft survives for the user to reapply after the refresh.
    expect(desktop.fileDraft).toBe("new");
  });

  it("a Conversation conflict updates the task in Recent and in the open task", async () => {
    const desktop = await session(
      conflict({
        currentRevision: "12",
        safeState: { type: "conversation", conversationId: "l1", revision: "12" },
      }),
    );
    await desktop.saveSelectedFile();

    expect(desktop.catalog.find("local", "l1")?.revision).toBe("12");
    expect(desktop.catalog.find("local", "l2")?.revision).toBe("8");
    expect(desktop.conversationDetail?.conversation.revision).toBe("12");
    expect(desktop.conversationDetail?.runs.map((entry) => entry.revision)).toEqual(["3", "2"]);
    expect(desktop.supervision?.execution?.run.revision).toBe("3");
  });

  it("a refusal without a Revision conflict changes no revision", async () => {
    const desktop = await session({
      ...conflict({
        currentRevision: "0",
        safeState: { type: "conversation", conversationId: "l1", revision: null },
      }),
      code: "user_edit.stale_revision",
      revisionConflict: null,
    });
    await desktop.saveSelectedFile();

    expect(desktop.workPanelNoticeError?.code).toBe("user_edit.stale_revision");
    expect(desktop.catalog.find("local", "l1")?.revision).toBe("5");
    expect(desktop.conversationDetail?.runs[0]?.revision).toBe("3");
  });
});
