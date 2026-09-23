import { describe, expect, it } from "vitest";

import {
  LANDED_SECTIONS,
  landedTarget,
  resolveTarget,
  settingsTargetForError,
} from "../src/lib/features/settings/model";
import { noticeLink } from "../src/lib/features/system/model";
import type { PublicError } from "../src/lib/jet/bridge";

const REMOTE = "0000000a-0000-4000-8000-000000000002";

function error(code: string, planeId: string | null = null): PublicError {
  return {
    category: "conflict",
    code,
    message: "Message",
    retryable: false,
    recoveryActions: [],
    restart: null,
    revisionConflict: null,
    protocolLimit: null,
    planeId,
  };
}

const RECOVERY_CODES = [
  "recovery.read_only",
  "recovery.deletion_ledger_corrupt",
  "recovery.restore_failed",
  "recovery.snapshot_gone",
  "recovery.purge_unavailable",
  "recovery.not_read_only_local",
];

describe("Wave 3.3 rows in the Settings target table", () => {
  it("lands Diagnostics and Versions in Safety and system; Recovery waits for its section", () => {
    expect(LANDED_SECTIONS.has("diagnostics")).toBe(true);
    expect(LANDED_SECTIONS.has("versions")).toBe(true);
    expect(LANDED_SECTIONS.has("recovery")).toBe(false);
    expect(resolveTarget({ pane: "safety", section: "versions" })).toEqual({ pane: "safety", section: "versions" });
    expect(resolveTarget({ pane: "safety", section: "diagnostics" })).toEqual({ pane: "safety", section: "diagnostics" });
    // A link to a section that has not landed opens the pane at its top.
    expect(resolveTarget({ pane: "safety", section: "recovery" })).toEqual({ pane: "safety", section: null });
    expect(landedTarget("versions", REMOTE)).toEqual({ pane: "safety", section: "versions", plane_id: REMOTE });
  });

  it("maps the security gap to Diagnostics and the export gate to Audit, carrying the Plane", () => {
    expect(settingsTargetForError(error("security.gap_unknown"))).toEqual({ pane: "safety", section: "diagnostics" });
    expect(settingsTargetForError(error("security.gap_unknown", REMOTE))).toEqual({
      pane: "safety",
      section: "diagnostics",
      plane_id: REMOTE,
    });
    expect(settingsTargetForError(error("audit.export_required", REMOTE))).toEqual({
      pane: "safety",
      section: "audit",
      plane_id: REMOTE,
    });
  });

  it("has a Recovery row for every recovery code, rendered only once the section lands", () => {
    for (const code of RECOVERY_CODES) {
      // Filtered by LANDED_SECTIONS: no link to a section that does not exist.
      expect(settingsTargetForError(error(code, REMOTE)), code).toBeNull();
    }
    expect(noticeLink("read_only", REMOTE)).toBeNull();
    expect(noticeLink("ledger_corrupt", REMOTE)).toBeNull();
  });

  it("keeps 3.2's rows for the codes 3.3 surfaces", () => {
    expect(settingsTargetForError(error("storage.disk_pressure", REMOTE))).toEqual({
      pane: "safety",
      section: "storage",
      plane_id: REMOTE,
    });
    expect(settingsTargetForError(error("security.audit_degraded"))).toEqual({ pane: "safety", section: "audit" });
    expect(settingsTargetForError(error("storage.collect_busy"))).toBeNull();
  });
});
