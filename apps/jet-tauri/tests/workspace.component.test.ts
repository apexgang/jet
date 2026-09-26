// @vitest-environment happy-dom
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";
import NewTask from "../src/lib/features/workspace/NewTask.svelte";
import Message from "../src/lib/features/workspace/Message.svelte";
import RenameTask from "../src/lib/features/workspace/RenameTask.svelte";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";
import type { ConversationDetail } from "../src/lib/jet/bridge";

afterEach(() => { cleanup(); clearMocks(); });

describe("focused desktop workspace", () => {
  it("inserts an editable starting point without sending or replacing user input", async () => {
    const calls: string[] = [];
    mockIPC((command) => { calls.push(command); return null; });
    const session = new DesktopSession();
    const view = render(NewTask, { session });
    await fireEvent.click(view.getByText("Make a change"));
    expect(session.draft).toBe("I would like to change ");
    expect(view.getByText("Make a change").closest("button")?.disabled).toBe(true);
    expect(view.getByRole("button", { name: /Start task/ }).hasAttribute("disabled")).toBe(true);
    expect(calls).toEqual([]);
  });

  it("renders generated HTML and links as inert text, including inside code fences", () => {
    const text = '# Result\n\n<script>alert(1)</script>\n\n[open](javascript:alert(1))\n\n```html\n<img src=x onerror=alert(1)>\n```';
    const view = render(Message, { text });
    expect(view.container.querySelectorAll("script, img, a")).toHaveLength(0);
    expect(view.container.querySelector("pre")?.textContent).toContain("<img src=x");
    expect(view.getByRole("heading").textContent).toBe("Result");
  });

  it("keeps a refused rename and its command identity available for retry", async () => {
    const calls: unknown[] = [];
    mockIPC((command, args) => {
      if (command === "rename_conversation") {
        calls.push(args!);
        throw { category: "offline", code: "transport.offline", message: "Reconnect and try again.", retryable: true };
      }
      return null;
    });
    const session = new DesktopSession();
    session.connectionState = "online";
    session.selectedConversationId = "task-1";
    session.conversationDetail = {
      conversation: { id: "task-1", planeId: "local", revision: "9007199254740993", title: "Original", createdAtUnixMs: "1", projectId: null },
      cursor: "1", workspaceId: null, workspaceRoot: null, runs: [], retention: "retain",
    } as ConversationDetail;
    render(RenameTask, { session });
    const attempt = "48720268-5a08-4c18-9819-44495f77c780";
    expect(await session.renameTask("My next step", attempt, "9007199254740993")).toBe(false);
    expect(await session.renameTask("My next step", attempt, "9007199254740993")).toBe(false);
    expect(calls).toEqual([0, 1].map(() => ({ conversationId: "task-1", expectedRevision: "9007199254740993", name: "My next step", attempt, planeId: "local" })));
    expect(session.conversationDetail.conversation.title).toBe("Original");
    expect(session.actionNotice).toBe("Reconnect and try again.");
  });

  it("refreshes a conflicting name without discarding the rename input", async () => {
    mockIPC((command) => {
      if (command === "rename_conversation") throw { category: "conflict", code: "revision.conflict" };
      return null;
    });
    const session = new DesktopSession();
    session.connectionState = "online";
    session.selectedConversationId = "task-1";
    session.conversationDetail = {
      conversation: { id: "task-1", planeId: "local", revision: "1", title: "Original", createdAtUnixMs: "1", projectId: null },
      cursor: "1", workspaceId: null, workspaceRoot: null, runs: [], retention: "retain",
    } as ConversationDetail;
    const refresh = vi.spyOn(session, "loadSelectedConversation").mockImplementation(async () => {
      session.conversationDetail = { ...session.conversationDetail!, conversation: { ...session.conversationDetail!.conversation, revision: "2", title: "Changed elsewhere" } };
    });
    expect(await session.renameTask("My name", "48720268-5a08-4c18-9819-44495f77c780", "1")).toBe(false);
    expect(refresh).toHaveBeenCalledWith(false);
    expect(session.conversationDetail.conversation.title).toBe("Changed elsewhere");
    expect(session.actionNotice).toContain("Use its latest version");
    expect(session.conversationBusy).toBe(false);
  });
});
