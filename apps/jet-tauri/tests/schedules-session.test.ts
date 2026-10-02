// @vitest-environment happy-dom
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import { SchedulesSession } from "../src/lib/features/schedules/session.svelte";
import type { ScheduleRequest } from "../src/lib/jet/schedules";
afterEach(clearMocks);
const flush = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };
describe("daily schedules", () => {
  it("keeps the exact command identity after uncertain creation and clears instructions only on success", async () => {
    const calls: ScheduleRequest[] = []; let fail = true;
    mockIPC((command, args) => {
      if (command === "load_schedules") return [];
      if (command === "create_schedule") {
        const { request } = args as { request: ScheduleRequest }; calls.push(request);
        if (fail) throw { category: "outcome_unknown", code: "command.outcome_unknown", message: "Try again." };
        return { id: "schedule-1", ...request, nextDueAtUnixMs: "1" };
      }
      return null;
    });
    const model = new SchedulesSession(); model.select("local", "task-1"); await flush();
    model.prompt = "Review changes"; model.timeZone = "Europe/Moscow";
    expect(await model.create()).toBe(false); expect(model.prompt).toBe("Review changes");
    fail = false;
    expect(await model.create()).toBe(true); expect(calls[1]).toEqual(calls[0]); expect(model.prompt).toBe("");
    model.prompt = "Review changes"; await model.create();
    expect(calls[2].attempt).not.toBe(calls[0].attempt);
  });
  it("ignores a late list from a previously selected task", async () => {
    let complete: ((value: unknown) => void) | undefined;
    mockIPC((command, args) => {
      if (command !== "load_schedules") return null;
      if ((args as { conversationId: string }).conversationId === "first") return new Promise((resolve) => { complete = resolve; });
      return [];
    });
    const model = new SchedulesSession(); model.select("local", "first"); await flush(); model.select("local", "second"); await flush();
    complete?.([{ id: "old", conversationId: "first" }]); await flush();
    expect(model.tasks).toEqual([]); expect(model.scope?.conversationId).toBe("second");
  });
  it("uses a new command after the service conclusively refuses an attempt", async () => {
    const attempts: string[] = [];
    mockIPC((command, args) => {
      if (command === "load_schedules") return [];
      if (command === "create_schedule") {
        attempts.push((args as { request: ScheduleRequest }).request.attempt);
        throw { category: "conflict", code: "conversation.trashed", message: "Restore the task first." };
      }
      return null;
    });
    const model = new SchedulesSession(); model.select("local", "task-1"); await flush();
    model.prompt = "Review changes";
    await model.create(); await model.create();
    expect(attempts).toHaveLength(2); expect(attempts[1]).not.toBe(attempts[0]);
    expect(model.prompt).toBe("Review changes"); expect(model.error).toBe("Restore the task first.");
  });
});
