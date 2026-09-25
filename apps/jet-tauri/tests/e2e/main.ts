/**
 * Runs the Linux desktop release journey (journey.ts).
 *
 *   bun tests/e2e/main.ts --driver <tauri-driver> --out <dir> [--signed]
 *       [--version <v>] [--application /usr/bin/jet-tauri]
 *       [--payload /usr/lib/Jet/jet-core.tar.gz] [--port 4444]
 *       [--settle-seconds 15] [--idle-seconds 60] [--warm-launches 3]
 *   bun tests/e2e/main.ts --dry-run [--out <dir>] [--quiet]
 *
 * The journey itself runs in CI only, on a fresh runner with the `.deb`
 * installed and a display (xvfb-run): the app it launches provisions the
 * real `~/.jet` and systemd user unit of whoever runs it, so it refuses to
 * start outside GitHub Actions unless `--disposable-machine` is given.
 *
 * `--dry-run` runs everything except the app: the WebDriver client against
 * a fake driver process (this file's `fake-driver` mode) whose app is a
 * `sleep` under another name and which takes only the journey's page
 * script, /proc sampling of real processes, a fake systemd and `jetd core`
 * answering from a scratch home, screenshots, diagnostics and the report.
 * It never touches the real home. `--quiet` prints progress only when it
 * fails.
 *
 * The version defaults to `packages/Cargo.toml`'s workspace version. The
 * report is printed to stdout and written to `<out>/desktop-e2e.json`;
 * progress goes to stderr. Exit status: 0 passed, 1 a check failed, 2 the
 * journey could not run.
 */
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import {
  appendFileSync,
  closeSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { tomlString } from "../../scripts/release/checks";
import { errorText, realClock, waitFor } from "./clock";
import { FakeApp, FakeHost, fakeDriver, type FakeProcess } from "./fake";
import { SystemHost } from "./host";
import { DEFAULT_OPTIONS, runJourney, summaryMarkdown, type JourneyOptions, type JourneyReport } from "./journey";
import { ProcFs } from "./procfs";
import { WebDriver } from "./webdriver";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPOSITORY = resolve(HERE, "../../../..");

class UsageError extends Error {}

type Arguments = Map<string, string | true>;

function parseArguments(argv: readonly string[]): Arguments {
  const flags = new Set(["--dry-run", "--signed", "--disposable-machine", "--quiet"]);
  const values = new Set([
    "--driver",
    "--out",
    "--version",
    "--application",
    "--payload",
    "--port",
    "--settle-seconds",
    "--idle-seconds",
    "--warm-launches",
    "--home",
    "--app",
  ]);
  const parsed: Arguments = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const name = argv[index];
    if (flags.has(name)) {
      parsed.set(name, true);
    } else if (values.has(name)) {
      const value = argv[index + 1];
      if (value === undefined || value.startsWith("--")) throw new UsageError(`${name} needs a value`);
      parsed.set(name, value);
      index += 1;
    } else {
      throw new UsageError(`unknown argument ${name}`);
    }
  }
  return parsed;
}

function text(args: Arguments, name: string): string | undefined {
  const value = args.get(name);
  return typeof value === "string" ? value : undefined;
}

function count(args: Arguments, name: string, fallback: number): number {
  const value = text(args, name);
  if (value === undefined) return fallback;
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed < 0) throw new UsageError(`${name} must be a non-negative number`);
  return parsed;
}

function workspaceVersion(): string {
  const version = tomlString(readFileSync(join(REPOSITORY, "packages/Cargo.toml"), "utf8"), "workspace.package", "version");
  if (!version) throw new UsageError("packages/Cargo.toml has no [workspace.package] version; pass --version");
  return version;
}

/** Progress lines; with `--quiet` they are held back and printed only on failure. */
const held: string[] = [];
let quiet = false;

function log(line: string): void {
  if (quiet) held.push(`[journey] ${line}`);
  else console.error(`[journey] ${line}`);
}

function saver(out: string) {
  return (name: string, data: Uint8Array | string): string => {
    writeFileSync(join(out, name), data);
    return name;
  };
}

/** A TCP port nothing listens on right now. */
function freePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => (typeof address === "object" && address ? resolvePort(address.port) : reject(new Error("no port"))));
    });
  });
}

/** Starts a WebDriver server process with its output in `logFile`, and waits for `/status`. */
async function startDriver(argv: string[], logFile: string, port: number): Promise<{ child: ChildProcess; driver: WebDriver }> {
  const output = openSync(logFile, "a");
  const child = spawn(argv[0], argv.slice(1), { stdio: ["ignore", output, output] });
  closeSync(output);
  const state: { failed: string | null } = { failed: null };
  child.once("error", (error) => {
    state.failed = error.message;
  });
  child.once("exit", (code, signal) => {
    state.failed ??= `exited early (${signal ?? `code ${code}`})`;
  });
  const driver = new WebDriver(`http://127.0.0.1:${port}`);
  try {
    await waitFor(
      realClock,
      `${basename(argv[0])} to answer /status`,
      async () => {
        if (state.failed) return true;
        await driver.status();
        return true;
      },
      { timeoutMs: 30_000, intervalMs: 250, describe: (_, error) => (error ? errorText(error) : "no answer") },
    );
  } catch (error) {
    child.kill("SIGKILL");
    throw error;
  }
  if (state.failed) throw new UsageError(`${basename(argv[0])} ${state.failed}; see ${logFile}`);
  return { child, driver };
}

async function stopProcess(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise<void>((resolveExit) => child.once("exit", () => resolveExit()));
  child.kill("SIGTERM");
  const timer = setTimeout(() => child.kill("SIGKILL"), 5_000);
  await exited;
  clearTimeout(timer);
}

/** `getconf CLK_TCK`, or Linux's 100. */
function clockTicks(): number {
  const result = spawnSync("getconf", ["CLK_TCK"], { encoding: "utf8" });
  const ticks = Number(result.stdout?.trim());
  return Number.isSafeInteger(ticks) && ticks > 0 ? ticks : 100;
}

/**
 * Writes the report and summary. `real` marks the journey proper: only it
 * writes the step summary and workflow annotations, never the dry run.
 */
function finish(report: JourneyReport, out: string, extraLogs: string[], real: boolean): number {
  const json = `${JSON.stringify(report, null, 2)}\n`;
  writeFileSync(join(out, "desktop-e2e.json"), json);
  const summary = summaryMarkdown(report);
  writeFileSync(join(out, "summary.md"), summary);
  if (real && process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
  process.stdout.write(json);
  if (quiet && report.result !== "passed") console.error(held.join("\n"));
  // Workflow annotations: a recorded-only check that did not hold, and each failed step.
  if (real) {
    for (const check of report.checks.filter((entry) => !entry.passed && !entry.required)) {
      console.error(`::notice::${check.name}: ${check.detail}`);
    }
  }
  if (report.result === "passed") return 0;
  for (const file of [join(out, "diagnostics.txt"), ...extraLogs]) {
    if (!existsSync(file)) continue;
    const lines = readFileSync(file, "utf8").split("\n");
    console.error(`\n--- ${basename(file)} (last 200 lines) ---\n${lines.slice(-200).join("\n")}`);
  }
  for (const failure of report.failures) {
    console.error(`${real ? "::error::" : "error: "}Desktop journey failed during ${failure.step}: ${failure.message}`);
  }
  return 1;
}

// -- Modes --------------------------------------------------------------------

async function journey(args: Arguments): Promise<number> {
  if (process.env.GITHUB_ACTIONS !== "true" && !args.get("--disposable-machine")) {
    throw new UsageError(
      "the journey provisions this user's real ~/.jet and systemd unit; run it on a CI runner, pass --disposable-machine on a throwaway machine, or use --dry-run",
    );
  }
  const driverPath = text(args, "--driver");
  const outArgument = text(args, "--out");
  if (!driverPath || !outArgument) throw new UsageError("--driver and --out are required (or use --dry-run)");
  const out = resolve(outArgument);
  mkdirSync(out, { recursive: true });
  const port = count(args, "--port", 4444);
  const options: JourneyOptions = {
    ...DEFAULT_OPTIONS,
    application: text(args, "--application") ?? DEFAULT_OPTIONS.application,
    payload: text(args, "--payload") ?? DEFAULT_OPTIONS.payload,
    expectVersion: text(args, "--version") ?? workspaceVersion(),
    signed: args.get("--signed") === true,
    settleMs: count(args, "--settle-seconds", DEFAULT_OPTIONS.settleMs / 1000) * 1000,
    idleMs: count(args, "--idle-seconds", DEFAULT_OPTIONS.idleMs / 1000) * 1000,
    warmLaunches: count(args, "--warm-launches", DEFAULT_OPTIONS.warmLaunches),
  };
  const driverLog = join(out, "tauri-driver.log");
  log(`starting ${driverPath}; its and the app's stderr go to ${driverLog}`);
  const { child, driver } = await startDriver([driverPath, "--port", String(port), "--native-port", String(port + 1)], driverLog, port);
  let report: JourneyReport;
  try {
    report = await runJourney(options, {
      driver,
      host: new SystemHost(homedir(), process.env.XDG_CONFIG_HOME),
      proc: new ProcFs(),
      clock: realClock,
      clockTicks: clockTicks(),
      driverPid: child.pid ?? null,
      log,
      save: saver(out),
    });
  } finally {
    await stopProcess(child);
  }
  return finish(report, out, [driverLog], true);
}

function startSleeper(executable: string): FakeProcess {
  const child = spawn(executable, ["3600"], { stdio: "ignore" });
  return {
    pid: child.pid ?? 0,
    stop: () => {
      child.kill("SIGKILL");
    },
  };
}

async function dryRun(args: Arguments): Promise<number> {
  const requested = text(args, "--out");
  const out = resolve(requested ?? mkdtempSync(join(tmpdir(), "jet-e2e-dry-run-")));
  mkdirSync(out, { recursive: true });
  const home = join(out, "home");
  const bin = join(out, "bin");
  mkdirSync(home, { recursive: true });
  mkdirSync(bin, { recursive: true });
  const sleep = ["/usr/bin/sleep", "/bin/sleep"].find((candidate) => existsSync(candidate));
  if (!sleep) throw new UsageError("the dry run needs sleep(1)");
  // Distinct process names, so /proc lookups never match another program.
  const app = join(bin, "jet-fake-app");
  const jetd = join(bin, "jet-fake-jetd");
  for (const link of [app, jetd]) if (!existsSync(link)) symlinkSync(sleep, link);
  const payload = join(out, "jet-core.tar.gz");
  writeFileSync(payload, "not a real payload\n");
  const version = text(args, "--version") ?? workspaceVersion();
  const port = await freePort();
  const driverLog = join(out, "fake-driver.log");
  log(`dry run in ${out}: fake driver on port ${port}, fake home ${home}`);
  const driverArgs = [process.execPath, fileURLToPath(import.meta.url), "fake-driver", "--port", String(port), "--home", home, "--version", version, "--app", app];
  if (args.get("--signed")) driverArgs.push("--signed");
  const { child, driver } = await startDriver(driverArgs, driverLog, port);
  const host = new FakeHost(home, {
    clock: realClock,
    version,
    application: app,
    startDaemon: () => startSleeper(jetd),
    onOutage: async (outage) => {
      await fetch(`${driver.base}/fake/outage`, { method: "POST", body: JSON.stringify(outage) });
    },
  });
  let report: JourneyReport;
  try {
    report = await runJourney(
      {
        ...DEFAULT_OPTIONS,
        application: app,
        appCommand: basename(app),
        payload,
        expectVersion: version,
        signed: args.get("--signed") === true,
        settleMs: 500,
        idleMs: 3_000,
        sampleIntervalMs: 500,
        warmLaunches: 2,
      },
      {
        driver,
        host,
        proc: new ProcFs(),
        clock: realClock,
        clockTicks: clockTicks(),
        driverPid: child.pid ?? null,
        log,
        save: saver(out),
      },
    );
  } finally {
    host.stopDaemon();
    await stopProcess(child);
  }
  const status = finish(report, out, [driverLog], false);
  if (status === 0 && requested === undefined) {
    rmSync(out, { recursive: true, force: true });
    log("dry run passed: everything but the app ran");
  } else {
    log(`dry run ${report.result}: everything but the app ran; artifacts in ${out}`);
  }
  return status;
}

/** The fake WebDriver server the dry run starts as its driver process. */
async function fakeDriverMode(args: Arguments): Promise<number> {
  const port = count(args, "--port", 0);
  const home = text(args, "--home");
  const version = text(args, "--version");
  const app = text(args, "--app");
  if (!home || !version || !app) throw new UsageError("fake-driver needs --home, --version and --app");
  const fake = new FakeApp({
    clock: realClock,
    home,
    configHome: join(home, ".config"),
    version,
    signed: args.get("--signed") === true,
    startProcess: () => startSleeper(app),
  });
  const server = fakeDriver(fake, (path, body) => {
    if (path === "/fake/outage") fake.outage(body as { from: number; to: number });
  });
  await new Promise<void>((resolveListen) => server.listen(port, "127.0.0.1", () => resolveListen()));
  await new Promise<void>((resolveStop) => {
    const stop = () => {
      if (fake.running) fake.stop();
      server.close(() => resolveStop());
      server.closeAllConnections();
    };
    process.once("SIGTERM", stop);
    process.once("SIGINT", stop);
  });
  return 0;
}

async function main(argv: string[]): Promise<number> {
  if (argv[0] === "fake-driver") return fakeDriverMode(parseArguments(argv.slice(1)));
  const args = parseArguments(argv);
  quiet = args.get("--quiet") === true;
  return args.get("--dry-run") ? dryRun(args) : journey(args);
}

try {
  process.exitCode = await main(process.argv.slice(2));
} catch (error) {
  if (held.length > 0) console.error(held.join("\n"));
  console.error(error instanceof UsageError ? error.message : `the journey could not run: ${errorText(error)}`);
  process.exitCode = 2;
}
