import { describe, expect, it } from "vitest";

import { launchNotices } from "../src/lib/features/planes/model";
import { PlanesSession, type FeedHandler } from "../src/lib/features/planes/session.svelte";
import type { PlanesSnapshot } from "../src/lib/jet/planes";

function snapshot(
  notice: PlanesSnapshot["notice"],
  identityNotice: PlanesSnapshot["identity"]["notice"],
): PlanesSnapshot {
  return {
    planes: [],
    identity: {
      clientId: "00000000-0000-4000-8000-00000000000c",
      key: "unknown",
      fingerprint: null,
      notice: identityNotice,
    },
    restoredSelection: null,
    notice,
    maximumRemotePlanes: 16,
  };
}

const noFeeds: FeedHandler = {
  receive: () => undefined,
  opened: () => undefined,
  openFailed: () => undefined,
};

describe("launch notices about saved Planes and this computer's identity", () => {
  it("says what launch did to each, registry first", () => {
    expect(launchNotices(snapshot(null, null))).toEqual([]);
    expect(launchNotices(snapshot("registry_reset", null))).toEqual([
      "Saved Planes couldn't be read and were reset. Add them again.",
    ]);
    const [newer] = launchNotices(snapshot("registry_newer", null));
    expect(newer).toMatch(/newer version of Jet/);
    expect(newer).toMatch(/unchanged/);
    expect(launchNotices(snapshot("registry_unreadable", null))[0]).toMatch(/kept unchanged/);

    const both = launchNotices(snapshot("registry_unreadable", "identity_replaced"));
    expect(both).toHaveLength(2);
    expect(both[1]).toMatch(/created a new one\. Pair remote Planes again\./);
    expect(launchNotices(snapshot(null, "identity_recovered"))[0]).toMatch(/restored the identity from the keyring/);
    expect(launchNotices(snapshot(null, "identity_unsaved"))[0]).toMatch(/stop working when Jet quits/);
  });

  it("ignores a notice this build does not know", () => {
    const unknown = {
      ...snapshot(null, null),
      notice: "registry_from_the_future",
      identity: { ...snapshot(null, null).identity, notice: "identity_from_the_future" },
    } as unknown as PlanesSnapshot;
    expect(launchNotices(unknown)).toEqual([]);
  });

  it("the Planes panel shows them until dismissed", () => {
    const planes = new PlanesSession(noFeeds);
    expect(planes.notices).toEqual([]);
    planes.snapshot = snapshot("registry_newer", "identity_recovered");
    expect(planes.notices).toHaveLength(2);
    planes.noticeDismissed = true;
    expect(planes.notices).toEqual([]);
  });
});
