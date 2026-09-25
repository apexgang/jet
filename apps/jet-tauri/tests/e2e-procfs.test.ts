import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  ProcFs,
  descendants,
  findProcess,
  parseSmapsRollup,
  parseStat,
  parseUptime,
  processStartEpochMs,
  sampleTree,
  summarize,
  type ProcStat,
  type TreeSample,
} from "./e2e/procfs";

/** A stat line with `ticks` split into utime and stime. */
function statLine(pid: number, comm: string, ppid: number, utime: number, stime: number, starttime: number): string {
  const fields = ["S", ppid, pid, pid, 0, -1, 4194560, 0, 0, 0, 0, utime, stime, 0, 0, 20, 0, 1, 0, starttime, 1000, 100];
  return `${pid} (${comm}) ${fields.join(" ")} 0 0 0\n`;
}

const SMAPS = `55d0-7ffd ---p 00000000 00:00 0 [rollup]
Rss:                2284 kB
Pss:                 182 kB
Pss_Dirty:           108 kB
Shared_Clean:       2116 kB
`;

let root: string | null = null;

function fakeProc(): string {
  root = mkdtempSync(join(tmpdir(), "jet-e2e-proc-"));
  return root;
}

function add(dir: string, pid: number, comm: string, ppid: number, ticks: [number, number] = [0, 0], starttime = 100, pss: number | null = 100) {
  mkdirSync(join(dir, String(pid)), { recursive: true });
  writeFileSync(join(dir, String(pid), "stat"), statLine(pid, comm, ppid, ticks[0], ticks[1], starttime));
  if (pss !== null) writeFileSync(join(dir, String(pid), "smaps_rollup"), SMAPS.replace("182", String(pss)));
}

afterEach(() => {
  if (root) rmSync(root, { recursive: true, force: true });
  root = null;
});

describe("proc(5) parsing", () => {
  it("reads pid, parent, CPU ticks and start time from stat", () => {
    expect(parseStat(statLine(4321, "jet-tauri", 4000, 150, 50, 98_765))).toEqual({
      pid: 4321,
      comm: "jet-tauri",
      state: "S",
      ppid: 4000,
      ticks: 200,
      starttime: 98_765,
    });
  });

  it("keeps spaces and parentheses in the command name", () => {
    const stat = parseStat(statLine(9, "Web Content (x) )", 1, 3, 4, 10));
    expect(stat.comm).toBe("Web Content (x) )");
    expect(stat.ticks).toBe(7);
    expect(stat.ppid).toBe(1);
  });

  it("refuses a stat line it cannot read", () => {
    expect(() => parseStat("garbage")).toThrow(/no command name/);
    expect(() => parseStat("12 (x) S notanumber")).toThrow(/field 4/);
  });

  it("reads Pss and Rss from smaps_rollup", () => {
    expect(parseSmapsRollup(SMAPS)).toEqual({ pssKiB: 182, rssKiB: 2284 });
    expect(parseSmapsRollup("Rss: 1 kB\n")).toBeNull();
    expect(parseUptime("12345.67 99999.00\n")).toBe(12345.67);
    expect(() => parseUptime("")).toThrow();
  });

  it("turns the start time after boot into a wall-clock launch time", () => {
    const stat = { starttime: 1_000 } as ProcStat;
    // Up 30.5 s, started 10 s after boot: running for 20.5 s.
    expect(processStartEpochMs(stat, 30.5, 1_000_000, 100)).toBe(979_500);
  });
});

describe("process trees", () => {
  const stats = (entries: Array<[number, string, number, number?]>): ProcStat[] =>
    entries.map(([pid, comm, ppid, starttime = 1]) => ({ pid, comm, ppid, state: "S", ticks: 0, starttime }));

  it("walks every process below the roots", () => {
    const table = stats([
      [1, "systemd", 0],
      [10, "tauri-driver", 1],
      [11, "WebKitWebDriver", 10],
      [12, "jet-tauri", 11],
      [13, "WebKitWebProces", 12],
      [14, "bwrap", 12],
      [15, "WebKitNetworkPr", 14],
      [20, "jetd", 1],
    ]);
    expect([...descendants(table, [12])].sort()).toEqual([12, 13, 14, 15]);
    expect([...descendants(table, [20])]).toEqual([20]);
    expect([...descendants(table, [99])]).toEqual([]);
  });

  it("finds the newest process by name, below an ancestor when given", () => {
    const table = stats([
      [1, "systemd", 0],
      [10, "tauri-driver", 1],
      [12, "jet-tauri", 10, 50],
      [13, "jet-tauri", 10, 70],
      [30, "jet-tauri", 1, 90],
    ]);
    expect(findProcess(table, "jet-tauri", 10)?.pid).toBe(13);
    expect(findProcess(table, "jet-tauri", null)?.pid).toBe(30);
    expect(findProcess(table, "jetd", null)).toBeNull();
  });

  it("samples a tree from a /proc directory, with unreadable memory as null", () => {
    const dir = fakeProc();
    writeFileSync(join(dir, "uptime"), "100.00 50.00\n");
    add(dir, 1, "systemd", 0);
    add(dir, 12, "jet-tauri", 1, [10, 5], 100, 30_000);
    add(dir, 13, "WebKitWebProces", 12, [40, 10], 110, null);
    mkdirSync(join(dir, "self"));
    const proc = new ProcFs(dir);
    expect(proc.pids().sort()).toEqual([1, 12, 13]);
    expect(proc.uptime()).toBe(100);
    expect(proc.stat(99)).toBeNull();
    const sample = sampleTree(proc, [12], 5);
    expect(sample.atMs).toBe(5);
    expect(sample.processes).toEqual([
      { pid: 12, comm: "jet-tauri", starttime: 100, ticks: 15, pssKiB: 30_000, rssKiB: 2284 },
      { pid: 13, comm: "WebKitWebProces", starttime: 110, ticks: 50, pssKiB: null, rssKiB: null },
    ]);
  });

  it("treats memory the kernel will not show as unreadable", () => {
    if (process.getuid?.() === 0) return; // root reads everything
    const dir = fakeProc();
    add(dir, 12, "jet-tauri", 1, [0, 0], 100, 500);
    chmodSync(join(dir, "12", "smaps_rollup"), 0o000);
    expect(new ProcFs(dir).memory(12)).toBeNull();
  });
});

describe("summaries over an idle window", () => {
  const process = (pid: number, comm: string, ticks: number, pssKiB: number | null, starttime = 1) => ({
    pid,
    comm,
    starttime,
    ticks,
    pssKiB,
    rssKiB: pssKiB === null ? null : pssKiB * 2,
  });

  it("counts CPU from the first sample and averages PSS", () => {
    const samples: TreeSample[] = [
      { atMs: 0, processes: [process(12, "jet-tauri", 1_000, 40_000), process(13, "WebKitWebProces", 500, 60_000)] },
      { atMs: 30_000, processes: [process(12, "jet-tauri", 1_010, 40_000), process(13, "WebKitWebProces", 520, 62_000)] },
      { atMs: 60_000, processes: [process(12, "jet-tauri", 1_020, 40_000), process(13, "WebKitWebProces", 530, 64_000)] },
    ];
    const summary = summarize(samples, 100, 12);
    // 20 + 30 ticks = 0.5 CPU-seconds over 60 s.
    expect(summary.cpuSeconds).toBe(0.5);
    expect(summary.cpuPercent).toBeCloseTo(0.833, 3);
    expect(summary.pss).toEqual({ meanKiB: 102_000, maxKiB: 104_000, lastKiB: 104_000 });
    expect(summary.rssMaxKiB).toBe(208_000);
    expect(summary.rootExited).toBe(false);
    expect(summary.unreadable).toEqual([]);
    expect(summary.processes).toEqual([
      { pid: 12, comm: "jet-tauri", cpuSeconds: 0.2, pssMaxKiB: 40_000 },
      { pid: 13, comm: "WebKitWebProces", cpuSeconds: 0.3, pssMaxKiB: 64_000 },
    ]);
  });

  it("counts a process started inside the window in full and keeps an exited one's last ticks", () => {
    const samples: TreeSample[] = [
      { atMs: 0, processes: [process(12, "jet-tauri", 100, 10), process(14, "helper", 50, 5)] },
      { atMs: 1_000, processes: [process(12, "jet-tauri", 100, 10), process(14, "helper", 60, 5), process(15, "late", 7, 1, 900)] },
      { atMs: 2_000, processes: [process(12, "jet-tauri", 100, 10), process(15, "late", 9, 1, 900)] },
    ];
    const summary = summarize(samples, 100);
    // helper: 10 ticks before it exited; late: all 9 of its ticks.
    expect(summary.cpuSeconds).toBe(0.19);
    expect(summary.pss?.lastKiB).toBe(11);
  });

  it("treats a reused pid as a new process", () => {
    const samples: TreeSample[] = [
      { atMs: 0, processes: [process(12, "jet-tauri", 1_000, 10, 1)] },
      { atMs: 1_000, processes: [process(12, "jet-tauri", 3, 10, 500)] },
    ];
    expect(summarize(samples, 100).cpuSeconds).toBe(0.03);
  });

  it("lists unreadable processes and notices a root that exited", () => {
    const samples: TreeSample[] = [
      { atMs: 0, processes: [process(20, "jetd", 0, 9_000), process(21, "bwrap", 0, null)] },
      { atMs: 1_000, processes: [process(21, "bwrap", 0, null)] },
    ];
    const summary = summarize(samples, 100, 20);
    expect(summary.rootExited).toBe(true);
    expect(summary.unreadable).toEqual(["bwrap"]);
    expect(summary.pss).toEqual({ meanKiB: 9_000, maxKiB: 9_000, lastKiB: 9_000 });
  });

  it("needs two samples", () => {
    expect(() => summarize([{ atMs: 0, processes: [] }], 100)).toThrow(/two samples/);
  });
});
