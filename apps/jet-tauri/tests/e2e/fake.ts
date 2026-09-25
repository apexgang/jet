/**
 * Stand-ins for everything the journey talks to, for its dry run and its
 * tests: a WebDriver server whose "app" follows a scripted timeline, and a
 * host whose systemd and `jetd core status` answer from a scratch home. The
 * fake app writes the core layout and unit the real app would, so the
 * journey's filesystem checks run unchanged. Nothing here starts the real
 * app, systemd or brew, or writes outside the scratch home it is given.
 */
import { chmodSync, existsSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { dirname } from "node:path";

import type { Clock } from "./clock";
import { servicePaths, UNIT, type Host, type RunResult } from "./host";
import { PAGE_SCRIPT, type ControlQuery, type MainView, type Mark, type SettingsView } from "./page";
import { ELEMENT_KEY } from "./webdriver";

/** A 1×1 PNG, the fake screenshot. */
export const FAKE_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

/** Milliseconds after the app process starts. */
export type FakeTimings = {
  pageMs: number;
  interactiveMs: number;
  checkingMs: number;
  /** Fresh launch only. */
  installingMs: number;
  runningMs: number;
  warmRunningMs: number;
  /** The daemon answers the feed this long before `running` on a warm launch. */
  warmConnectedMs: number;
  /** After a restarted daemon accepts again. */
  reconnectMs: number;
  settingsOpenMs: number;
};

export const DEFAULT_TIMINGS: FakeTimings = {
  pageMs: 100,
  interactiveMs: 400,
  checkingMs: 600,
  installingMs: 900,
  runningMs: 2_400,
  warmRunningMs: 700,
  warmConnectedMs: 450,
  reconnectMs: 300,
  settingsOpenMs: 300,
};

export type Outage = { from: number; to: number };

export type FakeProcess = { pid: number; stop(): void };

export type FakeAppOptions = {
  clock: Clock;
  home: string;
  configHome: string;
  version: string;
  signed: boolean;
  timings?: Partial<FakeTimings>;
  /** A fresh launch fails with this `service.*` code instead of running. */
  failure?: string | null;
  /** Setup never reads the Plane (a hung daemon). */
  neverConnect?: boolean;
  /** The Settings window never appears among the window handles. */
  hideSettingsWindow?: boolean;
  /** After a daemon restart this launch's UI stays offline. */
  staysOffline?: boolean;
  /** What App updates shows instead of what `signed` implies. */
  updates?: { status: string; controls: string[] };
  startProcess: () => FakeProcess;
};

type Launch = {
  at: number;
  fresh: boolean;
  process: FakeProcess;
  setupOpen: boolean;
  settings: { handle: string; openedAt: number; pane: string } | null;
  window: string;
};

const MAIN = "main-window";
const SERVICE_TEXT: Record<string, string> = {
  checking: "Checking the Jet service on this computer…",
  installing: "Setting up the Jet service on this computer…",
};

export class FakeApp {
  readonly timings: FakeTimings;
  private launch: Launch | null = null;
  private launches = 0;
  /** The saved layout: Setup was the destination when the app last quit. */
  private setupSaved = false;
  private outages: Outage[] = [];
  readonly clicks: string[] = [];

  constructor(private readonly options: FakeAppOptions) {
    this.timings = { ...DEFAULT_TIMINGS, ...options.timings };
  }

  private get now(): number {
    return this.options.clock.now();
  }

  private get paths() {
    return servicePaths({ home: this.options.home, configHome: this.options.configHome });
  }

  get running(): boolean {
    return this.launch !== null;
  }

  start(): string {
    if (this.launch) throw new Error("the fake app is already running");
    this.launches += 1;
    this.launch = {
      at: this.now,
      fresh: !existsSync(this.paths.current),
      process: this.options.startProcess(),
      setupOpen: this.setupSaved,
      settings: null,
      window: MAIN,
    };
    return `session-${this.launches}`;
  }

  stop(): void {
    if (this.launch) this.setupSaved = this.launch.setupOpen;
    this.launch?.process.stop();
    this.launch = null;
  }

  outage(outage: Outage): void {
    this.outages.push(outage);
  }

  private get current(): Launch {
    if (!this.launch) throw new Error("the fake app is not running");
    return this.launch;
  }

  /** When the service watcher reached `running` (or failed), after launch. */
  private get settledMs(): number {
    return this.current.fresh ? this.timings.runningMs : this.timings.warmRunningMs;
  }

  private elapsed(): number {
    return this.now - this.current.at;
  }

  /** Writes what provisioning leaves behind, once the fake pass finished. */
  private provisionOnce(): void {
    const launch = this.current;
    if (!launch.fresh || this.options.failure || this.elapsed() < this.timings.runningMs) return;
    const paths = this.paths;
    if (existsSync(paths.current)) return;
    const version = `${this.options.home}/.jet/core/versions/${this.options.version}`;
    mkdirSync(version, { recursive: true });
    writeFileSync(`${version}/jetd`, "#!/bin/sh\nexit 0\n");
    chmodSync(`${version}/jetd`, 0o755);
    symlinkSync(`versions/${this.options.version}`, paths.current);
    mkdirSync(dirname(paths.unit), { recursive: true });
    writeFileSync(paths.unit, "[Service]\nExecStart=%h/.jet/core/current/jetd serve --channel gui\n");
    mkdirSync(dirname(paths.socket), { recursive: true });
  }

  private marks(): { timeOrigin: number; marks: Mark[] } {
    const launch = this.current;
    const t = this.timings;
    const timeOrigin = launch.at + t.pageMs;
    const marks: Mark[] = [];
    const add = (name: string, afterLaunch: number) => {
      if (this.elapsed() >= afterLaunch) marks.push({ name, startTime: afterLaunch - t.pageMs });
    };
    add("shell-interactive", t.interactiveMs);
    add("local-service-checking", t.checkingMs);
    if (launch.fresh) add("local-service-installing", t.installingMs);
    add(this.options.failure && launch.fresh ? "local-service-failed" : "local-service-running", this.settledMs);
    if (!(this.options.failure && launch.fresh) && !this.options.neverConnect) {
      add("local-plane-connected", launch.fresh ? t.runningMs - 50 : t.warmConnectedMs);
      // After a daemon restart the real feed says `reconnecting`, then
      // `connected` with the new daemon's status, then `resumed`; the app
      // marks once and reads the Plane registry again on `connected`. The
      // shell tests drive that sequence
      // (shell-timing.test.ts, e2e-page.component.test.ts); this fake only
      // shows its outcome.
      for (const outage of this.outages) {
        if (outage.from >= launch.at && !this.options.staysOffline) add("local-plane-connected", outage.to + t.reconnectMs - launch.at);
      }
    }
    marks.sort((a, b) => a.startTime - b.startTime);
    return { timeOrigin, marks };
  }

  private connected(): boolean {
    const launch = this.current;
    if ((this.options.failure && launch.fresh) || this.options.neverConnect) return false;
    const at = this.now;
    const first = launch.at + (launch.fresh ? this.timings.runningMs - 50 : this.timings.warmConnectedMs);
    if (at < first) return false;
    if (this.options.staysOffline && this.outages.some((outage) => outage.from >= launch.at && at >= outage.from)) return false;
    return !this.outages.some((outage) => at >= outage.from && at < outage.to + this.timings.reconnectMs);
  }

  mainView(): MainView {
    const launch = this.current;
    this.provisionOnce();
    const { timeOrigin, marks } = this.marks();
    const elapsed = this.elapsed();
    const version = this.options.version;
    const status = this.connected() ? "Connected" : elapsed < this.settledMs ? "Connecting" : "Reconnecting";
    let setup: MainView["setup"] = null;
    if (launch.setupOpen) {
      const phase = [...marks].reverse().find((mark) => mark.name.startsWith("local-service-"))?.name.slice(14) ?? null;
      const ready = !this.options.neverConnect && this.connected() && elapsed >= this.settledMs;
      if (ready) {
        const notice = launch.fresh ? `The Jet service ${version} is set up on this computer.` : null;
        setup = {
          provisioning: null,
          failure: null,
          localPlane: {
            text: `Local Plane linux · Core ${version}${notice ? ` ${notice}` : ""} Connected`,
            coreVersion: version,
            status: "Connected",
            notice,
          },
        };
      } else if (phase === "failed") {
        const code = this.options.failure ?? "service.install_failed";
        setup = {
          provisioning: null,
          failure: {
            title: "The Jet service needs attention",
            text: `The Jet service needs attention Jet couldn't set up the service. ${code} Repair Try again`,
          },
          localPlane: null,
        };
      } else if (phase !== null && SERVICE_TEXT[phase]) {
        setup = { provisioning: SERVICE_TEXT[phase], failure: null, localPlane: null };
      } else {
        setup = {
          provisioning: null,
          failure: { title: "Local Plane unavailable", text: "Local Plane unavailable Jet could not reach this Plane. transport.offline Try again" },
          localPlane: null,
        };
      }
    }
    return {
      readyState: "complete",
      title: "Jet",
      timeOrigin,
      marks,
      planeStatus: `This computer ${status}`,
      planeState: status,
      composer: !launch.setupOpen,
      setup,
    };
  }

  settingsView(): SettingsView {
    const launch = this.current;
    const settings = launch.settings!;
    const loaded = this.now - settings.openedAt >= this.timings.settingsOpenMs;
    const version = this.options.version;
    const panes = ["General", "Agents", "Work", "Connections", "Safety and system"].map((name) => ({
      name,
      current: loaded && name === settings.pane,
      disabled: !loaded,
    }));
    return {
      readyState: "complete",
      title: "Jet Settings",
      panes,
      pane: loaded ? settings.pane : null,
      versions:
        loaded && settings.pane === "Safety and system"
          ? {
              service: {
                facts: {
                  "Managed by": "Managed by this app Starts when you log in (systemd user service)",
                  Status: "Running",
                  Version: version,
                  "Included with this app": version,
                },
                live: null,
                buttons: [],
              },
              updates:
                this.options.updates?.status ??
                (this.options.signed
                  ? `Jet ${version} is up to date.`
                  : "This is a development build, so it doesn't update itself."),
              updateControls:
                this.options.updates?.controls ??
                (this.options.signed ? ["Check for updates", "Check for updates automatically"] : []),
              thisApp: `This app ${version}, supports protocol 37.`,
            }
          : null,
    };
  }

  handles(): string[] {
    const launch = this.current;
    return launch.settings && !this.options.hideSettingsWindow ? [MAIN, launch.settings.handle] : [MAIN];
  }

  windowHandle(): string {
    return this.current.window;
  }

  switchTo(handle: string): boolean {
    if (!this.handles().includes(handle)) return false;
    this.current.window = handle;
    return true;
  }

  title(): string {
    return this.current.window === MAIN ? "Jet" : "Jet Settings";
  }

  /** The element id a button query finds in the current window, or null. */
  button(query: ControlQuery): string | null {
    const launch = this.current;
    const inMain = launch.window === MAIN;
    for (const name of query.names) {
      if (inMain && query.scope?.name === "Jet navigation") {
        if (name === "Add a Project" || name === "Manage Projects" || name === "Settings") return `button:${name}`;
      }
      if (!inMain && query.scope?.name === "Settings" && name === "Safety and system") return `button:${name}`;
    }
    return null;
  }

  click(element: string): boolean {
    const launch = this.current;
    this.clicks.push(element);
    switch (element) {
      case "button:Add a Project":
      case "button:Manage Projects":
        launch.setupOpen = true;
        return true;
      case "button:Settings":
        launch.settings ??= { handle: `settings-window-${this.launches}`, openedAt: this.now, pane: "General" };
        return true;
      case "button:Safety and system":
        if (launch.settings) launch.settings.pane = "Safety and system";
        return true;
      default:
        return false;
    }
  }
}

// -- WebDriver server ---------------------------------------------------------

type Reply = { status: number; value: unknown };

const error = (status: number, code: string, message: string): Reply => ({ status, value: { error: code, message, stacktrace: "" } });

/**
 * Serves the WebDriver commands the journey sends, answering from `app`.
 * The only script it takes is the journey's page script, compared as text
 * and never run; its first argument picks the answer.
 */
export function fakeDriver(app: FakeApp, onControl: (path: string, body: unknown) => void = () => undefined): Server {
  const sessions = new Set<string>();
  const route = (method: string, path: string, body: Record<string, unknown>): Reply => {
    if (method === "GET" && path === "/status") return { status: 200, value: { ready: true, message: "fake driver" } };
    if (method === "POST" && path.startsWith("/fake/")) {
      onControl(path, body);
      return { status: 200, value: null };
    }
    if (method === "POST" && path === "/session") {
      const options = (body.capabilities as { alwaysMatch?: Record<string, { application?: unknown }> } | undefined)?.alwaysMatch?.[
        "tauri:options"
      ];
      if (typeof options?.application !== "string") return error(400, "invalid argument", "tauri:options.application is required");
      if (app.running) return error(500, "session not created", "the app is already running");
      const id = app.start();
      sessions.add(id);
      return { status: 200, value: { sessionId: id, capabilities: { browserName: "fake" } } };
    }
    const match = /^\/session\/([^/]+)(\/.*)?$/.exec(path);
    if (!match || !sessions.has(match[1])) return error(404, "invalid session id", `no session at ${path}`);
    const [, id, rest = ""] = match;
    const command = `${method} ${rest}`;
    if (command === "DELETE ") {
      sessions.delete(id);
      app.stop();
      return { status: 200, value: null };
    }
    if (command === "POST /timeouts") return { status: 200, value: null };
    if (command === "GET /window") return { status: 200, value: app.windowHandle() };
    if (command === "GET /window/handles") return { status: 200, value: app.handles() };
    if (command === "POST /window") {
      return app.switchTo(String(body.handle)) ? { status: 200, value: null } : error(404, "no such window", String(body.handle));
    }
    if (command === "GET /title") return { status: 200, value: app.title() };
    if (command === "GET /url") return { status: 200, value: app.title() === "Jet" ? "tauri://localhost/" : "tauri://localhost/settings" };
    if (command === "GET /screenshot") return { status: 200, value: FAKE_PNG };
    if (command === "POST /execute/sync") {
      if (body.script !== PAGE_SCRIPT) return error(500, "javascript error", "not the journey's page script");
      const [pageCommand, argument] = (body.args as unknown[]) ?? [];
      switch (pageCommand) {
        case "main":
          return app.windowHandle() === MAIN ? { status: 200, value: app.mainView() } : error(500, "javascript error", "not the main window");
        case "settings":
          return app.windowHandle() !== MAIN ? { status: 200, value: app.settingsView() } : error(500, "javascript error", "not the Settings window");
        case "button": {
          const element = app.button(argument as ControlQuery);
          return { status: 200, value: element === null ? null : { [ELEMENT_KEY]: element } };
        }
        case "reveal":
          return { status: 200, value: true };
        case "text":
          return { status: 200, value: `fake ${app.title()} window text` };
        default:
          return error(500, "javascript error", `unknown page command ${String(pageCommand)}`);
      }
    }
    const click = /^POST \/element\/([^/]+)\/click$/.exec(command);
    if (click) {
      return app.click(decodeURIComponent(click[1])) ? { status: 200, value: null } : error(404, "no such element", click[1]);
    }
    return error(404, "unknown command", command);
  };
  return createServer((request: IncomingMessage, response: ServerResponse) => {
    let raw = "";
    request.on("data", (chunk: Buffer) => {
      raw += chunk.toString("utf8");
    });
    request.on("end", () => {
      let reply: Reply;
      try {
        const body = raw === "" ? {} : (JSON.parse(raw) as Record<string, unknown>);
        reply = route(request.method ?? "GET", new URL(request.url ?? "/", "http://fake").pathname, body);
      } catch (cause) {
        reply = error(500, "unknown error", (cause as Error).message);
      }
      response.writeHead(reply.status, { "content-type": "application/json; charset=utf-8" });
      response.end(JSON.stringify({ value: reply.value }));
    });
  });
}

// -- Host ---------------------------------------------------------------------

export type FakeHostOptions = {
  clock: Clock;
  version: string;
  application: string;
  /** Starts the daemon's process once the fake app wrote `current`. */
  startDaemon: () => FakeProcess;
  /** The unit's `RestartSec`. */
  restartMs?: number;
  /** Tells the fake app that the daemon is down for `outage`. */
  onOutage: (outage: Outage) => void | Promise<void>;
};

/** systemd, `jetd core status`, dpkg and the daemon socket, answered from a scratch home. */
export class FakeHost implements Host {
  readonly configHome: string;
  readonly calls: string[][] = [];
  readonly kills: Array<{ pid: number; signal: string }> = [];
  private daemon: FakeProcess | null = null;
  private outages: Outage[] = [];

  constructor(
    readonly home: string,
    private readonly options: FakeHostOptions,
  ) {
    this.configHome = `${home}/.config`;
  }

  private get now(): number {
    return this.options.clock.now();
  }

  private get paths() {
    return servicePaths(this);
  }

  private up(): boolean {
    if (!existsSync(this.paths.current)) return false;
    const at = this.now;
    return !this.outages.some((outage) => at >= outage.from && at < outage.to);
  }

  /** The daemon process while the service is up; a new one after each outage. */
  private process(): FakeProcess | null {
    if (!this.up()) {
      this.stopDaemon();
      return null;
    }
    this.daemon ??= this.options.startDaemon();
    return this.daemon;
  }

  stopDaemon(): void {
    this.daemon?.stop();
    this.daemon = null;
  }

  async run(argv: readonly string[]): Promise<RunResult> {
    this.calls.push([...argv]);
    const ok = (stdout: string): RunResult => ({ code: 0, stdout, stderr: "", error: null });
    const fail = (code: number, stderr: string): RunResult => ({ code, stdout: "", stderr, error: null });
    const line = argv.join(" ");
    const version = this.options.version;
    const provisioned = existsSync(this.paths.current);
    switch (line) {
      case "systemctl --user show-environment":
        return ok("HOME=/fake\n");
      case `systemctl --user show ${UNIT} --property=MainPID --value`:
        return ok(`${this.process()?.pid ?? 0}\n`);
      case `systemctl --user is-enabled ${UNIT}`:
        return provisioned ? ok("enabled\n") : fail(1, "");
      case `systemctl --user is-active ${UNIT}`:
        return this.process() ? ok("active\n") : { code: 3, stdout: provisioned ? "activating\n" : "inactive\n", stderr: "", error: null };
      case `systemctl --user kill ${UNIT}`: {
        if (!this.process()) return fail(1, "Failed to kill unit jetd.service: No main process");
        const outage = { from: this.now, to: this.now + (this.options.restartMs ?? 2_000) };
        this.outages.push(outage);
        this.stopDaemon();
        await this.options.onOutage(outage);
        return ok("");
      }
      case `${this.paths.currentJetd} core status`: {
        const daemon = this.process();
        return ok(
          `${JSON.stringify({
            status: "ok",
            current: provisioned ? version : null,
            previous: null,
            staged: provisioned ? [version] : [],
            live_helpers: [],
            owner: daemon ? { state: "held", daemon: { pid: daemon.pid, version, channel: "gui" } } : { state: "free" },
          })}\n`,
        );
      }
      case "uname -r":
        return ok("6.8.0-fake\n");
      case `dpkg-query --search ${this.options.application}`:
        return ok(`jet: ${this.options.application}\n`);
      case "dpkg-query --show --showformat=${Version} libwebkit2gtk-4.1-0":
        return ok("2.48.0-fake");
      case "dpkg-query --show --showformat=${Version} jet":
        return ok(version);
      default:
        if (argv[0] === "cat" && argv.length === 2) {
          return existsSync(argv[1]) ? ok(readFileSync(argv[1], "utf8")) : fail(1, `cat: ${argv[1]}: No such file`);
        }
        if (argv.includes("journalctl") || argv[0] === "journalctl") return ok("-- fake journal --\n");
        if (argv[0] === "systemctl" || argv[0] === "ls") return ok(`(fake) ${line}\n`);
        return { code: null, stdout: "", stderr: "", error: `not faked: ${line}` };
    }
  }

  async socketAccepts(): Promise<boolean> {
    return this.process() !== null;
  }

  kill(pid: number, signal: "SIGTERM" | "SIGKILL"): boolean {
    this.kills.push({ pid, signal });
    return false;
  }
}
