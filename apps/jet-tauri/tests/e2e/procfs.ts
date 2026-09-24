/**
 * CPU time and proportional memory of process trees, read from `/proc`
 * (proc(5)) for the desktop release journey:
 *
 * - `/proc/<pid>/stat`: `ppid`, `utime`, `stime` and `starttime`, in clock
 *   ticks (`getconf CLK_TCK`, 100 on Linux).
 * - `/proc/<pid>/smaps_rollup`: `Pss` in KiB, which splits shared pages
 *   between the processes mapping them, so a tree's sum does not count the
 *   WebKit libraries once per process.
 * - `/proc/uptime`: seconds since boot, to turn `starttime` into a
 *   wall-clock launch time.
 *
 * The app tree is `jet-tauri` and everything below it (WebKitWebProcess,
 * WebKitNetworkProcess and any sandbox helpers); the daemon tree is the
 * service's `jetd` and its children. A process whose memory the kernel
 * does not let this user read (a non-dumpable sandboxed process) counts for
 * CPU and is listed as unreadable for PSS.
 */
import { readdirSync, readFileSync, readlinkSync } from "node:fs";
import { join } from "node:path";

export type ProcStat = {
  pid: number;
  comm: string;
  state: string;
  ppid: number;
  /** User plus system CPU time, in clock ticks. */
  ticks: number;
  /** Start time after boot, in clock ticks. */
  starttime: number;
};

/**
 * Parses one `stat` line. `comm` sits in parentheses and may itself hold
 * spaces and parentheses, so the fields after it start at the last `)`.
 */
export function parseStat(text: string): ProcStat {
  const open = text.indexOf("(");
  const close = text.lastIndexOf(")");
  if (open < 0 || close < open) throw new Error("stat: no command name");
  const pid = Number(text.slice(0, open).trim());
  const fields = text.slice(close + 1).trim().split(/\s+/);
  // fields[0] is field 3 (state); utime is field 14, stime 15, starttime 22.
  const number = (field: number) => {
    const raw = fields[field - 3] ?? "";
    const value = Number(raw);
    if (!/^\d+$/.test(raw) || !Number.isSafeInteger(value)) throw new Error(`stat: field ${field} is not a count`);
    return value;
  };
  if (!/^[1-9]\d*$/.test(text.slice(0, open).trim())) throw new Error("stat: no pid");
  return {
    pid,
    comm: text.slice(open + 1, close),
    state: fields[0] ?? "?",
    ppid: number(4),
    ticks: number(14) + number(15),
    starttime: number(22),
  };
}

/** `Pss` and `Rss` from `smaps_rollup`, in KiB; null when absent. */
export function parseSmapsRollup(text: string): { pssKiB: number; rssKiB: number } | null {
  const field = (name: string) => {
    const match = new RegExp(`^${name}:\\s+(\\d+) kB$`, "m").exec(text);
    return match ? Number(match[1]) : null;
  };
  const pss = field("Pss");
  const rss = field("Rss");
  return pss === null || rss === null ? null : { pssKiB: pss, rssKiB: rss };
}

export function parseUptime(text: string): number {
  const first = text.trim().split(/\s+/)[0] ?? "";
  if (!/^\d+(\.\d+)?$/.test(first)) throw new Error("uptime: not a number of seconds");
  return Number(first);
}

/**
 * Wall-clock launch time of a process: now, less how long it has run
 * (uptime minus its start after boot). Resolution is one clock tick.
 */
export function processStartEpochMs(stat: ProcStat, uptimeSeconds: number, nowEpochMs: number, clockTicks: number): number {
  const ageSeconds = uptimeSeconds - stat.starttime / clockTicks;
  return nowEpochMs - ageSeconds * 1000;
}

/** Reads `/proc`, or a directory laid out like it in tests. */
export class ProcFs {
  constructor(readonly root = "/proc") {}

  pids(): number[] {
    return readdirSync(this.root)
      .filter((name) => /^[1-9]\d*$/.test(name))
      .map(Number)
      .sort((a, b) => a - b);
  }

  /** null once the process is gone. */
  stat(pid: number): ProcStat | null {
    try {
      return parseStat(readFileSync(join(this.root, String(pid), "stat"), "utf8"));
    } catch {
      return null;
    }
  }

  /** Every process that can be read now. */
  stats(): ProcStat[] {
    const stats: ProcStat[] = [];
    for (const pid of this.pids()) {
      const stat = this.stat(pid);
      if (stat) stats.push(stat);
    }
    return stats;
  }

  /** null when the process is gone or its memory is not readable by this user. */
  memory(pid: number): { pssKiB: number; rssKiB: number } | null {
    try {
      return parseSmapsRollup(readFileSync(join(this.root, String(pid), "smaps_rollup"), "utf8"));
    } catch {
      return null;
    }
  }

  uptime(): number {
    return parseUptime(readFileSync(join(this.root, "uptime"), "utf8"));
  }

  /** The executable path, when this user may read it. */
  exe(pid: number): string | null {
    try {
      return readlinkSync(join(this.root, String(pid), "exe"));
    } catch {
      return null;
    }
  }
}

/** `roots` and every process below them. */
export function descendants(stats: readonly ProcStat[], roots: readonly number[]): Set<number> {
  const children = new Map<number, number[]>();
  for (const stat of stats) {
    const list = children.get(stat.ppid) ?? [];
    list.push(stat.pid);
    children.set(stat.ppid, list);
  }
  const alive = new Set(stats.map((stat) => stat.pid));
  const tree = new Set<number>();
  const queue = roots.filter((pid) => alive.has(pid));
  while (queue.length > 0) {
    const pid = queue.shift()!;
    if (tree.has(pid)) continue;
    tree.add(pid);
    queue.push(...(children.get(pid) ?? []));
  }
  return tree;
}

/** The newest process named `comm` below `ancestor`, or anywhere without one. */
export function findProcess(stats: readonly ProcStat[], comm: string, ancestor: number | null): ProcStat | null {
  const scope = ancestor === null ? null : descendants(stats, [ancestor]);
  const matches = stats.filter((stat) => stat.comm === comm && (scope === null || scope.has(stat.pid)));
  matches.sort((a, b) => b.starttime - a.starttime);
  return matches[0] ?? null;
}

export type ProcessSample = {
  pid: number;
  comm: string;
  starttime: number;
  ticks: number;
  /** null when unreadable. */
  pssKiB: number | null;
  rssKiB: number | null;
};

export type TreeSample = { atMs: number; processes: ProcessSample[] };

export function sampleTree(proc: ProcFs, roots: readonly number[], atMs: number): TreeSample {
  const stats = proc.stats();
  const tree = descendants(stats, roots);
  const processes: ProcessSample[] = [];
  for (const stat of stats) {
    if (!tree.has(stat.pid)) continue;
    const memory = proc.memory(stat.pid);
    processes.push({
      pid: stat.pid,
      comm: stat.comm,
      starttime: stat.starttime,
      ticks: stat.ticks,
      pssKiB: memory?.pssKiB ?? null,
      rssKiB: memory?.rssKiB ?? null,
    });
  }
  return { atMs, processes };
}

export type ProcessSummary = {
  pid: number;
  comm: string;
  cpuSeconds: number;
  pssMaxKiB: number | null;
};

export type TreeSummary = {
  durationMs: number;
  samples: number;
  /** Percent of one CPU over the window. */
  cpuPercent: number;
  cpuSeconds: number;
  /** Sum over readable processes, per sample. */
  pss: { meanKiB: number; maxKiB: number; lastKiB: number } | null;
  rssMaxKiB: number | null;
  /** Processes whose memory could not be read (names). */
  unreadable: string[];
  /** The tree's root was gone before the window ended. */
  rootExited: boolean;
  processes: ProcessSummary[];
};

/**
 * CPU and memory of one tree over a series of samples. A process is keyed
 * by pid and start time, so a reused pid is a new process. CPU counts
 * ticks from the first sample for processes already running, and all of
 * them for processes started inside the window; a process that exits
 * keeps what its last sample saw.
 */
export function summarize(samples: readonly TreeSample[], clockTicks: number, root: number | null = null): TreeSummary {
  if (samples.length < 2) throw new Error("summarize needs at least two samples");
  const first = samples[0];
  const last = samples[samples.length - 1];
  const durationMs = last.atMs - first.atMs;
  type Seen = { pid: number; comm: string; baseline: number; latest: number; pssMax: number | null };
  const seen = new Map<string, Seen>();
  const key = (process: ProcessSample) => `${process.pid}@${process.starttime}`;
  for (const process of first.processes) {
    seen.set(key(process), { pid: process.pid, comm: process.comm, baseline: process.ticks, latest: process.ticks, pssMax: process.pssKiB });
  }
  const unreadable = new Set<string>();
  const totals: number[] = [];
  let rssMax: number | null = null;
  for (const sample of samples) {
    let pss = 0;
    let readable = 0;
    let rss = 0;
    for (const process of sample.processes) {
      const entry = seen.get(key(process));
      if (entry) {
        entry.latest = process.ticks;
        if (process.pssKiB !== null) entry.pssMax = Math.max(entry.pssMax ?? 0, process.pssKiB);
      } else {
        seen.set(key(process), { pid: process.pid, comm: process.comm, baseline: 0, latest: process.ticks, pssMax: process.pssKiB });
      }
      if (process.pssKiB === null) {
        unreadable.add(process.comm);
      } else {
        pss += process.pssKiB;
        readable += 1;
      }
      rss += process.rssKiB ?? 0;
    }
    if (readable > 0) totals.push(pss);
    rssMax = Math.max(rssMax ?? 0, rss);
  }
  const processes = [...seen.values()].map((entry) => ({
    pid: entry.pid,
    comm: entry.comm,
    cpuSeconds: round((entry.latest - entry.baseline) / clockTicks, 2),
    pssMaxKiB: entry.pssMax,
  }));
  const ticks = [...seen.values()].reduce((sum, entry) => sum + Math.max(0, entry.latest - entry.baseline), 0);
  const cpuSeconds = ticks / clockTicks;
  return {
    durationMs,
    samples: samples.length,
    cpuPercent: durationMs > 0 ? round((cpuSeconds / (durationMs / 1000)) * 100, 3) : 0,
    cpuSeconds: round(cpuSeconds, 2),
    pss:
      totals.length === 0
        ? null
        : {
            meanKiB: Math.round(totals.reduce((sum, value) => sum + value, 0) / totals.length),
            maxKiB: Math.max(...totals),
            lastKiB: totals[totals.length - 1],
          },
    rssMaxKiB: rssMax,
    unreadable: [...unreadable].sort(),
    rootExited: root !== null && !last.processes.some((process) => process.pid === root),
    processes,
  };
}

function round(value: number, digits: number): number {
  const scale = 10 ** digits;
  return Math.round(value * scale) / scale;
}
