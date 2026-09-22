import { expect, test } from "vitest";

import { fixtureForState, fixtureStates } from "../src/lib/jet/fixture";
import { DesktopSession } from "../src/lib/features/shell/session.svelte";

test("the desktop fixture loader exposes every frozen state", () => {
  expect(fixtureStates.map((state) => fixtureForState(state).state)).toEqual(fixtureStates);
});

test("the active shell fixture keeps protocol-sized values as text", () => {
  const fixture = fixtureForState("active");

  expect(fixture.plane.cursor).toBe("108");
  expect(fixture.conversation?.title).toBe("Polish first-run flow");
  expect(fixture.timeline.map((entry) => entry.kind)).toEqual(["user", "agent", "activity"]);
});

test("every frozen fixture state can drive the shell session", () => {
  const session = new DesktopSession();

  for (const state of fixtureStates) {
    session.showFixture(state);
    expect(session.scenario.state).toBe(state);
  }
});
