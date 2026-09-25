import type { LocalServiceView } from "../../src/lib/jet/local-service";

/** A running, app-managed local service; override what a test needs. */
export function serviceView(overrides: Partial<LocalServiceView> = {}): LocalServiceView {
  return {
    revision: 0,
    phase: "running",
    channel: "gui",
    manager: "systemd",
    bundledVersion: "0.2.0",
    currentVersion: "0.2.0",
    previousVersion: null,
    runningVersion: "0.2.0",
    canRepair: false,
    canRollback: false,
    lastAction: null,
    error: null,
    ...overrides,
  };
}
