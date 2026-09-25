/**
 * The Linux desktop release journey (Wave 4 spec F). On a fresh runner with
 * the built `.deb` installed, it drives the real app through tauri-driver
 * and WebKitWebDriver and checks, through the DOM and the machine:
 *
 * 1. Cold launch: the shell becomes interactive; Setup reaches a connected
 *    local Plane after the app provisioned the service from its bundled
 *    payload (`~/.jet/core/current`, an enabled `jetd.service`, `jetd core
 *    status` on the GUI channel).
 * 2. Idle: CPU and PSS of the app tree and of jetd over a quiet window.
 * 3. Settings › Versions (a second window): "Managed by this app" and the
 *    bundled version; App updates on in a signed build (the updater's own
 *    controls show) and off in an unsigned one.
 * 4. `systemctl --user kill jetd.service`: the UI reconnects to the
 *    restarted daemon.
 * 5. Quit and relaunch (warm): the same connected state, nothing
 *    provisioned again, the same daemon.
 *
 * It records the measurements in a JSON report. Ceilings in
 * `PROPOSED_CEILINGS` are proposals (docs/resource-budgets.md) and never
 * fail the journey; functional checks do.
 */
import { accessSync, constants, existsSync, lstatSync, readFileSync, readlinkSync } from "node:fs";
import { cpus, totalmem } from "node:os";

// The shell's own mark names; timing.ts imports nothing, so bun runs it as is.
import { LOCAL_PLANE_CONNECTED, SHELL_INTERACTIVE, localServiceMark } from "../../src/lib/features/shell/timing";
import { errorText, type Clock, waitFor } from "./clock";
import {
  diagnostics,
  mainPid,
  parseCoreStatus,
  servicePaths,
  systemctlWord,
  UNIT,
  type CoreStatus,
  type Host,
} from "./host";
import { PAGE_SCRIPT, type ControlQuery, type MainView, type SettingsView } from "./page";
import {
  findProcess,
  processStartEpochMs,
  sampleTree,
  summarize,
  type ProcFs,
  type TreeSample,
  type TreeSummary,
} from "./procfs";
import type { Session, WebDriver } from "./webdriver";

const SERVICE_MARK = localServiceMark("");

/** Proposed ceilings (docs/resource-budgets.md, "Desktop (Linux)"); reported, never enforced. */
export const PROPOSED_CEILINGS = {
  coldLaunchMs: 2_500,
  warmLaunchMs: 1_000,
  idleAppCpuPercent: 0.5,
  reconnectAfterReadyMs: 1_500,
} as const;

export type JourneyOptions = {
  /** The installed app, as tauri-driver launches it. */
  application: string;
  /** Its process name in /proc (`comm`). */
  appCommand: string;
  /** The core payload the bundle installs next to the app. */
  payload: string;
  /** The release version: the bundled core and the app. */
  expectVersion: string;
  /** Signed bundles carry the updater configuration; unsigned ones say updates are off. */
  signed: boolean;
  /** Quiet time after provisioning before the idle window. */
  settleMs: number;
  idleMs: number;
  sampleIntervalMs: number;
  /** Relaunches after the first quit; each is one warm launch sample. */
  warmLaunches: number;
  pollMs: number;
  timeouts: {
    launchMs: number;
    provisionMs: number;
    settingsMs: number;
    reconnectMs: number;
    quitMs: number;
  };
};

export const DEFAULT_OPTIONS: Omit<JourneyOptions, "expectVersion"> = {
  application: "/usr/bin/jet-tauri",
  appCommand: "jet-tauri",
  payload: "/usr/lib/Jet/jet-core.tar.gz",
  signed: false,
  settleMs: 15_000,
  idleMs: 60_000,
  sampleIntervalMs: 2_000,
  warmLaunches: 3,
  pollMs: 100,
  timeouts: { launchMs: 90_000, provisionMs: 180_000, settingsMs: 60_000, reconnectMs: 60_000, quitMs: 20_000 },
};

export type JourneyDeps = {
  driver: WebDriver;
  host: Host;
  proc: ProcFs;
  clock: Clock;
  /** `getconf CLK_TCK`. */
  clockTicks: number;
  /** The WebDriver server's pid; the app is looked for below it. */
  driverPid: number | null;
  log: (line: string) => void;
  /** Writes an artifact next to the report and returns its file name. */
  save: (name: string, data: Uint8Array | string) => string;
};

export type Check = { name: string; passed: boolean; required: boolean; detail: string };

export type LaunchRecord = {
  kind: "cold" | "warm";
  appPid: number;
  /** Process start (from /proc) to the `shell-interactive` mark. */
  processToInteractiveMs: number;
  /** The WebDriver new-session request to the mark; includes the driver starting the app. */
  requestToInteractiveMs: number;
  /** Process start to this window's `local-service-running` mark. */
  processToServiceRunningMs: number | null;
  /** Process start to the first `local-plane-connected` mark: the local feed is online. */
  processToPlaneConnectedMs: number | null;
  /** Process start to the poll that saw Setup show the local Plane connected. */
  processToSetupConnectedMs: number | null;
  /** The local-service phases this window saw, in order. */
  servicePhases: string[];
};

export type ServiceRecord = {
  currentTarget: string | null;
  unitEnabled: string;
  unitActive: string;
  mainPid: number | null;
  coreStatus: CoreStatus | null;
};

export type JourneyReport = {
  schema: 1;
  kind: "jet-desktop-linux-journey";
  result: "passed" | "failed";
  /** The first failure; `failures` lists every step that failed. */
  failure: { step: string; message: string } | null;
  failures: Array<{ step: string; message: string }>;
  startedAt: string;
  finishedAt: string | null;
  app: { application: string; version: string; signed: boolean };
  environment: Record<string, string | number | null>;
  launches: LaunchRecord[];
  provisioning: {
    /** Phases the main window saw, in ms after the app process started. */
    phases: Array<{ phase: string; atMs: number }>;
    /** Setup's provisioning sentences, in the order seen. */
    setupTexts: string[];
    sawSettingUp: boolean;
    checkingToRunningMs: number | null;
    processToRunningMs: number | null;
    notice: string | null;
  } | null;
  service: ServiceRecord | null;
  idle: {
    settleMs: number;
    windowMs: number;
    sampleIntervalMs: number;
    app: TreeSummary;
    jetd: TreeSummary;
  } | null;
  settings: {
    managedBy: string;
    status: string;
    version: string | null;
    bundled: string | null;
    updates: string | null;
    updateControls: string[];
    thisApp: string | null;
  } | null;
  reconnect: {
    how: string;
    oldPid: number | null;
    newPid: number | null;
    /** The kill to the first accepted connection on the daemon socket. */
    killToDaemonReadyMs: number | null;
    /** The kill to the new jetd process's start (from /proc). */
    killToDaemonStartMs: number | null;
    /** The daemon socket accepting again to the UI's `local-plane-connected` mark. */
    daemonReadyToUiConnectedMs: number | null;
    killToUiConnectedMs: number;
    /** The sidebar's Plane states seen, in order. */
    planeStates: string[];
    /** Resolution of the daemon-ready time. */
    pollMs: number;
  } | null;
  quits: Array<{ forced: boolean; exitMs: number }>;
  proposals: Array<{ measurement: string; value: number | null; ceiling: number; unit: string; within: boolean | null }>;
  screenshots: string[];
  checks: Check[];
};

class JourneyFailure extends Error {
  constructor(message: string) {
    super(message);
    this.name = "JourneyFailure";
  }
}

const SIDEBAR = { role: "complementary", name: "Jet navigation" } as const;
const SETUP_BUTTONS = ["Add a Project", "Manage Projects"];
/**
 * App updates controls that AppUpdatesBlock shows only while the updater is
 * on: its check, install and restart buttons and the automatic-check toggle.
 * A disabled updater shows none of them, and a failed read shows only "Try
 * again".
 */
const UPDATER_CONTROLS = /^(Check for updates|Checking…|Install Jet |Installing…|Restart Jet…)/;

/** Whether Settings › Versions shows the updater on. */
export function updaterOn(versions: Pick<NonNullable<SettingsView["versions"]>, "updateControls">): boolean {
  return versions.updateControls.some((name) => UPDATER_CONTROLS.test(name));
}

export async function runJourney(options: JourneyOptions, deps: JourneyDeps): Promise<JourneyReport> {
  return new Journey(options, deps).run();
}

class Journey {
  private step = "start";
  private session: Session | null = null;
  private mainHandle: string | null = null;
  private settingsHandle: string | null = null;
  private appPid: number | null = null;
  /** Wall-clock start of the current app process. */
  private appStartedAt = 0;
  private shots = 0;
  private diagnosticsText = "";
  private readonly report: JourneyReport;

  constructor(
    private readonly options: JourneyOptions,
    private readonly deps: JourneyDeps,
  ) {
    this.report = {
      schema: 1,
      kind: "jet-desktop-linux-journey",
      result: "failed",
      failure: null,
      failures: [],
      startedAt: new Date(deps.clock.now()).toISOString(),
      finishedAt: null,
      app: { application: options.application, version: options.expectVersion, signed: options.signed },
      environment: {},
      launches: [],
      provisioning: null,
      service: null,
      idle: null,
      settings: null,
      reconnect: null,
      quits: [],
      proposals: [],
      screenshots: [],
      checks: [],
    };
  }

  async run(): Promise<JourneyReport> {
    // Settings, the reconnect and the relaunch only need a provisioned app,
    // so one run reports each of them even when an earlier one fails.
    const independent = async (step: () => Promise<void>) => {
      try {
        await step();
      } catch (error) {
        await this.failed(error);
        await this.backToMain();
      }
    };
    try {
      await this.preflight();
      const cold = await this.launch("cold");
      await this.provision(cold);
      this.report.service = await this.checkService("the provisioned service");
      await this.idle();
      await independent(() => this.settings());
      await independent(() => this.reconnect());
      await independent(() => this.relaunch());
      this.step = "final quit";
      await this.quitApp();
    } catch (error) {
      await this.failed(error);
      try {
        await this.quitApp();
      } catch (quitError) {
        this.deps.log(`could not quit the app after the failure: ${errorText(quitError)}`);
      }
    }
    this.report.result = this.report.failures.length === 0 ? "passed" : "failed";
    this.report.failure = this.report.failures[0] ?? null;
    this.report.proposals = proposals(this.report);
    this.report.finishedAt = new Date(this.deps.clock.now()).toISOString();
    return this.report;
  }

  // -- Checks and helpers ---------------------------------------------------

  /** Records a check; a failed required check ends the journey. */
  private check(name: string, passed: boolean, detail: string, required = true): void {
    this.report.checks.push({ name, passed, required, detail });
    this.deps.log(`${passed ? "ok  " : required ? "FAIL" : "note"} ${name}: ${detail}`);
    if (required && !passed) throw new JourneyFailure(`${name}: ${detail}`);
  }

  private get currentSession(): Session {
    if (!this.session) throw new JourneyFailure("no WebDriver session");
    return this.session;
  }

  private readMain(): Promise<MainView> {
    return this.currentSession.execute<MainView>(PAGE_SCRIPT, ["main"]);
  }

  private readSettings(): Promise<SettingsView> {
    return this.currentSession.execute<SettingsView>(PAGE_SCRIPT, ["settings"]);
  }

  /** Clicks a button found by role and accessible name, waiting for it to appear. */
  private async click(query: ControlQuery, what: string): Promise<void> {
    const session = this.currentSession;
    const element = await waitFor(
      this.deps.clock,
      `${what} to appear`,
      async () => (await session.element(what, PAGE_SCRIPT, ["button", query])) ?? undefined,
      { timeoutMs: 15_000, intervalMs: 250 },
    );
    await session.click(element);
  }

  /** A screenshot of the current window; a failed one is logged, never fatal. */
  private async shot(name: string): Promise<void> {
    if (!this.session) return;
    this.shots += 1;
    const file = `${String(this.shots).padStart(2, "0")}-${name}.png`;
    try {
      const image = await this.session.screenshot();
      this.report.screenshots.push(this.deps.save(file, image));
    } catch (error) {
      this.deps.log(`screenshot ${file} failed: ${errorText(error)}`);
    }
  }

  private sinceStart(epochMs: number | null): number | null {
    return epochMs === null ? null : round(epochMs - this.appStartedAt);
  }

  // -- Steps ----------------------------------------------------------------

  private async preflight(): Promise<void> {
    this.step = "preflight";
    const { host } = this.deps;
    const { application, payload, appCommand } = this.options;
    const paths = servicePaths(host);
    this.check("the app is installed", isExecutable(application), application);
    this.check("the bundle installed the core payload", existsSync(payload), payload);
    this.check("the runner has no Jet core yet", !isPresent(paths.current), paths.current);
    this.check("the runner has no jetd unit yet", !isPresent(paths.unit), paths.unit);
    const manager = await host.run(["systemctl", "--user", "show-environment"]);
    this.check(
      "the systemd user manager answers",
      manager.code === 0,
      manager.code === 0 ? "systemctl --user show-environment" : manager.stderr.trim() || manager.error || `exit ${manager.code}`,
    );
    const running = findProcess(this.deps.proc.stats(), appCommand, null);
    this.check("no app is running yet", running === null, running ? `pid ${running.pid}` : "none");

    const word = async (argv: string[]) => {
      const result = await host.run(argv);
      return result.code === 0 ? result.stdout.trim() || null : null;
    };
    const owner = await word(["dpkg-query", "--search", application]);
    const packageName = owner?.split(":")[0]?.trim() ?? null;
    this.report.environment = {
      os: osRelease(),
      kernel: await word(["uname", "-r"]),
      cpus: cpus().length,
      memoryMiB: Math.round(totalmem() / 2 ** 20),
      clockTicks: this.deps.clockTicks,
      webkitgtk: await word(["dpkg-query", "--show", "--showformat=${Version}", "libwebkit2gtk-4.1-0"]),
      package: packageName,
      packageVersion: packageName ? await word(["dpkg-query", "--show", "--showformat=${Version}", packageName]) : null,
    };
  }

  private async launch(kind: "cold" | "warm"): Promise<LaunchRecord> {
    this.step = `${kind} launch`;
    const { clock, driver, proc } = this.deps;
    const { timeouts, pollMs, appCommand } = this.options;
    const requestedAt = clock.now();
    this.session = await driver.newSession({ "tauri:options": { application: this.options.application } }, timeouts.launchMs);
    await this.session.setTimeouts({ script: 30_000, pageLoad: 60_000, implicit: 0 });
    this.mainHandle = await this.session.windowHandle();
    this.settingsHandle = null;
    const app = await waitFor(
      clock,
      `the ${appCommand} process below the WebDriver server`,
      async () => findProcess(proc.stats(), appCommand, this.deps.driverPid) ?? undefined,
      { timeoutMs: 15_000, intervalMs: 100 },
    );
    this.appPid = app.pid;
    this.appStartedAt = processStartEpochMs(app, proc.uptime(), clock.now(), this.deps.clockTicks);
    const view = await waitFor<MainView, MainView>(
      clock,
      `the shell to become interactive (performance mark ${SHELL_INTERACTIVE})`,
      async (seen) => {
        const current = await this.readMain();
        seen(current);
        return markEpoch(current, SHELL_INTERACTIVE) === null ? undefined : current;
      },
      { timeoutMs: timeouts.launchMs, intervalMs: pollMs, describe: describeMain },
    );
    const interactiveAt = markEpoch(view, SHELL_INTERACTIVE)!;
    this.check(`${kind} launch: the window shows Jet's navigation`, view.planeStatus !== null, view.planeStatus ?? "no navigation landmark");
    const record: LaunchRecord = {
      kind,
      appPid: app.pid,
      processToInteractiveMs: round(interactiveAt - this.appStartedAt),
      requestToInteractiveMs: round(interactiveAt - requestedAt),
      processToServiceRunningMs: null,
      processToPlaneConnectedMs: null,
      processToSetupConnectedMs: null,
      servicePhases: [],
    };
    // Recorded now, so a later failure still reports this launch.
    this.report.launches.push(record);
    this.deps.log(`${kind} launch: shell interactive ${record.processToInteractiveMs} ms after the process started`);
    await this.shot(`${kind}-shell-interactive`);
    return record;
  }

  /** Setup, opened from the sidebar unless it shows within `waitMs`. */
  private async openSetup(waitMs: number): Promise<void> {
    if (waitMs > 0) {
      try {
        await waitFor(this.deps.clock, "Setup", async () => ((await this.readMain()).setup ? true : undefined), {
          timeoutMs: waitMs,
          intervalMs: this.options.pollMs,
        });
        return;
      } catch {
        // Not restored: open it as a user would.
      }
    }
    await this.click({ scope: SIDEBAR, names: SETUP_BUTTONS }, "the sidebar's Setup button (Add a Project)");
  }

  /**
   * Waits until Setup shows the local Plane on `expectVersion`, the sidebar
   * says Connected and the service watcher saw `running`. A failure Setup
   * shows after the service settled ends the wait at once.
   */
  private async setupConnected(texts: string[], onProvisioning: () => Promise<void>): Promise<MainView> {
    const { clock } = this.deps;
    const outcome = await waitFor<MainView, { view: MainView } | { failed: string }>(
      clock,
      "Setup to show the local Plane connected",
      async (seen) => {
        const view = await this.readMain();
        seen(view);
        const setup = view.setup;
        if (setup?.provisioning && texts[texts.length - 1] !== setup.provisioning) {
          texts.push(setup.provisioning);
          await onProvisioning();
        }
        const phase = lastPhase(view);
        if (setup?.failure && (phase === "failed" || phase === "stopped" || phase === "not_installed")) {
          return { failed: `Setup shows "${setup.failure.text}" after the service settled as ${phase}` };
        }
        if (setup?.localPlane?.coreVersion && view.planeState === "Connected" && phase === "running") return { view };
        return undefined;
      },
      { timeoutMs: this.options.timeouts.provisionMs, intervalMs: this.options.pollMs, describe: describeMain },
    );
    if ("failed" in outcome) throw new JourneyFailure(outcome.failed);
    return outcome.view;
  }

  private async provision(record: LaunchRecord): Promise<void> {
    this.step = "provisioning";
    const { clock } = this.deps;
    await this.openSetup(0);
    const texts: string[] = [];
    let captured = false;
    const view = await this.setupConnected(texts, async () => {
      if (captured) return;
      captured = true;
      await this.shot("setup-provisioning");
    });
    const connectedAt = clock.now();
    const plane = view.setup!.localPlane!;
    const phases = servicePhases(view);
    const at = (phase: string) => phases.find((entry) => entry.phase === phase)?.at ?? null;
    record.servicePhases = phases.map((entry) => entry.phase);
    record.processToServiceRunningMs = this.sinceStart(at("running"));
    record.processToPlaneConnectedMs = this.sinceStart(markEpoch(view, LOCAL_PLANE_CONNECTED));
    record.processToSetupConnectedMs = this.sinceStart(connectedAt);

    const sawSettingUp = phases.some((entry) => entry.phase === "installing") || texts.some((text) => text.startsWith("Setting up"));
    this.report.provisioning = {
      phases: phases.map((entry) => ({ phase: entry.phase, atMs: this.sinceStart(entry.at)! })),
      setupTexts: texts,
      sawSettingUp,
      checkingToRunningMs: at("checking") !== null && at("running") !== null ? round(at("running")! - at("checking")!) : null,
      processToRunningMs: this.sinceStart(at("running")),
      notice: plane.notice,
    };
    this.check("Setup shows the local Plane on the bundled core", plane.coreVersion === this.options.expectVersion, plane.text);
    this.check("the sidebar shows the local Plane connected", view.planeState === "Connected", view.planeStatus ?? "no status");
    this.check(
      "Setup says this launch set the service up",
      /\bset up on this computer\b/.test(plane.notice ?? ""),
      plane.notice ?? "no notice in the Local Plane row",
    );
    // The page only sees phases after its watcher registered; a fast pass
    // can finish first, so this is recorded rather than required.
    this.check(
      "Setup showed the service being set up",
      sawSettingUp,
      `phases ${record.servicePhases.join(" → ") || "none"}; Setup said ${texts.map((text) => `"${text}"`).join(", ") || "nothing"}`,
      false,
    );
    await this.shot("setup-connected");
  }

  private async checkService(what: string): Promise<ServiceRecord> {
    this.step = `checking ${what}`;
    const { host, clock } = this.deps;
    const paths = servicePaths(host);
    const target = readLink(paths.current);
    this.check("~/.jet/core/current exists", isPresent(paths.current), target ? `${paths.current} -> ${target}` : paths.current);
    this.check("current/jetd is executable", isExecutable(paths.currentJetd), paths.currentJetd);
    this.check("the jetd user unit is installed", isPresent(paths.unit), paths.unit);
    const enabled = await systemctlWord(host, "is-enabled");
    this.check(`${UNIT} is enabled`, enabled === "enabled", enabled);
    const active = await systemctlWord(host, "is-active");
    this.check(`${UNIT} is active`, active === "active", active);
    // jetd takes its lock before it writes the owner's metadata.
    let lastOutput = "";
    const status = await waitFor<string, CoreStatus>(
      clock,
      "jetd core status to name the owner",
      async (seen) => {
        const result = await host.run([paths.currentJetd, "core", "status"]);
        lastOutput = result.stdout.trim() || result.stderr.trim() || result.error || `exit ${result.code}`;
        seen(lastOutput);
        if (result.code !== 0) return undefined;
        const parsed = parseCoreStatus(result.stdout);
        return parsed.owner.state === "held" && parsed.owner.daemon !== null ? parsed : undefined;
      },
      { timeoutMs: 15_000, intervalMs: 500, describe: (last, error) => (error ? errorText(error) : (last ?? "no output")) },
    );
    const daemon = status.owner.state === "held" ? status.owner.daemon : null;
    this.check("jetd core status: the daemon runs on the GUI channel", daemon?.channel === "gui", lastOutput);
    this.check(
      "jetd core status: current and the running daemon are the bundled version",
      status.current === this.options.expectVersion && daemon?.version === this.options.expectVersion,
      `current ${status.current}, running ${daemon?.version ?? "unknown"}`,
    );
    const pid = await mainPid(host);
    this.check(`${UNIT} has a main process`, pid !== null, String(pid));
    return { currentTarget: target, unitEnabled: enabled, unitActive: active, mainPid: pid, coreStatus: status };
  }

  private async idle(): Promise<void> {
    this.step = "idle measurement";
    const { clock, proc } = this.deps;
    const { settleMs, idleMs, sampleIntervalMs } = this.options;
    const app = this.appPid!;
    const jetd = this.report.service!.mainPid!;
    this.deps.log(`idle: settling ${settleMs} ms, then sampling ${idleMs} ms`);
    await clock.sleep(settleMs);
    const appSamples: TreeSample[] = [];
    const jetdSamples: TreeSample[] = [];
    const started = clock.now();
    for (;;) {
      const now = clock.now();
      appSamples.push(sampleTree(proc, [app], now));
      jetdSamples.push(sampleTree(proc, [jetd], now));
      const elapsed = now - started;
      if (elapsed >= idleMs) break;
      await clock.sleep(Math.min(sampleIntervalMs, idleMs - elapsed));
    }
    const appSummary = summarize(appSamples, this.deps.clockTicks, app);
    const jetdSummary = summarize(jetdSamples, this.deps.clockTicks, jetd);
    this.report.idle = { settleMs, windowMs: appSummary.durationMs, sampleIntervalMs, app: appSummary, jetd: jetdSummary };
    this.check("the app stayed up while idle", !appSummary.rootExited, `pid ${app}`);
    this.check("jetd stayed up while idle", !jetdSummary.rootExited, `pid ${jetd}`);
    const names = [...new Set(appSummary.processes.map((process) => process.comm))];
    this.check(
      "the app tree includes the WebKit web and network processes",
      names.some((name) => name.startsWith("WebKitWebProc")) && names.some((name) => name.startsWith("WebKitNetwork")),
      names.join(", "),
      false,
    );
    this.deps.log(
      `idle: app ${appSummary.cpuPercent}% CPU, PSS ${appSummary.pss?.meanKiB ?? "?"} KiB; jetd ${jetdSummary.cpuPercent}% CPU, PSS ${jetdSummary.pss?.meanKiB ?? "?"} KiB`,
    );
  }

  private async settings(): Promise<void> {
    this.step = "Settings › Versions";
    const { clock } = this.deps;
    const { timeouts, pollMs, expectVersion, signed } = this.options;
    const session = this.currentSession;
    const before = await session.windowHandles();
    await this.click({ scope: SIDEBAR, names: ["Settings"] }, "the sidebar's Settings button");
    const handle = await waitFor<string[], string>(
      clock,
      "the Settings window to open",
      async (seen) => {
        const handles = await session.windowHandles();
        seen(handles);
        return handles.find((candidate) => !before.includes(candidate));
      },
      {
        timeoutMs: timeouts.settingsMs,
        intervalMs: 250,
        describe: (handles) =>
          `window handles ${JSON.stringify(handles ?? before)}, ${JSON.stringify(before)} before the click; the Settings webview must be under WebKit automation to be listed`,
      },
    );
    await session.switchToWindow(handle);
    this.settingsHandle = handle;
    await waitFor<SettingsView, SettingsView>(
      clock,
      "the Settings window to load its panes",
      async (seen) => {
        const view = await this.readSettings();
        seen(view);
        return view.panes.length > 0 && view.panes.every((pane) => !pane.disabled) ? view : undefined;
      },
      { timeoutMs: timeouts.settingsMs, intervalMs: pollMs, describe: describeSettings },
    );
    await this.click({ scope: { role: "navigation", name: "Settings" }, names: ["Safety and system"] }, "Settings' Safety and system pane");
    const view = await waitFor<SettingsView, SettingsView>(
      clock,
      "Settings › Versions to show the Jet service on this computer",
      async (seen) => {
        const current = await this.readSettings();
        seen(current);
        const facts = current.versions?.service?.facts;
        return facts?.["Managed by"] && facts.Status === "Running" && current.versions?.updates ? current : undefined;
      },
      { timeoutMs: timeouts.settingsMs, intervalMs: pollMs, describe: describeSettings },
    );
    await session.execute(PAGE_SCRIPT, ["reveal", "Jet service on this computer"]);
    await this.shot("settings-versions");
    const versions = view.versions!;
    const facts = versions.service!.facts;
    this.report.settings = {
      managedBy: facts["Managed by"],
      status: facts.Status,
      version: facts.Version ?? null,
      bundled: facts["Included with this app"] ?? null,
      updates: versions.updates,
      updateControls: versions.updateControls,
      thisApp: versions.thisApp,
    };
    this.check("the Settings window is Jet Settings", view.title === "Jet Settings", view.title);
    this.check("Versions: Managed by this app", facts["Managed by"].startsWith("Managed by this app"), facts["Managed by"]);
    this.check(`Versions: the service version is ${expectVersion}`, facts.Version === expectVersion, facts.Version ?? "missing");
    this.check(
      `Versions: this app includes ${expectVersion}`,
      facts["Included with this app"] === expectVersion,
      facts["Included with this app"] ?? "missing",
    );
    const updates = versions.updates ?? "";
    const on = updaterOn(versions);
    const shown = `${updates || "no status"} (controls: ${versions.updateControls.join(", ") || "none"})`;
    if (signed) {
      this.check("App updates are on in a signed build", on, shown);
    } else {
      this.check("App updates are off in an unsigned build", !on && /development build/.test(updates), shown);
    }
    await session.switchToWindow(this.mainHandle!);
  }

  private async reconnect(): Promise<void> {
    this.step = "reconnect after systemctl --user kill jetd";
    const { clock, host, proc } = this.deps;
    const { timeouts, pollMs } = this.options;
    const paths = servicePaths(host);
    const oldPid = await mainPid(host);
    const before = await this.readMain();
    this.check("the local Plane is connected before the kill", before.planeState === "Connected", before.planeStatus ?? "no status");
    const killedAt = clock.now();
    const kill = await host.run(["systemctl", "--user", "kill", UNIT]);
    this.check(`systemctl --user kill ${UNIT}`, kill.code === 0, kill.stderr.trim() || kill.error || `exit ${kill.code}`);
    // The daemon socket refuses while jetd is down; the first accepted
    // connection after that is "ready".
    const socket: { refusedAt: number | null; readyAt: number | null } = { refusedAt: null, readyAt: null };
    const states: string[] = [];
    let captured = false;
    const { view, uiAt } = await waitFor<MainView, { view: MainView; uiAt: number }>(
      clock,
      "the UI to reconnect to the restarted jetd",
      async (seen) => {
        if (socket.readyAt === null) {
          const accepted = await host.socketAccepts(paths.socket, 1_000);
          const now = clock.now();
          if (!accepted) {
            socket.refusedAt ??= now;
          } else if (socket.refusedAt !== null) {
            socket.readyAt = now;
          } else {
            // Restarted between two probes: the main process changed.
            const pid = await mainPid(host);
            if (pid !== null && pid !== oldPid) socket.readyAt = now;
          }
        }
        const current = await this.readMain();
        seen(current);
        if (current.planeState !== null && states[states.length - 1] !== current.planeState) states.push(current.planeState);
        if (current.planeState !== "Connected" && !captured) {
          captured = true;
          await this.shot("reconnecting");
        }
        const connected = markEpoch(current, LOCAL_PLANE_CONNECTED, killedAt);
        return socket.readyAt !== null && connected !== null && current.planeState === "Connected"
          ? { view: current, uiAt: connected }
          : undefined;
      },
      {
        timeoutMs: timeouts.reconnectMs,
        intervalMs: pollMs,
        describe: (last, error) =>
          `daemon socket ${socket.readyAt !== null ? "accepting again" : socket.refusedAt !== null ? "refusing" : "never refused"}; Plane states ${states.join(" → ") || "none"}; ${describeMain(last, error)}`,
      },
    );
    const readyAt = socket.readyAt;
    const newPid = await mainPid(host);
    const newStat = newPid === null ? null : proc.stat(newPid);
    const daemonStart = newStat ? processStartEpochMs(newStat, proc.uptime(), clock.now(), this.deps.clockTicks) : null;
    this.report.reconnect = {
      how: `systemctl --user kill ${UNIT} (SIGTERM to the unit; Restart=always)`,
      oldPid,
      newPid,
      killToDaemonReadyMs: readyAt === null ? null : round(readyAt - killedAt),
      killToDaemonStartMs: daemonStart === null ? null : round(daemonStart - killedAt),
      daemonReadyToUiConnectedMs: readyAt === null ? null : round(uiAt - readyAt),
      killToUiConnectedMs: round(uiAt - killedAt),
      planeStates: states,
      pollMs,
    };
    this.check("jetd restarted as a new process", newPid !== null && newPid !== oldPid, `${oldPid} -> ${newPid}`);
    this.check("the UI showed the Plane offline while jetd was down", states.some((state) => state !== "Connected"), states.join(" → "), false);
    this.check("the UI reconnected", view.planeState === "Connected", `${this.report.reconnect.killToUiConnectedMs} ms after the kill`);
    this.report.service = { ...this.report.service!, mainPid: newPid };
    await this.shot("reconnected");
  }

  private async relaunch(): Promise<void> {
    const { host } = this.deps;
    for (let index = 1; index <= this.options.warmLaunches; index += 1) {
      this.step = `quit before warm launch ${index}`;
      const daemonBefore = await mainPid(host);
      await this.quitApp();
      const daemonAfter = await mainPid(host);
      this.check("jetd keeps running when the app quits", daemonAfter !== null && daemonAfter === daemonBefore, `${daemonBefore} -> ${daemonAfter}`);
      const record = await this.launch("warm");
      this.step = `warm launch ${index}`;
      await this.openSetup(3_000);
      const texts: string[] = [];
      const view = await this.setupConnected(texts, async () => undefined);
      const phases = servicePhases(view);
      record.servicePhases = phases.map((entry) => entry.phase);
      record.processToServiceRunningMs = this.sinceStart(phases.find((entry) => entry.phase === "running")?.at ?? null);
      record.processToPlaneConnectedMs = this.sinceStart(markEpoch(view, LOCAL_PLANE_CONNECTED));
      record.processToSetupConnectedMs = this.sinceStart(this.deps.clock.now());
      const plane = view.setup!.localPlane!;
      this.check(
        `warm launch ${index}: nothing was provisioned again`,
        !record.servicePhases.includes("installing") && !record.servicePhases.includes("updating"),
        record.servicePhases.join(" → "),
      );
      this.check(
        `warm launch ${index}: Setup reports no new setup`,
        !/set up on this computer|was updated/.test(plane.notice ?? ""),
        plane.notice ?? "no notice",
      );
      this.check(`warm launch ${index}: Setup shows the bundled core`, plane.coreVersion === this.options.expectVersion, plane.text);
      if (index === 1) {
        const previous = this.report.service!;
        const service = await this.checkService("the service after a relaunch");
        this.check("the relaunch kept current", service.currentTarget === previous.currentTarget, `${previous.currentTarget} -> ${service.currentTarget}`);
        this.check("the relaunch left jetd running", service.mainPid === daemonAfter, `${daemonAfter} -> ${service.mainPid}`);
        await this.shot("relaunch-connected");
      }
    }
  }

  /** Ends the session (WebKitWebDriver closes the app) and waits for the process to go. */
  private async quitApp(): Promise<void> {
    const { clock, host, proc } = this.deps;
    const pid = this.appPid;
    const session = this.session;
    this.session = null;
    this.mainHandle = null;
    this.settingsHandle = null;
    const quitAt = clock.now();
    if (session) {
      try {
        await session.delete(this.options.timeouts.quitMs);
      } catch (error) {
        this.deps.log(`ending the WebDriver session: ${errorText(error)}`);
      }
    }
    if (pid === null) return;
    this.appPid = null;
    const gone = () => {
      const stat = proc.stat(pid);
      return stat === null || stat.state === "Z" || stat.comm !== this.options.appCommand;
    };
    const exited = (timeoutMs: number) =>
      waitFor(clock, "the app to exit", async () => (gone() ? true : undefined), { timeoutMs, intervalMs: 100 }).then(
        () => true,
        () => false,
      );
    let forced = false;
    if (!(await exited(this.options.timeouts.quitMs))) {
      forced = true;
      this.deps.log(`the app (pid ${pid}) outlived its session; sending SIGTERM`);
      host.kill(pid, "SIGTERM");
      if (!(await exited(5_000))) {
        host.kill(pid, "SIGKILL");
        await exited(5_000);
      }
    }
    this.report.quits.push({ forced, exitMs: round(clock.now() - quitAt) });
  }

  /** Records a failed step with screenshots, page text and machine diagnostics. */
  private async failed(error: unknown): Promise<void> {
    const failure = { step: this.step, message: errorText(error) };
    this.report.failures.push(failure);
    this.deps.log(`FAILED during ${failure.step}: ${failure.message}`);
    const number = this.report.failures.length;
    const session = this.session;
    if (session) {
      for (const [label, handle] of [
        ["main", this.mainHandle],
        ["settings", this.settingsHandle],
      ] as const) {
        if (!handle) continue;
        try {
          await session.switchToWindow(handle);
          await this.shot(`failure-${number}-${label}`);
          const text = await session.execute<string>(PAGE_SCRIPT, ["text"]);
          this.deps.save(`failure-${number}-${label}.txt`, String(text));
        } catch (captureError) {
          this.deps.log(`could not capture the ${label} window: ${errorText(captureError)}`);
        }
      }
    }
    try {
      const machine = await diagnostics(this.deps.host);
      this.diagnosticsText += `### Failure ${number}: ${failure.step}\n${failure.message}\n\n${machine}\n`;
      this.deps.save("diagnostics.txt", this.diagnosticsText);
    } catch (diagnosticsError) {
      this.deps.log(`could not collect diagnostics: ${errorText(diagnosticsError)}`);
    }
  }

  /** After a failed independent step, the next one starts from the main window. */
  private async backToMain(): Promise<void> {
    if (!this.session || !this.mainHandle) return;
    try {
      await this.session.switchToWindow(this.mainHandle);
    } catch (error) {
      this.deps.log(`could not return to the main window: ${errorText(error)}`);
    }
  }
}

// -- Pure helpers -------------------------------------------------------------

/** Wall-clock time of the first mark `name` at or after `after`, or null. */
export function markEpoch(view: MainView, name: string, after = -Infinity): number | null {
  for (const mark of view.marks) {
    const at = view.timeOrigin + mark.startTime;
    if (mark.name === name && at >= after) return at;
  }
  return null;
}

/** The `local-service-<phase>` marks, in order, as wall-clock times. */
export function servicePhases(view: MainView): Array<{ phase: string; at: number }> {
  return view.marks
    .filter((mark) => mark.name.startsWith(SERVICE_MARK))
    .map((mark) => ({ phase: mark.name.slice(SERVICE_MARK.length), at: view.timeOrigin + mark.startTime }))
    .sort((a, b) => a.at - b.at);
}

function lastPhase(view: MainView): string | null {
  const phases = servicePhases(view);
  return phases[phases.length - 1]?.phase ?? null;
}

export function describeMain(view: MainView | undefined, error: unknown): string {
  if (!view) return error ? `no answer from the page: ${errorText(error)}` : "no answer from the page";
  const setup = view.setup;
  const parts = [
    `title "${view.title}"`,
    `marks ${view.marks.map((mark) => mark.name).join(", ") || "none"}`,
    `Plane status "${view.planeStatus ?? "none"}"`,
  ];
  if (!setup) parts.push("Setup not shown");
  else if (setup.localPlane) parts.push(`Local Plane "${setup.localPlane.text}"`);
  else if (setup.provisioning) parts.push(`Setup provisioning "${setup.provisioning}"`);
  else if (setup.failure) parts.push(`Setup alert "${setup.failure.text}"`);
  else parts.push("Setup loading");
  if (error) parts.push(`last error ${errorText(error)}`);
  return parts.join("; ");
}

export function describeSettings(view: SettingsView | undefined, error: unknown): string {
  if (!view) return error ? `no answer from the Settings window: ${errorText(error)}` : "no answer from the Settings window";
  const parts = [
    `title "${view.title}"`,
    `panes ${view.panes.map((pane) => `${pane.name}${pane.current ? " (current)" : ""}${pane.disabled ? " (disabled)" : ""}`).join(", ") || "none"}`,
    `pane "${view.pane ?? "none"}"`,
  ];
  if (!view.versions) parts.push("Versions not shown");
  else {
    parts.push(
      `service ${JSON.stringify(view.versions.service?.facts ?? null)}`,
      `updates "${view.versions.updates ?? "none"}"`,
      `update controls ${JSON.stringify(view.versions.updateControls)}`,
    );
  }
  if (error) parts.push(`last error ${errorText(error)}`);
  return parts.join("; ");
}

export function proposals(report: JourneyReport): JourneyReport["proposals"] {
  const cold = report.launches.find((launch) => launch.kind === "cold")?.processToInteractiveMs ?? null;
  const warm = report.launches.filter((launch) => launch.kind === "warm").map((launch) => launch.processToInteractiveMs);
  const row = (measurement: string, value: number | null, ceiling: number, unit: string) => ({
    measurement,
    value,
    ceiling,
    unit,
    within: value === null ? null : value <= ceiling,
  });
  return [
    row("cold launch to shell-interactive", cold, PROPOSED_CEILINGS.coldLaunchMs, "ms"),
    row("warm launch to shell-interactive (slowest sample)", warm.length > 0 ? Math.max(...warm) : null, PROPOSED_CEILINGS.warmLaunchMs, "ms"),
    row("idle app-tree CPU", report.idle?.app.cpuPercent ?? null, PROPOSED_CEILINGS.idleAppCpuPercent, "%"),
    row("reconnect after the daemon is ready", report.reconnect?.daemonReadyToUiConnectedMs ?? null, PROPOSED_CEILINGS.reconnectAfterReadyMs, "ms"),
  ];
}

/** A short Markdown summary for `$GITHUB_STEP_SUMMARY`. */
export function summaryMarkdown(report: JourneyReport): string {
  const kib = (value: number | null | undefined) => (value === null || value === undefined ? "n/a" : `${(value / 1024).toFixed(1)} MiB`);
  const lines = [
    `### Desktop journey (${report.app.version}, ${report.app.signed ? "signed" : "unsigned"}): ${report.result}`,
    "",
  ];
  for (const failure of report.failures) lines.push(`Failed during **${failure.step}**: ${failure.message}`, "");
  lines.push("| Measurement | Value | Proposed ceiling |", "| --- | ---: | ---: |");
  for (const launch of report.launches) {
    lines.push(`| ${launch.kind} launch to shell-interactive | ${launch.processToInteractiveMs} ms | ${launch.kind === "cold" ? PROPOSED_CEILINGS.coldLaunchMs : PROPOSED_CEILINGS.warmLaunchMs} ms |`);
  }
  if (report.provisioning) lines.push(`| cold launch to service running (provisioning) | ${report.provisioning.processToRunningMs ?? "n/a"} ms | – |`);
  for (const launch of report.launches) {
    lines.push(`| ${launch.kind} launch to local Plane connected | ${launch.processToPlaneConnectedMs ?? "n/a"} ms | – |`);
  }
  if (report.idle) {
    lines.push(
      `| idle app-tree CPU (${Math.round(report.idle.windowMs / 1000)} s) | ${report.idle.app.cpuPercent}% | ${PROPOSED_CEILINGS.idleAppCpuPercent}% |`,
      `| idle app-tree PSS (mean) | ${kib(report.idle.app.pss?.meanKiB)} | baseline |`,
      `| idle jetd CPU | ${report.idle.jetd.cpuPercent}% | – |`,
      `| idle jetd PSS (mean) | ${kib(report.idle.jetd.pss?.meanKiB)} | baseline |`,
    );
  }
  if (report.reconnect) {
    lines.push(
      `| kill to daemon ready | ${report.reconnect.killToDaemonReadyMs ?? "n/a"} ms | – |`,
      `| daemon ready to UI reconnected | ${report.reconnect.daemonReadyToUiConnectedMs ?? "n/a"} ms | ${PROPOSED_CEILINGS.reconnectAfterReadyMs} ms |`,
    );
  }
  lines.push("", "Ceilings are proposals and are not enforced. Checks:", "");
  for (const check of report.checks) {
    lines.push(`- ${check.passed ? "pass" : check.required ? "**FAIL**" : "note"}: ${check.name}: ${check.detail}`);
  }
  return `${lines.join("\n")}\n`;
}

function round(value: number): number {
  return Math.round(value);
}

function isPresent(path: string): boolean {
  try {
    lstatSync(path);
    return true;
  } catch {
    return false;
  }
}

function isExecutable(path: string): boolean {
  try {
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function readLink(path: string): string | null {
  try {
    return readlinkSync(path);
  } catch {
    return null;
  }
}

function osRelease(): string | null {
  try {
    return /^PRETTY_NAME="?([^"\n]*)"?$/m.exec(readFileSync("/etc/os-release", "utf8"))?.[1] ?? null;
  } catch {
    return null;
  }
}
