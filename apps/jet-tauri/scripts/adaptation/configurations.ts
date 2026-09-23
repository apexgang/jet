/**
 * The adaptation audit's configuration and scene lists (wave 3.4 §8).
 *
 * Local only: `just adaptation-audit` runs them in Chromium through
 * `agent-browser` against mocked IPC. Chromium is not WebKitGTK, so the audit
 * proves CSS, layout logic and axe rules, not the engine, the CSP or native
 * window behaviour. Those are in the manual checklist.
 */

import { OVERLAY_BREAKPOINT } from "../../src/lib/features/shell/layout";

export type ColorScheme = "light" | "dark";

/** One emulated window and media environment. */
export type Configuration = {
  id: string;
  width: number;
  height: number;
  /** Device pixel ratio. */
  scale: number;
  colorScheme: ColorScheme;
  contrast: "no-preference" | "more";
  forcedColors: "none" | "active";
  reducedMotion: "no-preference" | "reduce";
  reducedTransparency: "no-preference" | "reduce";
};

export type SceneName =
  | "setup"
  | "approval-run"
  | "changes"
  | "new-task"
  | "planes"
  | "schedules"
  | "trash"
  | "settings";

/** What the page shows. The mocked IPC builds its answers from the name. */
export type Scene = {
  name: SceneName;
  /** The app route. The scene name travels in the query string. */
  route: "/" | "/settings";
  /**
   * A sidebar destination to open after load, by its visible button text,
   * for destinations the saved layout does not restore.
   */
  sidebarButton: string | null;
  /** The scene shows a task with a composer, so the Send button is checked. */
  composer: boolean;
};

export const SCENES: readonly Scene[] = [
  { name: "setup", route: "/", sidebarButton: null, composer: false },
  { name: "approval-run", route: "/", sidebarButton: null, composer: true },
  { name: "changes", route: "/", sidebarButton: null, composer: true },
  { name: "new-task", route: "/", sidebarButton: null, composer: true },
  { name: "planes", route: "/", sidebarButton: null, composer: false },
  { name: "schedules", route: "/", sidebarButton: null, composer: false },
  { name: "trash", route: "/", sidebarButton: "Jet Trash", composer: false },
  { name: "settings", route: "/settings", sidebarButton: null, composer: false },
];

const BASE: Omit<Configuration, "id" | "width" | "height"> = {
  scale: 1,
  colorScheme: "dark",
  contrast: "no-preference",
  forcedColors: "none",
  reducedMotion: "no-preference",
  reducedTransparency: "no-preference",
};

const VIEWPORTS = [
  { width: 900, height: 600, scale: 1 },
  { width: 1100, height: 700, scale: 1 },
  { width: 1101, height: 700, scale: 1 },
  { width: 1280, height: 800, scale: 1 },
  { width: 1920, height: 1080, scale: 1 },
  { width: 2560, height: 1440, scale: 1 },
  { width: 1280, height: 800, scale: 2 },
] as const;

type Feature = Pick<Configuration, "colorScheme"> & Partial<Configuration> & { name: string };

/** Media features checked at the default and the minimum window size. */
const FEATURES: readonly Feature[] = [
  { name: "more-light", colorScheme: "light", contrast: "more" },
  { name: "more-dark", colorScheme: "dark", contrast: "more" },
  { name: "forced-dark", colorScheme: "dark", forcedColors: "active" },
  { name: "forced-light", colorScheme: "light", forcedColors: "active" },
  { name: "motion-dark", colorScheme: "dark", reducedMotion: "reduce" },
  { name: "transparency-light", colorScheme: "light", reducedTransparency: "reduce" },
];

function viewportId(viewport: (typeof VIEWPORTS)[number]): string {
  return `${viewport.width}x${viewport.height}${viewport.scale === 1 ? "" : `@${viewport.scale}`}`;
}

export const CONFIGURATIONS: readonly Configuration[] = [
  ...VIEWPORTS.flatMap((viewport) =>
    (["light", "dark"] as const).map((colorScheme) => ({
      ...BASE,
      ...viewport,
      colorScheme,
      id: `${viewportId(viewport)}-${colorScheme}`,
    })),
  ),
  ...[VIEWPORTS[3], VIEWPORTS[0]].flatMap((viewport) =>
    FEATURES.map(({ name, ...feature }) => ({
      ...BASE,
      ...viewport,
      ...feature,
      id: `${viewportId(viewport)}-${name}`,
    })),
  ),
];

/**
 * What the plan asks the audit to cover (design-language l.44, l.50, l.183,
 * l.192; `docs/desktop-implementation-plan.md` §3.4). The audit refuses to
 * run when a configuration list edit drops one of them.
 *
 * Full screen and multiple displays are native window states; they are in
 * the manual checklist. 3.2 shipped no appearance override
 * (`docs/wave-3.2.md`), so there is no override configuration to emulate.
 */
export const REQUIRED: ReadonlyArray<{ name: string; covered: (configuration: Configuration) => boolean }> = [
  { name: "narrow 900x600", covered: (c) => c.width === 900 && c.height === 600 },
  { name: "compact edge 1100", covered: (c) => c.width === OVERLAY_BREAKPOINT },
  { name: "regular edge 1101", covered: (c) => c.width === OVERLAY_BREAKPOINT + 1 },
  { name: "default 1280x800", covered: (c) => c.width === 1280 && c.height === 800 && c.scale === 1 },
  { name: "wide 1920", covered: (c) => c.width === 1920 },
  { name: "wide 2560", covered: (c) => c.width === 2560 },
  { name: "HiDPI 2x", covered: (c) => c.scale === 2 },
  { name: "light", covered: (c) => c.colorScheme === "light" && c.contrast === "no-preference" },
  { name: "dark", covered: (c) => c.colorScheme === "dark" && c.contrast === "no-preference" },
  { name: "increased contrast light", covered: (c) => c.contrast === "more" && c.colorScheme === "light" },
  { name: "increased contrast dark", covered: (c) => c.contrast === "more" && c.colorScheme === "dark" },
  { name: "forced colors", covered: (c) => c.forcedColors === "active" },
  { name: "reduced motion", covered: (c) => c.reducedMotion === "reduce" },
  { name: "reduced transparency", covered: (c) => c.reducedTransparency === "reduce" },
  {
    name: "media features at the minimum size",
    covered: (c) => c.width === 900 && (c.contrast === "more" || c.forcedColors === "active"),
  },
];

export const REQUIRED_SCENES: readonly SceneName[] = [
  "setup",
  "approval-run",
  "changes",
  "new-task",
  "planes",
  "schedules",
  "trash",
  "settings",
];

/** Names of required configurations or scenes the lists no longer cover. */
export function missingCoverage(
  configurations: readonly Configuration[] = CONFIGURATIONS,
  scenes: readonly Scene[] = SCENES,
): string[] {
  const ids = new Set<string>();
  const duplicates = configurations.filter((configuration) => {
    const seen = ids.has(configuration.id);
    ids.add(configuration.id);
    return seen;
  });
  return [
    ...REQUIRED.filter((required) => !configurations.some(required.covered)).map((required) => required.name),
    ...REQUIRED_SCENES.filter((name) => !scenes.some((scene) => scene.name === name)).map((name) => `scene ${name}`),
    ...duplicates.map((configuration) => `duplicate configuration ${configuration.id}`),
  ];
}

export function isCompact(configuration: Configuration): boolean {
  return configuration.width <= OVERLAY_BREAKPOINT;
}
