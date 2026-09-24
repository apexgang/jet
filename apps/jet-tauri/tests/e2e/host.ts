/**
 * The machine the desktop release journey runs on: commands with fixed
 * argv and a deadline, the daemon socket, and signals. The CI runner uses
 * `SystemHost`; the dry run and the tests use fakes, so nothing touches a
 * real Jet home, systemd or brew outside CI.
 */
import { spawn } from "node:child_process";
import { createConnection } from "node:net";

export type RunResult = {
  /** Exit code; null when killed or not started. */
  code: number | null;
  stdout: string;
  stderr: string;
  /** Why it did not run to completion: not found, timed out, … */
  error: string | null;
};

export type RunOptions = { timeoutMs?: number };

export interface Host {
  /** The home directory the app provisions under (`~/.jet`, `~/.config`). */
  readonly home: string;
  /** `$XDG_CONFIG_HOME` or `~/.config`. */
  readonly configHome: string;
  run(argv: readonly string[], options?: RunOptions): Promise<RunResult>;
  /** Whether a connection to the Unix socket at `path` is accepted. */
  socketAccepts(path: string, timeoutMs: number): Promise<boolean>;
  /** Signals a process; false when it is gone. */
  kill(pid: number, signal: "SIGTERM" | "SIGKILL"): boolean;
}

/** Output beyond this is cut; diagnostics only need the tail of a journal. */
const OUTPUT_LIMIT = 4 * 1024 * 1024;

export class SystemHost implements Host {
  readonly configHome: string;

  constructor(
    readonly home: string,
    configHome: string | undefined,
  ) {
    this.configHome = configHome && configHome.startsWith("/") ? configHome : `${home}/.config`;
  }

  run(argv: readonly string[], options: RunOptions = {}): Promise<RunResult> {
    const timeoutMs = options.timeoutMs ?? 30_000;
    return new Promise((resolve) => {
      let stdout = "";
      let stderr = "";
      let error: string | null = null;
      let child: ReturnType<typeof spawnPiped>;
      try {
        child = spawnPiped(argv);
      } catch (cause) {
        resolve({ code: null, stdout, stderr, error: (cause as Error).message });
        return;
      }
      const timer = setTimeout(() => {
        error = `timed out after ${timeoutMs} ms`;
        child.kill("SIGKILL");
      }, timeoutMs);
      child.stdout.on("data", (chunk: Buffer) => {
        if (stdout.length < OUTPUT_LIMIT) stdout += chunk.toString("utf8");
      });
      child.stderr.on("data", (chunk: Buffer) => {
        if (stderr.length < OUTPUT_LIMIT) stderr += chunk.toString("utf8");
      });
      child.on("error", (cause) => {
        error ??= cause.message;
      });
      child.on("close", (code) => {
        clearTimeout(timer);
        resolve({ code: error === null ? code : null, stdout, stderr, error });
      });
    });
  }

  socketAccepts(path: string, timeoutMs: number): Promise<boolean> {
    return socketAccepts(path, timeoutMs);
  }

  kill(pid: number, signal: "SIGTERM" | "SIGKILL"): boolean {
    try {
      process.kill(pid, signal);
      return true;
    } catch {
      return false;
    }
  }
}

function spawnPiped(argv: readonly string[]) {
  return spawn(argv[0], argv.slice(1), { stdio: ["ignore", "pipe", "pipe"] });
}

/** Connects and closes at once; any error or the deadline means no. */
export function socketAccepts(path: string, timeoutMs: number): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = createConnection({ path });
    const done = (accepted: boolean) => {
      clearTimeout(timer);
      socket.destroy();
      resolve(accepted);
    };
    const timer = setTimeout(() => done(false), timeoutMs);
    socket.once("connect", () => done(true));
    socket.once("error", () => done(false));
  });
}

/** The Jet home, service and core paths the journey checks. */
export function servicePaths(host: Pick<Host, "home" | "configHome">) {
  const jet = `${host.home}/.jet`;
  return {
    jet,
    current: `${jet}/core/current`,
    currentJetd: `${jet}/core/current/jetd`,
    socket: `${jet}/runtime/jetd.sock`,
    unit: `${host.configHome}/systemd/user/jetd.service`,
  };
}

export const UNIT = "jetd.service";

/** `jetd core status` (docs/core-distribution.md), as far as the journey reads it. */
export type CoreStatus = {
  current: string | null;
  previous: string | null;
  owner:
    | { state: "free" }
    | { state: "held"; daemon: { pid: number; version: string; channel: string } | null };
};

export function parseCoreStatus(stdout: string): CoreStatus {
  const value = JSON.parse(stdout.trim()) as Record<string, unknown>;
  const text = (field: unknown) => (typeof field === "string" ? field : null);
  const owner = value.owner as Record<string, unknown> | undefined;
  if (!owner || (owner.state !== "free" && owner.state !== "held")) throw new Error("no owner in jetd core status");
  let parsedOwner: CoreStatus["owner"] = { state: "free" };
  if (owner.state === "held") {
    const daemon = owner.daemon as Record<string, unknown> | null | undefined;
    parsedOwner = {
      state: "held",
      daemon:
        daemon && typeof daemon.pid === "number"
          ? { pid: daemon.pid, version: text(daemon.version) ?? "", channel: text(daemon.channel) ?? "" }
          : null,
    };
  }
  return { current: text(value.current), previous: text(value.previous), owner: parsedOwner };
}

/** The service's main process, or null when it has none. */
export async function mainPid(host: Host): Promise<number | null> {
  const result = await host.run(["systemctl", "--user", "show", UNIT, "--property=MainPID", "--value"]);
  const pid = Number(result.stdout.trim());
  return result.code === 0 && Number.isSafeInteger(pid) && pid > 0 ? pid : null;
}

export async function systemctlWord(host: Host, verb: "is-enabled" | "is-active"): Promise<string> {
  const result = await host.run(["systemctl", "--user", verb, UNIT]);
  return result.stdout.trim() || result.error || `exit ${result.code}`;
}

/** Best-effort diagnostics for a failed journey; each command's output is kept as is. */
export async function diagnostics(host: Host): Promise<string> {
  const paths = servicePaths(host);
  const commands: string[][] = [
    ["systemctl", "--user", "status", UNIT, "--no-pager", "--full"],
    ["systemctl", "--user", "show", UNIT, "--property=ActiveState,SubState,MainPID,NRestarts,ExecMainStatus,ConditionResult"],
    // User units log to the system journal on a runner with volatile storage.
    ["sudo", "-n", "journalctl", "--no-pager", "--output=short-precise", `_SYSTEMD_USER_UNIT=${UNIT}`, "-n", "300"],
    ["journalctl", "--user", "--no-pager", "--output=short-precise", "-u", UNIT, "-n", "300"],
    ["ls", "-la", `${paths.jet}`, `${paths.jet}/core`, `${paths.jet}/runtime`],
    [paths.currentJetd, "core", "status"],
    ["cat", paths.unit],
  ];
  const sections: string[] = [];
  for (const argv of commands) {
    const result = await host.run(argv, { timeoutMs: 20_000 });
    sections.push(
      `$ ${argv.join(" ")}\n${result.stdout}${result.stderr}${result.error ? `(${result.error})\n` : result.code === 0 ? "" : `(exit ${result.code})\n`}`,
    );
  }
  return sections.join("\n");
}
