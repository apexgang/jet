import { afterEach, describe, expect, it, vi } from "vitest";
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { DeliverySession } from "../src/lib/features/delivery/session.svelte";
import { deliverySummary } from "../src/lib/features/delivery/model";
import type { Delivery, DeliveryReview } from "../src/lib/jet/delivery";

const review: DeliveryReview = { reviewId: "native-review", planeId: "local", conversationId: "a", workingTree: "Managed Workspace", operation: { kind: "push", remote: "origin" }, checkpointFiles: null, contentComplete: null };
const failure = { category: "offline", code: "transport.offline", message: "Offline", retryable: true };
const row: Delivery = { id: "d", operation: "push", destination: "origin", checkpoint: null, status: "completed", head: "abcdef", branch: "feature/a", pullRequest: null, code: null, acknowledged: false, title: null };

afterEach(() => { if (typeof window !== "undefined") clearMocks(); vi.unstubAllGlobals(); });
function ipc(handler: Parameters<typeof mockIPC>[0]) {
  vi.stubGlobal("window", {});
  mockIPC(handler);
}

describe("delivery boundary and state", () => {
  it("retains one reviewed request across uncertain transport and navigation", async () => {
    const sent: unknown[] = [];
    ipc((command, args) => {
      if (command === "load_deliveries") return [];
      if (command === "prepare_delivery") { expect(args).toEqual({ conversationId: "a", operation: { kind: "push", remote: "origin" }, planeId: "local" }); return review; }
      if (command === "execute_delivery") {
        sent.push(args);
        if (sent.length === 1) throw failure;
        return { kind: "accepted", deliveryId: "effect-1" };
      }
      throw new Error(`Unexpected ${command}`);
    });
    const session = new DeliverySession();
    session.select("a"); await session.refresh();
    await session.prepare({ kind: "push", remote: "origin" });
    await session.confirm();
    expect(session.review?.kind).toBe("uncertain");
    session.select("b"); session.select("a");
    expect(session.review?.kind).toBe("uncertain");
    await session.confirm();
    expect(sent).toEqual([{ reviewId: "native-review" }, { reviewId: "native-review" }]);
    expect(session.review).toEqual({ kind: "queued", deliveryId: "effect-1" });
    expect(session.rows).toEqual([]); // Acceptance is not optimistic completion.
  });

  it("ignores history arriving for an old selection or after disconnect", async () => {
    let resolve!: (rows: Delivery[]) => void;
    ipc(() => new Promise<Delivery[]>((r) => { resolve = r; }));
    const session = new DeliverySession();
    session.select("a"); const first = session.refresh();
    session.select("b"); resolve([row]); await first;
    expect(session.rows).toEqual([]);
    const second = session.refresh(); session.offline(); resolve([row]); await second;
    expect(session.fresh).toBe(false); expect(session.rows).toEqual([]);
  });

  it("does not replace a newer review with a late preview from an earlier selection", async () => {
    let finishOld!: (review: DeliveryReview) => void;
    let previews = 0;
    ipc((command) => {
      if (command === "load_deliveries") return [];
      if (command === "prepare_delivery") {
        if (++previews === 1) return new Promise<DeliveryReview>((resolve) => { finishOld = resolve; });
        return { ...review, reviewId: "new-review" };
      }
      throw new Error(command);
    });
    const session = new DeliverySession(); session.select("a"); await session.refresh();
    const old = session.prepare({ kind: "push", remote: "origin" });
    session.select("b"); session.select("a"); await session.refresh();
    await session.prepare({ kind: "push", remote: "origin" });
    finishOld(review); await old;
    expect(session.review).toEqual({ kind: "ready", review: { ...review, reviewId: "new-review" } });
  });

  it("ends admission only on an explicit daemon refusal", async () => {
    ipc((command) => {
      if (command === "load_deliveries") return [];
      if (command === "prepare_delivery") return review;
      return { kind: "refused", error: { ...failure, category: "unavailable", code: "git.active_work" } };
    });
    const session = new DeliverySession(); session.select("a"); await session.refresh();
    await session.prepare({ kind: "push", remote: "origin" }); await session.confirm();
    expect(session.review).toBe(null);
    expect(session.error?.code).toBe("git.active_work");
  });

  it("acknowledges an unknown effect without resubmitting its Git operation", async () => {
    const calls: string[] = [];
    ipc((command) => {
      calls.push(command);
      if (command === "load_deliveries") return [{ ...row, status: "outcome_unknown" }];
      if (command === "prepare_delivery_acknowledgement") return "ack-review";
      if (command === "execute_delivery") return { kind: "accepted", deliveryId: "d" };
      throw new Error(command);
    });
    const session = new DeliverySession(); session.select("a"); await session.refresh();
    await session.acknowledge("d"); expect(session.review?.kind).toBe("acknowledge");
    await session.confirm();
    expect(session.review).toEqual({ kind: "acknowledged", deliveryId: "d" });
    expect(calls).not.toContain("prepare_delivery");
  });

  it("keeps a reviewed request per Plane, so the same Conversation UUID on another Plane is another scope", async () => {
    const remote = "0000000a-0000-4000-8000-000000000002";
    const prepared: unknown[] = [];
    ipc((command, args) => {
      if (command === "load_deliveries") return [];
      if (command === "prepare_delivery") { prepared.push(args); return { ...review, planeId: (args as { planeId: string }).planeId }; }
      if (command === "execute_delivery") throw failure;
      throw new Error(command);
    });
    const session = new DeliverySession();
    session.select("a", remote); await session.refresh();
    await session.prepare({ kind: "push", remote: "origin" });
    await session.confirm();
    expect(session.review?.kind).toBe("uncertain");
    session.select("a", "local");
    expect(session.review).toBe(null);
    session.select("a", remote);
    expect(session.review?.kind).toBe("uncertain");
    expect(prepared).toEqual([{ conversationId: "a", operation: { kind: "push", remote: "origin" }, planeId: remote }]);
  });

  it("preserves partial success and gives uncertainty precedence", () => {
    expect(deliverySummary([row, { ...row, id: "failed", status: "failed" }])).toContain("Completed changes remain");
    expect(deliverySummary([row, { ...row, status: "outcome_unknown" }])).toContain("Do not repeat");
    expect(deliverySummary([{ ...row, status: "pending" }])).toContain("not been confirmed");
  });
});
