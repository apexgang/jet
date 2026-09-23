import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { PublicError } from "../src/lib/jet/bridge";
import type { TrashEntry, TrashPreview } from "../src/lib/jet/retention";
import { TrashSession, type TrashHost } from "../src/lib/features/trash/session.svelte";

const REMOTE = "0000000a-0000-4000-8000-000000000002";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

type Handler = (command: string, args: Record<string, unknown>) => unknown;

function ipc(handler: Handler) {
  vi.stubGlobal("window", {});
  mockIPC((command, args) => handler(command, (args ?? {}) as Record<string, unknown>));
}

function failure(code: string, category = "conflict"): PublicError {
  return {
    category,
    code,
    message: "Refused.",
    retryable: category === "offline",
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId: null,
  };
}

const ENTRY: TrashEntry = {
  conversationId: "c1",
  reason: "manual_forget",
  trashedAtUnixMs: "1",
  expiresAtUnixMs: "2592000001",
  restorable: true,
};

function preview(overrides: Partial<TrashPreview> = {}): TrashPreview {
  return {
    reviewId: "review-1",
    conversationId: "c1",
    title: "Fix login",
    planeLabel: "Build box",
    protections: [],
    workspaceUnchecked: false,
    trash: null,
    auditRecords: "0",
    graceDays: 30,
    activeRun: false,
    pendingTurn: false,
    stopAcknowledgementRequired: false,
    ...overrides,
  };
}

function session(outcomes: Array<[string, string | null]> = []) {
  const host: TrashHost = {
    planeIds: () => ["local", REMOTE],
    knownTitle: () => null,
    observe: (planeId, error) => outcomes.push([planeId, error?.code ?? null]),
  };
  return new TrashSession(host);
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}

describe("Move to Trash dialog", () => {
  it("opens with Forget selected and sends only the review, mode and acknowledgement", async () => {
    const sent: unknown[] = [];
    const outcomes: Array<[string, string | null]> = [];
    ipc((command, args) => {
      if (command === "preview_trash") {
        expect(args).toEqual({ planeId: REMOTE, conversationId: "c1" });
        return preview();
      }
      if (command === "trash_conversation") {
        sent.push(args);
        return { kind: "trashed", entry: ENTRY };
      }
      if (command === "load_trash_status") return { trash: ENTRY };
      throw new Error(`Unexpected ${command}`);
    });
    const trash = session(outcomes);
    await trash.openMove(REMOTE, "c1");
    expect(trash.dialog).toMatchObject({ kind: "review", mode: "forget", stopAcknowledged: false });
    expect(trash.canConfirm).toBe(true);
    await trash.confirm();
    expect(sent).toEqual([{ planeId: REMOTE, reviewId: "review-1", mode: "forget", acknowledgeStop: false }]);
    expect(trash.dialog).toMatchObject({ kind: "trashed", entry: ENTRY });
    expect(outcomes).toEqual([[REMOTE, null]]);
  });

  it("blocks Delete everywhere until the stop is acknowledged", async () => {
    const sent: unknown[] = [];
    ipc((command, args) => {
      if (command === "preview_trash") {
        return preview({ activeRun: true, protections: [{ kind: "active_run", label: "Activity in progress" }], stopAcknowledgementRequired: true });
      }
      if (command === "trash_conversation") {
        sent.push(args);
        return { kind: "trashed", entry: { ...ENTRY, reason: "delete_everywhere" } };
      }
      return null;
    });
    const trash = session();
    await trash.openMove(REMOTE, "c1");
    trash.chooseMode("delete_everywhere");
    expect(trash.canConfirm).toBe(false);
    await trash.confirm();
    expect(sent).toEqual([]);
    trash.acknowledgeStop(true);
    expect(trash.canConfirm).toBe(true);
    await trash.confirm();
    expect(sent).toEqual([{ planeId: REMOTE, reviewId: "review-1", mode: "delete_everywhere", acknowledgeStop: true }]);
  });

  it("shows a review whose workspace could not be checked", async () => {
    ipc((command) => {
      if (command === "preview_trash") {
        return preview({ protections: null, workspaceUnchecked: true, auditRecords: null, stopAcknowledgementRequired: true });
      }
      throw new Error(`Unexpected ${command}`);
    });
    const trash = session();
    await trash.openMove("local", "c1");
    expect(trash.dialog).toMatchObject({ kind: "review_unchecked", mode: "forget" });
    // Forget stops nothing, so it stays available.
    expect(trash.canConfirm).toBe(true);
    trash.chooseMode("delete_everywhere");
    expect(trash.canConfirm).toBe(false);
  });

  it("an uncertain request keeps its review and locked mode across close and reopen", async () => {
    const sent: unknown[] = [];
    let previews = 0;
    ipc((command, args) => {
      if (command === "preview_trash") {
        previews += 1;
        return preview();
      }
      if (command === "trash_conversation") {
        sent.push(args);
        if (sent.length === 1) throw failure("transport.offline", "offline");
        return { kind: "trashed", entry: ENTRY };
      }
      return null;
    });
    const trash = session();
    await trash.openMove(REMOTE, "c1");
    trash.chooseMode("delete_everywhere");
    await trash.confirm();
    expect(trash.dialog).toMatchObject({ kind: "uncertain", mode: "delete_everywhere" });
    expect(trash.closeMove()).toBe(true);
    expect(trash.dialog.kind).toBe("closed");

    await trash.openMove(REMOTE, "c1");
    expect(previews).toBe(1);
    expect(trash.dialog).toMatchObject({ kind: "uncertain", mode: "delete_everywhere" });
    trash.chooseMode("forget");
    expect(trash.dialog).toMatchObject({ mode: "delete_everywhere" });
    await trash.confirm();
    expect(sent).toEqual([
      { planeId: REMOTE, reviewId: "review-1", mode: "delete_everywhere", acknowledgeStop: false },
      { planeId: REMOTE, reviewId: "review-1", mode: "delete_everywhere", acknowledgeStop: false },
    ]);
    expect(trash.dialog.kind).toBe("trashed");
    // Resolved: the next open reads a new review.
    trash.closeMove();
    await trash.openMove(REMOTE, "c1");
    expect(previews).toBe(2);
  });

  it("a stale review asks for a new one, and Review again reads it", async () => {
    let previews = 0;
    ipc((command) => {
      if (command === "preview_trash") {
        previews += 1;
        return preview({ reviewId: `review-${previews}` });
      }
      if (command === "trash_conversation") return { kind: "refused", error: failure("retention.review_stale") };
      return null;
    });
    const trash = session();
    await trash.openMove("local", "c1");
    trash.chooseMode("delete_everywhere");
    await trash.confirm();
    expect(trash.dialog).toMatchObject({ kind: "stale", error: { code: "retention.review_stale" } });
    await trash.reviewAgain();
    expect(previews).toBe(2);
    expect(trash.dialog).toMatchObject({ kind: "review", mode: "forget", preview: { reviewId: "review-2" } });
  });

  it("drops a late preview for another task, and shows an already trashed task", async () => {
    const first = deferred<TrashPreview>();
    ipc((command, args) => {
      if (command === "preview_trash") {
        if (args.conversationId === "c1") return first.promise;
        return preview({ conversationId: "c2", reviewId: "", trash: { ...ENTRY, conversationId: "c2" } });
      }
      throw new Error(`Unexpected ${command}`);
    });
    const trash = session();
    const late = trash.openMove("local", "c1");
    await trash.openMove("local", "c2");
    first.resolve(preview());
    await late;
    expect(trash.dialog).toMatchObject({ kind: "already", preview: { conversationId: "c2" } });
    expect(trash.canConfirm).toBe(false);
  });

  it("does not close while sending", async () => {
    const pending = deferred<unknown>();
    ipc((command) => {
      if (command === "preview_trash") return preview();
      if (command === "trash_conversation") return pending.promise;
      return null;
    });
    const trash = session();
    await trash.openMove("local", "c1");
    const sending = trash.confirm();
    expect(trash.dialog.kind).toBe("sending");
    expect(trash.closeMove()).toBe(false);
    pending.resolve({ kind: "refused", error: failure("retention.live_work") });
    await sending;
    expect(trash.dialog).toMatchObject({ kind: "refused", error: { code: "retention.live_work" } });
  });
});
