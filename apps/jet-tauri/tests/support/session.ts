import type { RunSupervision } from "../../src/lib/jet/bridge";
import type { DesktopSession } from "../../src/lib/features/shell/session.svelte";

/** Puts an active Run with an active Turn on the session: both run controls are available. */
export function withActiveRun(session: DesktopSession): void {
  session.selectedConversationId ??= "l1";
  session.sidebarSelection = "conversation";
  const turn: RunSupervision["turns"][number] = {
    id: "turn-1",
    sequence: "1",
    position: 0,
    source: "user",
    state: "active",
    runId: "run-1",
    target: "Current Run",
    withdrawable: false,
  } as RunSupervision["turns"][number];
  session.supervision = {
    cursor: "40",
    maximumEntries: 128,
    maximumPromptBytes: 65536,
    turns: [turn],
    execution: {
      cursor: "40",
      run: {
        id: "run-1",
        conversationId: "l1",
        revision: "3",
        lifecycle: "active",
        title: "Run",
        createdAtUnixMs: "1",
        endedAtUnixMs: null,
      },
      activity: "waiting_for_approval",
      needsAttention: false,
      termination: null,
    },
  };
}
