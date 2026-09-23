/**
 * The adaptation audit (wave 3.4 §8). Local only, never CI: it needs
 * `agent-browser` (Chromium) on the PATH. Run it from `apps/jet-tauri` with
 * `just adaptation-audit`, or directly:
 *
 *   bun scripts/adaptation/audit.ts [--port 1420] [--scenes a,b] [--configurations id,...]
 *     [--output dir] [--no-screenshots]
 *
 * It starts `bun run dev`, then opens every scene in every configuration
 * against mocked IPC and checks: axe (no serious or critical violation),
 * no horizontal overflow, the conversation keeps 420 px when it is not
 * covered, the compact work panel is closed after load and Send is not
 * covered, no running animation under reduced motion, no backdrop filter, and
 * a 40-step Tab walk where every focused element shows a focus indicator and
 * is not obscured.
 *
 * Viewport and media features are emulated over the Chrome DevTools
 * Protocol connection `agent-browser get cdp-url` names, held open for the
 * whole run so the emulation stays in force. Every result is checked against
 * `matchMedia` and the viewport, so an emulation that did not apply fails the
 * run instead of passing it.
 *
 * Screenshots and the JSON report go to `$TMPDIR/jet-adaptation/<ISO>/`
 * (or `--output`), never inside the repository. Exit status: 0 all passed, 1 a check failed,
 * 2 the audit could not run.
 */

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { createConnection } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { CONVERSATION_MINIMUM } from "../../src/lib/features/shell/layout";
import {
  CONFIGURATIONS,
  SCENES,
  isCompact,
  missingCoverage,
  type Configuration,
  type Scene,
} from "./configurations";

const SESSION = "jet-adaptation";
const TAB_STEPS = 40;
const APP_DIRECTORY = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const REPOSITORY = resolve(APP_DIRECTORY, "../..");

type Options = {
  port: number;
  scenes: Scene[];
  configurations: Configuration[];
  screenshots: boolean;
  /** Where run folders go; outside the repository. */
  output: string;
};

type Finding = { check: string; detail: string };

type RunResult = {
  scene: string;
  configuration: string;
  failures: Finding[];
  warnings: Finding[];
  unmocked: string[];
  /** Distinct elements the Tab walk reached. */
  focusable: number;
  screenshot: string | null;
};

class AuditError extends Error {}

function parseOptions(argv: string[]): Options {
  const value = (flag: string): string | null => {
    const index = argv.indexOf(flag);
    if (index === -1) return null;
    const next = argv[index + 1];
    if (next === undefined || next.startsWith("--")) throw new AuditError(`${flag} needs a value.`);
    return next;
  };
  const list = (flag: string): string[] | null => value(flag)?.split(",").map((item) => item.trim()).filter(Boolean) ?? null;
  const port = Number(value("--port") ?? "1420");
  if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new AuditError("--port must be 1024-65535.");
  const sceneNames = list("--scenes");
  const configurationIds = list("--configurations");
  const scenes = sceneNames ? SCENES.filter((scene) => sceneNames.includes(scene.name)) : [...SCENES];
  const configurations = configurationIds
    ? CONFIGURATIONS.filter((configuration) => configurationIds.includes(configuration.id))
    : [...CONFIGURATIONS];
  const unknown = [
    ...(sceneNames ?? []).filter((name) => !SCENES.some((scene) => scene.name === name)),
    ...(configurationIds ?? []).filter((id) => !CONFIGURATIONS.some((configuration) => configuration.id === id)),
  ];
  if (unknown.length > 0) throw new AuditError(`Unknown scene or configuration: ${unknown.join(", ")}.`);
  const output = resolve(value("--output") ?? join(process.env.TMPDIR ?? tmpdir(), "jet-adaptation"));
  if (!relative(REPOSITORY, output).startsWith("..")) {
    throw new AuditError("--output must be outside the repository.");
  }
  return { port, scenes, configurations, screenshots: !argv.includes("--no-screenshots"), output };
}

/** Runs `agent-browser` in the audit session and returns its standard output. */
function browser(args: string[], initScripts: string[] = []): string {
  const result = spawnSync(
    "agent-browser",
    ["--session", SESSION, ...initScripts.flatMap((script) => ["--init-script", script]), ...args],
    { encoding: "utf8", timeout: 60_000 },
  );
  if (result.error) throw new AuditError(`agent-browser ${args[0]} failed: ${result.error.message}`);
  if (result.status !== 0) {
    throw new AuditError(`agent-browser ${args[0]} exited ${result.status}: ${(result.stderr || result.stdout).trim()}`);
  }
  return result.stdout;
}

function portInUse(port: number): Promise<boolean> {
  return new Promise((done) => {
    const socket = createConnection({ port, host: "127.0.0.1" });
    socket.once("connect", () => {
      socket.destroy();
      done(true);
    });
    socket.once("error", () => done(false));
  });
}

async function startDevServer(port: number): Promise<ChildProcess> {
  // A server already on the port may belong to another checkout; auditing it
  // would report on someone else's tree.
  if (await portInUse(port)) {
    throw new AuditError(`Port ${port} is in use. Stop that server or pass --port.`);
  }
  const server = spawn("bun", ["run", "dev", "--", "--port", String(port), "--strictPort"], {
    cwd: APP_DIRECTORY,
    stdio: ["ignore", "ignore", "pipe"],
    detached: true,
  });
  // Shown only when the server fails to start; its exit on SIGTERM is expected.
  let errors = "";
  server.stderr?.on("data", (chunk: Buffer) => {
    errors = (errors + chunk.toString()).slice(-4_000);
  });
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if (server.exitCode !== null) throw new AuditError(`bun run dev exited ${server.exitCode}: ${errors.trim()}`);
    try {
      const response = await fetch(`http://localhost:${port}/`);
      if (response.status === 200) return server;
    } catch {
      // Not listening yet.
    }
    await delay(250);
  }
  stopDevServer(server);
  throw new AuditError("bun run dev did not answer within 60 seconds.");
}

function stopDevServer(server: ChildProcess): void {
  if (server.pid === undefined || server.exitCode !== null) return;
  try {
    process.kill(-server.pid, "SIGTERM");
  } catch {
    server.kill("SIGTERM");
  }
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((done) => setTimeout(done, milliseconds));
}

/** A minimal DevTools Protocol client over the page's own WebSocket. */
class DevTools {
  private next = 0;
  private readonly pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>();

  private constructor(private readonly socket: WebSocket) {
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data)) as { id?: number; result?: unknown; error?: { message: string } };
      if (message.id === undefined) return;
      const waiter = this.pending.get(message.id);
      if (!waiter) return;
      this.pending.delete(message.id);
      if (message.error) waiter.reject(new AuditError(`DevTools: ${message.error.message}`));
      else waiter.resolve(message.result);
    });
  }

  static async attach(browserUrl: string, pageUrlPrefix: string): Promise<DevTools> {
    const origin = new URL(browserUrl);
    const targets = (await (await fetch(`http://${origin.host}/json/list`)).json()) as Array<{
      type: string;
      url: string;
      webSocketDebuggerUrl: string;
    }>;
    const page = targets.find((target) => target.type === "page" && target.url.startsWith(pageUrlPrefix));
    if (!page) throw new AuditError("The audit page is not among the browser's targets.");
    const socket = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise<void>((done, fail) => {
      socket.addEventListener("open", () => done(), { once: true });
      socket.addEventListener("error", () => fail(new AuditError("Could not open the DevTools connection.")), {
        once: true,
      });
    });
    return new DevTools(socket);
  }

  send<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const id = ++this.next;
    return new Promise<T>((resolveCall, rejectCall) => {
      this.pending.set(id, { resolve: (value) => resolveCall(value as T), reject: rejectCall });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  async evaluate<T>(expression: string): Promise<T> {
    const reply = await this.send<{ result: { value: T }; exceptionDetails?: { text: string } }>("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (reply.exceptionDetails) throw new AuditError(`Page script failed: ${reply.exceptionDetails.text}`);
    return reply.result.value;
  }

  async emulate(configuration: Configuration): Promise<void> {
    await this.send("Emulation.setDeviceMetricsOverride", {
      width: configuration.width,
      height: configuration.height,
      deviceScaleFactor: configuration.scale,
      mobile: false,
    });
    await this.send("Emulation.setEmulatedMedia", {
      media: "screen",
      features: [
        { name: "prefers-color-scheme", value: configuration.colorScheme },
        { name: "prefers-contrast", value: configuration.contrast },
        { name: "forced-colors", value: configuration.forcedColors },
        { name: "prefers-reduced-motion", value: configuration.reducedMotion },
        { name: "prefers-reduced-transparency", value: configuration.reducedTransparency },
      ],
    });
  }

  async pressTab(): Promise<void> {
    const key = { key: "Tab", code: "Tab", windowsVirtualKeyCode: 9, nativeVirtualKeyCode: 9 };
    await this.send("Input.dispatchKeyEvent", { type: "keyDown", ...key });
    await this.send("Input.dispatchKeyEvent", { type: "keyUp", ...key });
  }

  close(): void {
    this.socket.close();
  }
}

/** Page-side helpers, evaluated in the page. Kept as plain JavaScript text. */
const PAGE_HELPERS = String.raw`
(() => {
  const describe = (element) => {
    if (!element || element === document.body) return "body";
    const label = element.getAttribute("aria-label") || element.textContent || "";
    const id = element.id ? "#" + element.id : "";
    const classes = element.classList.length ? "." + [...element.classList].join(".") : "";
    return element.tagName.toLowerCase() + id + classes + " \"" + label.trim().replace(/\s+/g, " ").slice(0, 40) + "\"";
  };
  const visible = (element) => {
    const style = getComputedStyle(element);
    const box = element.getBoundingClientRect();
    return style.visibility !== "hidden" && style.display !== "none" && box.width > 0 && box.height > 0;
  };
  const covered = (element) => {
    const box = element.getBoundingClientRect();
    const x = Math.min(Math.max(box.left + box.width / 2, 0), innerWidth - 1);
    const y = Math.min(Math.max(box.top + box.height / 2, 0), innerHeight - 1);
    const hit = document.elementFromPoint(x, y);
    return hit !== null && hit !== element && !element.contains(hit) && !hit.contains(element) ? describe(hit) : null;
  };
  return { describe, visible, covered };
})()
`;

type PageState = {
  width: number;
  scale: number;
  media: Record<string, boolean>;
  ready: boolean;
  overflow: number;
  conversation: { width: number; covered: boolean } | null;
  overlayOpen: boolean;
  panelVisible: boolean;
  send: string | null | "absent";
  backdrop: string[];
  unmocked: string[];
};

function stateExpression(configuration: Configuration): string {
  return `(() => {
    const h = ${PAGE_HELPERS};
    const conversation = document.querySelector(".conversation");
    const overlayOpen = document.querySelector(".main-region[inert]") !== null;
    const panel = document.querySelector(".work-panel");
    const send = document.querySelector(".send-button");
    const backdrop = [...document.querySelectorAll("*")]
      .filter((element) => { const value = getComputedStyle(element).backdropFilter; return value && value !== "none"; })
      .map(h.describe);
    return {
      width: innerWidth,
      scale: devicePixelRatio,
      media: {
        scheme: matchMedia("(prefers-color-scheme: ${configuration.colorScheme})").matches,
        contrast: matchMedia("(prefers-contrast: ${configuration.contrast})").matches,
        forced: matchMedia("(forced-colors: ${configuration.forcedColors})").matches,
        motion: matchMedia("(prefers-reduced-motion: ${configuration.reducedMotion})").matches,
        transparency: matchMedia("(prefers-reduced-transparency: ${configuration.reducedTransparency})").matches,
      },
      ready: document.querySelector(".app-shell, .settings-app") !== null && document.querySelector('[aria-busy="true"]') === null,
      // What the window can scroll sideways; hidden columns clipped by the
      // shell don't count.
      overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      conversation: conversation && h.visible(conversation)
        ? { width: Math.round(conversation.getBoundingClientRect().width), covered: overlayOpen }
        : null,
      overlayOpen,
      panelVisible: panel !== null && h.visible(panel),
      send: send ? h.covered(send) : "absent",
      backdrop,
      unmocked: window.__JET_ADAPTATION_UNMOCKED__ ?? [],
    };
  })()`;
}

type FocusStep = { element: string; indicator: boolean; covered: string | null } | null;

/**
 * The focused element's indicator: its own outline or box shadow, or one its
 * parent or grandparent draws through `:focus-within` (the composer draws
 * the ring on its box). An ancestor counts only when its ring goes away
 * once focus leaves, so an ordinary card shadow never passes for one.
 */
const FOCUS_EXPRESSION = `(() => {
  const h = ${PAGE_HELPERS};
  const element = document.activeElement;
  if (!element || element === document.body || element === document.documentElement) return null;
  const ring = (node) => {
    const style = getComputedStyle(node);
    const outline = style.outlineStyle !== "none" && parseFloat(style.outlineWidth) > 0 ? style.outline : "";
    return outline + (style.boxShadow !== "none" ? " " + style.boxShadow : "");
  };
  let indicator = ring(element) !== "";
  if (!indicator) {
    const ancestors = [element.parentElement, element.parentElement?.parentElement].filter(
      (node) => node && node.matches(":focus-within"),
    );
    const focused = ancestors.map(ring);
    element.blur();
    const blurred = ancestors.map(ring);
    element.focus();
    indicator = focused.some((value, index) => value !== "" && value !== blurred[index]);
  }
  return { element: h.describe(element), indicator, covered: h.covered(element) };
})()`;

type AxeReport = {
  success: boolean;
  data: {
    violations: Array<{ id: string; impact: string | null; nodes: Array<{ target: unknown }> }>;
    incomplete: Array<{ id: string; impact: string | null; nodes: Array<{ target: unknown }> }>;
  };
};

function axe(configuration: Configuration): { failures: Finding[]; warnings: Finding[] } {
  const report = JSON.parse(browser(["a11y", "--json"])) as AxeReport;
  if (!report.success) throw new AuditError("agent-browser a11y did not report a result.");
  const serious = (impact: string | null) => impact === "serious" || impact === "critical";
  // Under forced colours the system palette decides contrast, not the app.
  const counted = (id: string) => !(configuration.forcedColors === "active" && id === "color-contrast");
  const finding = (rule: { id: string; impact: string | null; nodes: Array<{ target: unknown }> }): Finding => ({
    check: `axe ${rule.id}`,
    detail: `${rule.impact ?? "unknown"}: ${rule.nodes.map((node) => JSON.stringify(node.target)).join(", ")}`,
  });
  return {
    failures: report.data.violations.filter((rule) => serious(rule.impact) && counted(rule.id)).map(finding),
    warnings: [
      ...report.data.violations.filter((rule) => !serious(rule.impact) || !counted(rule.id)),
      ...report.data.incomplete.filter((rule) => serious(rule.impact) && counted(rule.id)),
    ].map(finding),
  };
}

async function waitUntilReady(devtools: DevTools, configuration: Configuration): Promise<PageState> {
  const deadline = Date.now() + 10_000;
  let state = await devtools.evaluate<PageState>(stateExpression(configuration));
  while (!state.ready && Date.now() < deadline) {
    await delay(100);
    state = await devtools.evaluate<PageState>(stateExpression(configuration));
  }
  // Let late IPC answers, the timeline feed and layout effects settle.
  await delay(400);
  return devtools.evaluate<PageState>(stateExpression(configuration));
}

async function auditOne(
  devtools: DevTools,
  baseUrl: string,
  scene: Scene,
  configuration: Configuration,
  output: string,
  screenshots: boolean,
): Promise<RunResult> {
  const failures: Finding[] = [];
  const fail = (check: string, detail: string) => failures.push({ check, detail });
  await devtools.emulate(configuration);
  browser(["open", `${baseUrl}${scene.route}?scene=${scene.name}`]);
  if (scene.sidebarButton !== null) {
    await waitUntilReady(devtools, configuration);
    const clicked = await devtools.evaluate<boolean>(`(() => {
      const button = [...document.querySelectorAll(".sidebar button")].find((candidate) => candidate.textContent.includes(${JSON.stringify(scene.sidebarButton)}));
      button?.click();
      return button !== undefined;
    })()`);
    if (!clicked) fail("scene", `No sidebar button "${scene.sidebarButton}".`);
  }
  let state = await waitUntilReady(devtools, configuration);

  if (!state.ready) fail("load", "The page did not finish loading in 10 seconds.");
  if (state.width !== configuration.width || state.scale !== configuration.scale) {
    fail("emulation", `Viewport is ${state.width}@${state.scale}, expected ${configuration.width}@${configuration.scale}.`);
  }
  for (const [feature, matches] of Object.entries(state.media)) {
    if (!matches) fail("emulation", `The ${feature} media feature is not emulated.`);
  }
  if (state.overflow > 0) fail("overflow", `The page scrolls sideways by ${state.overflow} px.`);
  if (state.conversation && !state.conversation.covered && state.conversation.width < CONVERSATION_MINIMUM) {
    fail("conversation width", `${state.conversation.width} px, below ${CONVERSATION_MINIMUM}.`);
  }
  if (scene.route === "/" && isCompact(configuration) && (state.overlayOpen || state.panelVisible)) {
    fail("compact panel", "The work panel is open after load in a compact window.");
  }
  if (scene.composer) {
    if (state.send === "absent") fail("send", "The Send button is missing.");
    else if (state.send !== null) fail("send", `The Send button is covered by ${state.send}.`);
  }
  if (state.backdrop.length > 0) fail("backdrop-filter", state.backdrop.join(", "));

  if (configuration.reducedMotion === "reduce") {
    await delay(1_000);
    const running = await devtools.evaluate<string[]>(
      `document.getAnimations().map((animation) => animation.animationName ?? animation.transitionProperty ?? animation.constructor.name)`,
    );
    if (running.length > 0) fail("reduced motion", `Still animating: ${running.join(", ")}.`);
  }

  const { failures: axeFailures, warnings } = axe(configuration);
  failures.push(...axeFailures);

  // The Tab walk runs last: moving focus can open nothing, but it scrolls.
  await devtools.evaluate(`(document.activeElement instanceof HTMLElement && document.activeElement.blur(), true)`);
  const seen = new Set<string>();
  for (let step = 0; step < TAB_STEPS; step += 1) {
    await devtools.pressTab();
    const focus = await devtools.evaluate<FocusStep>(FOCUS_EXPRESSION);
    if (focus === null) continue;
    const key = `${focus.element}`;
    if (seen.has(key)) continue;
    seen.add(key);
    if (!focus.indicator) fail("focus indicator", focus.element);
    if (focus.covered !== null) fail("focus obscured", `${focus.element} under ${focus.covered}`);
  }
  if (seen.size === 0) fail("focus", "Tab reached no element.");

  let screenshot: string | null = null;
  if (screenshots) {
    screenshot = join(output, `${scene.name}--${configuration.id}.png`);
    browser(["screenshot", screenshot]);
  }
  state = await devtools.evaluate<PageState>(stateExpression(configuration));
  return {
    scene: scene.name,
    configuration: configuration.id,
    failures,
    warnings,
    unmocked: [...new Set(state.unmocked)],
    focusable: seen.size,
    screenshot,
  };
}

async function main(): Promise<number> {
  const missing = missingCoverage();
  if (missing.length > 0) {
    console.error(`The configuration list no longer covers: ${missing.join("; ")}.`);
    return 2;
  }
  const options = parseOptions(process.argv.slice(2));
  const version = spawnSync("agent-browser", ["--version"], { encoding: "utf8" });
  if (version.status !== 0) throw new AuditError("agent-browser is not installed or not on the PATH.");

  const root = options.output;
  const output = join(root, new Date().toISOString().replace(/[:.]/g, "-"));
  mkdirSync(output, { recursive: true });
  const mock = join(root, "mock-ipc.js");
  const bundle = spawnSync(
    "bun",
    ["build", "--target", "browser", "--format", "iife", "scripts/adaptation/mock-ipc.ts", "--outfile", mock],
    { cwd: APP_DIRECTORY, encoding: "utf8" },
  );
  if (bundle.status !== 0) throw new AuditError(`Bundling the mocked IPC failed: ${bundle.stderr.trim()}`);

  const server = await startDevServer(options.port);
  const cleanup = () => stopDevServer(server);
  process.once("SIGINT", () => {
    cleanup();
    process.exit(130);
  });
  const baseUrl = `http://localhost:${options.port}`;
  const results: RunResult[] = [];
  let devtools: DevTools | null = null;
  try {
    // A fresh browser session, so the two init scripts are registered first.
    spawnSync("agent-browser", ["--session", SESSION, "close"], { encoding: "utf8" });
    const initScripts = [join(APP_DIRECTORY, "scripts/adaptation/scene-prelude.js"), mock];
    // The closed session's daemon may still be shutting down.
    for (let attempt = 1; ; attempt += 1) {
      try {
        browser(["open", `${baseUrl}/?scene=setup`], initScripts);
        break;
      } catch (error: unknown) {
        if (attempt === 3) throw error;
        await delay(1_000);
      }
    }
    devtools = await DevTools.attach(browser(["get", "cdp-url"]).trim(), baseUrl);
    const total = options.scenes.length * options.configurations.length;
    for (const configuration of options.configurations) {
      for (const scene of options.scenes) {
        const result = await auditOne(devtools, baseUrl, scene, configuration, output, options.screenshots);
        results.push(result);
        const status = result.failures.length === 0 ? `pass (${result.focusable} focus stops)` : `FAIL ${result.failures.length}`;
        console.log(`[${results.length}/${total}] ${scene.name} ${configuration.id}: ${status}`);
        for (const failure of result.failures) console.log(`    ${failure.check}: ${failure.detail}`);
      }
    }
  } finally {
    devtools?.close();
    spawnSync("agent-browser", ["--session", SESSION, "close"], { encoding: "utf8" });
    cleanup();
  }

  const failed = results.filter((result) => result.failures.length > 0);
  const report = {
    date: new Date().toISOString(),
    engine: "Chromium via agent-browser; not WebKitGTK",
    agentBrowser: version.stdout.trim(),
    configurations: options.configurations.map((configuration) => configuration.id),
    scenes: options.scenes.map((scene) => scene.name),
    runs: results.length,
    failed: failed.length,
    unmocked: [...new Set(results.flatMap((result) => result.unmocked))].sort(),
    warnings: summarise(results.flatMap((result) => result.warnings)),
    results,
  };
  writeFileSync(join(output, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
  console.log(`${results.length} runs, ${failed.length} failed. Report: ${join(output, "report.json")}`);
  return failed.length === 0 ? 0 : 1;
}

/** Warning counts by check, so the report reads at a glance. */
function summarise(warnings: Finding[]): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const warning of warnings) counts[warning.check] = (counts[warning.check] ?? 0) + 1;
  return counts;
}

main().then(
  (status) => process.exit(status),
  (error: unknown) => {
    console.error(error instanceof AuditError ? error.message : error);
    process.exit(2);
  },
);
