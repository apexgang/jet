import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import type { Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { VirtualClock } from "./e2e/clock";
import { FakeApp, FakeHost, fakeDriver, type FakeAppOptions, type FakeProcess } from "./e2e/fake";
import { DEFAULT_OPTIONS, runJourney, summaryMarkdown, type JourneyOptions, type JourneyReport } from "./e2e/journey";
import { ProcFs } from "./e2e/procfs";
import { WebDriver } from "./e2e/webdriver";

const VERSION = "0.2.0";
const DRIVER_PID = 40_000;
const TICKS = 100;

/**
 * A /proc directory whose processes burn CPU at fixed rates as the
 * virtual clock advances, so the idle summary has an exact answer.
 */
class FakeProc {
  private readonly processes = new Map<number, { comm: string; ppid: number; start: number; ticksPerSecond: number; pssKiB: number }>();
  private nextPid = 50_000;

  constructor(
    readonly root: string,
    private readonly clock: VirtualClock,
    private readonly boot: number,
  ) {
    clock.onAdvance(() => this.write());
    this.add("tauri-driver", 1, 0, 1_000, DRIVER_PID);
  }

  add(comm: string, ppid: number, ticksPerSecond: number, pssKiB: number, pid = this.nextPid++): number {
    this.processes.set(pid, { comm, ppid, start: this.clock.now(), ticksPerSecond, pssKiB });
    this.write();
    return pid;
  }

  remove(pid: number): void {
    this.processes.delete(pid);
    rmSync(join(this.root, String(pid)), { recursive: true, force: true });
  }

  private write(): void {
    mkdirSync(this.root, { recursive: true });
    writeFileSync(join(this.root, "uptime"), `${((this.clock.now() - this.boot) / 1000).toFixed(2)} 0.00\n`);
    for (const [pid, entry] of this.processes) {
      const dir = join(this.root, String(pid));
      mkdirSync(dir, { recursive: true });
      const starttime = Math.round(((entry.start - this.boot) / 1000) * TICKS);
      // Whole milliseconds times a whole rate: exact.
      const ticks = Math.floor(((this.clock.now() - entry.start) * entry.ticksPerSecond) / 1000);
      const fields = ["S", entry.ppid, pid, pid, 0, -1, 0, 0, 0, 0, 0, ticks, 0, 0, 0, 20, 0, 1, 0, starttime, 0, 0];
      writeFileSync(join(dir, "stat"), `${pid} (${entry.comm}) ${fields.join(" ")}\n`);
      writeFileSync(join(dir, "smaps_rollup"), `Rss: ${entry.pssKiB * 2} kB\nPss: ${entry.pssKiB} kB\n`);
    }
  }
}

type Rig = {
  report: JourneyReport;
  app: FakeApp;
  host: FakeHost;
  saved: Map<string, Uint8Array | string>;
  logs: string[];
};

let cleanups: Array<() => Promise<void> | void> = [];

afterEach(async () => {
  for (const cleanup of cleanups.reverse()) await cleanup();
  cleanups = [];
});

async function journey(
  app: Partial<Omit<FakeAppOptions, "clock" | "home" | "configHome" | "version" | "startProcess">> = {},
  options: Partial<JourneyOptions> = {},
  prepare: (home: string) => void = () => undefined,
): Promise<Rig> {
  const scratch = mkdtempSync(join(tmpdir(), "jet-e2e-journey-"));
  cleanups.push(() => rmSync(scratch, { recursive: true, force: true }));
  const home = join(scratch, "home");
  mkdirSync(home);
  prepare(home);
  const application = join(scratch, "jet-tauri");
  writeFileSync(application, "#!/bin/sh\n");
  chmodSync(application, 0o755);
  const payload = join(scratch, "jet-core.tar.gz");
  writeFileSync(payload, "payload");

  const clock = new VirtualClock();
  const proc = new FakeProc(join(scratch, "proc"), clock, clock.now() - 3_600_000);
  const startApp = (): FakeProcess => {
    const pid = proc.add("jet-tauri", DRIVER_PID, 1, 60_000);
    const web = proc.add("WebKitWebProces", pid, 2, 90_000);
    const network = proc.add("WebKitNetworkPr", pid, 0, 20_000);
    return { pid, stop: () => [pid, web, network].forEach((each) => proc.remove(each)) };
  };
  const fake = new FakeApp({
    clock,
    home,
    configHome: join(home, ".config"),
    version: VERSION,
    signed: false,
    startProcess: startApp,
    ...app,
  });
  const server: Server = fakeDriver(fake);
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
  cleanups.push(() => new Promise<void>((resolve) => server.close(() => resolve())));
  const { port } = server.address() as AddressInfo;
  const host = new FakeHost(home, {
    clock,
    version: VERSION,
    application,
    startDaemon: () => {
      const pid = proc.add("jetd", 1, 0, 12_000);
      return { pid, stop: () => proc.remove(pid) };
    },
    onOutage: (outage) => fake.outage(outage),
  });
  const saved = new Map<string, Uint8Array | string>();
  const logs: string[] = [];
  const report = await runJourney(
    { ...DEFAULT_OPTIONS, application, payload, expectVersion: VERSION, warmLaunches: 2, ...options },
    {
      driver: new WebDriver(`http://127.0.0.1:${port}`),
      host,
      proc: new ProcFs(proc.root),
      clock,
      clockTicks: TICKS,
      driverPid: DRIVER_PID,
      log: (line) => logs.push(line),
      save: (name, data) => {
        saved.set(name, data);
        return name;
      },
    },
  );
  return { report, app: fake, host, saved, logs };
}

describe("the desktop release journey against fakes", () => {
  it("passes a fresh install end to end and records every measurement", async () => {
    const { report, app, host, saved } = await journey();
    expect(report.failure).toBeNull();
    expect(report.result).toBe("passed");
    expect(report.checks.filter((check) => !check.passed)).toEqual([]);
    expect(report.environment).toMatchObject({ kernel: "6.8.0-fake", package: "jet", packageVersion: VERSION, clockTicks: TICKS });

    const [cold, ...warm] = report.launches;
    expect(cold).toMatchObject({
      kind: "cold",
      processToInteractiveMs: 400,
      processToServiceRunningMs: 2_400,
      processToPlaneConnectedMs: 2_350,
      servicePhases: ["checking", "installing", "running"],
    });
    expect(warm).toHaveLength(2);
    for (const launch of warm) {
      expect(launch).toMatchObject({ kind: "warm", processToInteractiveMs: 400, processToServiceRunningMs: 700, servicePhases: ["checking", "running"] });
    }

    expect(report.provisioning).toMatchObject({
      sawSettingUp: true,
      checkingToRunningMs: 1_800,
      processToRunningMs: 2_400,
      notice: "The Jet service 0.2.0 is set up on this computer.",
    });
    expect(report.provisioning!.setupTexts).toContain("Setting up the Jet service on this computer…");

    // jet-tauri 1 + web process 2 ticks a second: 3% of a CPU; jetd idle.
    expect(report.idle!.windowMs).toBe(60_000);
    expect(report.idle!.app.cpuPercent).toBe(3);
    expect(report.idle!.app.pss).toEqual({ meanKiB: 170_000, maxKiB: 170_000, lastKiB: 170_000 });
    expect(report.idle!.app.processes.map((process) => process.comm).sort()).toEqual(["WebKitNetworkPr", "WebKitWebProces", "jet-tauri"]);
    expect(report.idle!.jetd.cpuPercent).toBe(0);
    expect(report.idle!.jetd.pss?.meanKiB).toBe(12_000);

    expect(report.settings).toMatchObject({
      managedBy: expect.stringMatching(/^Managed by this app/),
      version: VERSION,
      bundled: VERSION,
      updates: "This is a development build, so it doesn't update itself.",
      updateControls: [],
    });

    // RestartSec is 2 s; the fake UI reconnects 300 ms after the daemon.
    expect(report.reconnect).toMatchObject({
      killToDaemonReadyMs: 2_000,
      daemonReadyToUiConnectedMs: 300,
      killToUiConnectedMs: 2_300,
      planeStates: ["Reconnecting", "Connected"],
    });
    expect(report.reconnect!.newPid).not.toBe(report.reconnect!.oldPid);
    expect(report.quits).toHaveLength(3);
    expect(report.quits.every((quit) => !quit.forced)).toBe(true);
    expect(app.running).toBe(false);

    expect(report.proposals.map((row) => [row.measurement, row.value, row.within])).toEqual([
      ["cold launch to shell-interactive", 400, true],
      ["warm launch to shell-interactive (slowest sample)", 400, true],
      ["idle app-tree CPU", 3, false],
      ["reconnect after the daemon is ready", 300, true],
    ]);
    expect(report.screenshots).toEqual([
      "01-cold-shell-interactive.png",
      "02-setup-provisioning.png",
      "03-setup-connected.png",
      "04-settings-versions.png",
      "05-reconnecting.png",
      "06-reconnected.png",
      "07-warm-shell-interactive.png",
      "08-relaunch-connected.png",
      "09-warm-shell-interactive.png",
    ]);
    expect([...saved.keys()]).toEqual(report.screenshots);
    expect(app.clicks).toEqual(["button:Projects", "button:Settings", "button:Safety and system"]);
    expect(host.calls).toContainEqual(["systemctl", "--user", "kill", "jetd.service"]);
    // It never signals anything itself when the app exits with its session.
    expect(host.kills).toEqual([]);
    expect(summaryMarkdown(report)).toContain("cold launch to shell-interactive | 400 ms | 2500 ms");
  });

  it("expects App updates on in a signed build", async () => {
    const { report } = await journey({ signed: true }, { signed: true, warmLaunches: 0 });
    expect(report.result).toBe("passed");
    expect(report.settings?.updates).toBe("Jet 0.2.0 is up to date.");
    expect(report.settings?.updateControls).toEqual(["Check for updates", "Check for updates automatically"]);
    expect(report.checks.find((check) => check.name === "App updates are on in a signed build")?.passed).toBe(true);
  });

  it("takes a signed build whose update check failed as on: the updater ran", async () => {
    const { report } = await journey(
      {
        signed: true,
        updates: {
          status: "The update information isn't available right now. Try again later. update.release_unavailable",
          controls: ["Check for updates", "Check for updates automatically"],
        },
      },
      { signed: true, warmLaunches: 0 },
    );
    expect(report.result).toBe("passed");
  });

  it("fails a signed build whose updater is off or unreadable, whatever the status says", async () => {
    const cases = [
      {
        status: "This copy of Jet can't update itself. Install it from a .deb, .rpm or AppImage release to get updates.",
        controls: [],
      },
      { status: "Homebrew keeps this copy of Jet up to date. Update it with brew upgrade.", controls: [] },
      { status: "Jet couldn't read its update state. update.state_unavailable", controls: ["Try again"] },
    ];
    for (const updates of cases) {
      const { report } = await journey({ signed: true, updates }, { signed: true, warmLaunches: 0 });
      expect(report.result).toBe("failed");
      expect(report.failure).toEqual({
        step: "Settings › Versions",
        message: `App updates are on in a signed build: ${updates.status} (controls: ${updates.controls.join(", ") || "none"})`,
      });
    }
  });

  it("fails when the build's updater state does not match how it was built", async () => {
    const { report } = await journey({ signed: true }, { signed: false, warmLaunches: 0 });
    expect(report.result).toBe("failed");
    expect(report.failure).toEqual({
      step: "Settings › Versions",
      message:
        "App updates are off in an unsigned build: Jet 0.2.0 is up to date. (controls: Check for updates, Check for updates automatically)",
    });
  });

  it("stops at a settled provisioning failure and collects diagnostics", async () => {
    const { report, app, saved } = await journey({ failure: "service.install_failed" });
    expect(report.result).toBe("failed");
    expect(report.failures).toHaveLength(1);
    expect(report.failure?.step).toBe("provisioning");
    expect(report.failure?.message).toContain("service.install_failed");
    expect(report.failure?.message).toContain("settled as failed");
    // The launch before the failure is still measured; nothing after it ran.
    expect(report.launches.map((launch) => launch.kind)).toEqual(["cold"]);
    expect(report.idle).toBeNull();
    expect(String(saved.get("diagnostics.txt"))).toContain("### Failure 1: provisioning");
    expect(String(saved.get("diagnostics.txt"))).toContain("$ systemctl --user status jetd.service");
    expect(String(saved.get("failure-1-main.txt"))).toContain("fake Jet window text");
    expect(report.screenshots.some((name) => name.endsWith("failure-1-main.png"))).toBe(true);
    expect(report.quits).toHaveLength(1);
    expect(app.running).toBe(false);
  });

  it("names what Setup last showed when the local Plane never connects", async () => {
    const { report } = await journey({ neverConnect: true }, { timeouts: { ...DEFAULT_OPTIONS.timeouts, provisionMs: 10_000 } });
    expect(report.failure?.step).toBe("provisioning");
    expect(report.failure?.message).toMatch(/^Setup to show the local Plane connected: not within 10 s \(/);
    expect(report.failure?.message).toContain("local-service-running");
    expect(report.failure?.message).toContain('Setup alert "Local Plane unavailable');
  });

  it("explains a Settings window WebDriver never lists, and still runs the later steps", async () => {
    const { report } = await journey({ hideSettingsWindow: true }, { timeouts: { ...DEFAULT_OPTIONS.timeouts, settingsMs: 5_000 } });
    expect(report.result).toBe("failed");
    expect(report.failures).toHaveLength(1);
    expect(report.failure?.step).toBe("Settings › Versions");
    expect(report.failure?.message).toContain("the Settings window to open: not within 5 s");
    expect(report.failure?.message).toContain('window handles ["main-window"]');
    expect(report.idle).not.toBeNull();
    // The reconnect and both relaunches ran from the main window.
    expect(report.reconnect?.killToUiConnectedMs).toBe(2_300);
    expect(report.launches.map((launch) => launch.kind)).toEqual(["cold", "warm", "warm"]);
    expect(summaryMarkdown(report)).toContain("Failed during **Settings › Versions**");
  });

  it("reports every independent step that fails", async () => {
    const { report, saved } = await journey(
      { hideSettingsWindow: true, staysOffline: true },
      { timeouts: { ...DEFAULT_OPTIONS.timeouts, settingsMs: 1_000, reconnectMs: 10_000 }, warmLaunches: 1 },
    );
    expect(report.failures.map((failure) => failure.step)).toEqual(["Settings › Versions", "reconnect after systemctl --user kill jetd"]);
    expect(report.failures[1].message).toMatch(/^the UI to reconnect to the restarted jetd: not within 10 s \(daemon socket accepting again; Plane states Reconnecting;/);
    // The relaunch still ran, and connected: a new page is not the stuck one.
    expect(report.launches.map((launch) => launch.kind)).toEqual(["cold", "warm"]);
    expect(report.checks.find((check) => check.name === "warm launch 1: nothing was provisioned again")?.passed).toBe(true);
    const diagnostics = String(saved.get("diagnostics.txt"));
    expect(diagnostics).toContain("### Failure 1: Settings › Versions");
    expect(diagnostics).toContain("### Failure 2: reconnect after systemctl --user kill jetd");
    const summary = summaryMarkdown(report);
    expect(summary).toContain("Failed during **Settings › Versions**");
    expect(summary).toContain("Failed during **reconnect after systemctl --user kill jetd**");
  });

  it("refuses a runner that already has a Jet core", async () => {
    const { report, app } = await journey({}, {}, (home) => {
      mkdirSync(join(home, ".jet/core/versions/0.1.0"), { recursive: true });
      symlinkSync("versions/0.1.0", join(home, ".jet/core/current"));
    });
    expect(report.failure?.step).toBe("preflight");
    expect(report.failure?.message).toMatch(/^the runner has no Jet core yet: /);
    expect(report.launches).toEqual([]);
    expect(app.running).toBe(false);
  });
});

describe("the fake journey leaves nothing behind", () => {
  it("writes only inside its scratch home", async () => {
    const { host } = await journey({}, { warmLaunches: 0 });
    expect(host.home.startsWith(tmpdir())).toBe(true);
    expect(existsSync(join(host.home, ".jet/core/current/jetd"))).toBe(true);
    expect(readdirSync(join(host.home, ".config/systemd/user"))).toEqual(["jetd.service"]);
    expect(readFileSync(join(host.home, ".config/systemd/user/jetd.service"), "utf8")).toContain("--channel gui");
  });
});
