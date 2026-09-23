import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import {
  changeAutodeleteRule,
  loadAutodeleteRules,
  loadTrash,
  loadTrashStatus,
  previewTrash,
  resolveConversationNames,
  restoreConversation,
  trashConversation,
} from "../src/lib/jet/retention";

const REMOTE = "0000000a-0000-4000-8000-000000000002";

afterEach(() => {
  if (typeof window !== "undefined") clearMocks();
  vi.unstubAllGlobals();
});

function record(): Array<[string, unknown]> {
  const calls: Array<[string, unknown]> = [];
  vi.stubGlobal("window", {});
  mockIPC((command, args) => {
    calls.push([command, args]);
    return null;
  });
  return calls;
}

describe("Jet Trash IPC adapter", () => {
  it("sends each command with exactly its Plane-scoped arguments", async () => {
    const calls = record();
    await loadTrash(REMOTE);
    await loadTrashStatus("local", "c1");
    await resolveConversationNames(REMOTE, ["c1", "c2"]);
    await previewTrash(REMOTE, "c1");
    await trashConversation(REMOTE, "review-1", "delete_everywhere", true);
    await trashConversation("local", "review-2", "forget", false);
    await restoreConversation(REMOTE, "c1");
    expect(calls).toEqual([
      ["load_trash", { planeId: REMOTE }],
      ["load_trash_status", { planeId: "local", conversationId: "c1" }],
      ["resolve_conversation_names", { planeId: REMOTE, conversationIds: ["c1", "c2"] }],
      ["preview_trash", { planeId: REMOTE, conversationId: "c1" }],
      ["trash_conversation", { planeId: REMOTE, reviewId: "review-1", mode: "delete_everywhere", acknowledgeStop: true }],
      ["trash_conversation", { planeId: "local", reviewId: "review-2", mode: "forget", acknowledgeStop: false }],
      ["restore_conversation", { planeId: REMOTE, conversationId: "c1" }],
    ]);
  });

  it("sends auto-delete reads and changes with snake_case change fields", async () => {
    const calls = record();
    await loadAutodeleteRules(REMOTE);
    await changeAutodeleteRule(REMOTE, { kind: "compile", rule_id: null, prompt: "Old tasks" });
    await changeAutodeleteRule("local", { kind: "set_inactive_days", rule_id: "r1", inactive_days: 30 });
    await changeAutodeleteRule("local", { kind: "approve", token_id: "t1" });
    await changeAutodeleteRule("local", { kind: "authorize_everywhere", token_id: "t2" });
    await changeAutodeleteRule("local", { kind: "delete", rule_id: "r1" });
    expect(calls).toEqual([
      ["load_autodelete_rules", { planeId: REMOTE }],
      ["change_autodelete_rule", { planeId: REMOTE, change: { kind: "compile", rule_id: null, prompt: "Old tasks" } }],
      ["change_autodelete_rule", { planeId: "local", change: { kind: "set_inactive_days", rule_id: "r1", inactive_days: 30 } }],
      ["change_autodelete_rule", { planeId: "local", change: { kind: "approve", token_id: "t1" } }],
      ["change_autodelete_rule", { planeId: "local", change: { kind: "authorize_everywhere", token_id: "t2" } }],
      ["change_autodelete_rule", { planeId: "local", change: { kind: "delete", rule_id: "r1" } }],
    ]);
  });
});
