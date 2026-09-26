/**
 * What the desktop release journey reads inside the app's webview. The
 * journey sends `jetPage` as a WebDriver script, so it must stay
 * self-contained: no imports, no module-level names, only DOM globals.
 * `tests/e2e-page.component.test.ts` runs its serialized source against the
 * real Svelte components.
 *
 * Elements are found the way assistive technology names them: by role and
 * accessible name (`aria-labelledby`, `aria-label`, then text), never by
 * class names. The names come from the components:
 *
 * - main window: the `Jet navigation` complementary landmark (Sidebar), its
 *   Plane status live region, the `Set up Jet` region
 *   (SetupPanel) with its provisioning status, failure alert and the
 *   `Local Plane` region, and the `Task message` composer;
 * - Settings window: the `Settings` navigation (SettingsApp), the `Versions
 *   and capabilities` region (VersionsSection) with the `Jet service on this
 *   computer` and `App updates` blocks (LocalServiceBlock, AppUpdatesBlock).
 */

export type PageCommand = "main" | "settings" | "button" | "reveal" | "text";

export type Mark = { name: string; startTime: number };

export type SetupView = {
  /** The sentence under "Connecting to the local Jet service", while provisioning. */
  provisioning: string | null;
  /** Setup's alert: its heading and full text (with any error code). */
  failure: { title: string; text: string } | null;
  /** The `Local Plane` row once Setup has read the Plane. */
  localPlane: {
    text: string;
    coreVersion: string | null;
    /** "Connected" or "Needs attention". */
    status: string | null;
    /** What the last provisioning pass changed, e.g. "The Jet service 0.2.0 is set up on this computer." */
    notice: string | null;
  } | null;
};

export type MainView = {
  readyState: string;
  title: string;
  timeOrigin: number;
  marks: Mark[];
  /** The sidebar's Plane status text, e.g. "This computer Connected". */
  planeStatus: string | null;
  /** Its state word: Connected, Connecting, Reconnecting, Unavailable, … */
  planeState: string | null;
  composer: boolean;
  setup: SetupView | null;
};

export type SettingsView = {
  readyState: string;
  title: string;
  panes: Array<{ name: string; current: boolean; disabled: boolean }>;
  /** The shown pane's title. */
  pane: string | null;
  versions: {
    /** Term → definition in "Jet service on this computer". */
    service: { facts: Record<string, string>; live: string | null; buttons: string[] } | null;
    /** The App updates status line. */
    updates: string | null;
    /**
     * The App updates block's buttons and checkboxes, by accessible name.
     * The updater's own controls ("Check for updates", "Check for updates
     * automatically", …) show only while it is on; an unsigned, Homebrew or
     * unsupported install shows none, a failed read only "Try again".
     */
    updateControls: string[];
    /** "This app …" from the Plane's own facts, once they load. */
    thisApp: string | null;
  } | null;
};

export type ControlQuery = {
  /** The landmark or region to search in; null searches the whole page. */
  scope: { role: string; name: string } | null;
  /** Accepted accessible names, in order of preference. */
  names: string[];
};

export function jetPage(command: PageCommand, argument?: unknown): unknown {
  const ROLES: Record<string, string> = {
    alert: '[role="alert"]',
    button: 'button, [role="button"], input[type="button"], input[type="submit"]',
    checkbox: 'input[type="checkbox"], [role="checkbox"]',
    complementary: 'aside, [role="complementary"]',
    heading: 'h1, h2, h3, h4, h5, h6, [role="heading"]',
    navigation: 'nav, [role="navigation"]',
    region: 'section[aria-label], section[aria-labelledby], [role="region"]',
    status: '[role="status"], output',
    textbox: 'textarea, input:not([type]), input[type="text"], [role="textbox"]',
  };
  const PLANE_STATES = [
    "Connected",
    "Connecting",
    "Reconnecting",
    "Unavailable",
    "Not connected",
    "Needs pairing",
    "Session ended",
    "Read-only",
    "Security needs attention",
  ];

  const clean = (value: string | null | undefined): string => (value ?? "").replace(/\s+/g, " ").trim();
  const hidden = (element: Element): boolean => element.closest('[hidden], [aria-hidden="true"]') !== null;

  /** Visible text, with a space between elements, leaving out `skip` and hidden parts. */
  const textOf = (node: Node | null | undefined, skip: Node | null = null): string => {
    if (!node) return "";
    const parts: string[] = [];
    const walk = (current: Node) => {
      if (current === skip) return;
      if (current.nodeType === 3) {
        parts.push(current.nodeValue ?? "");
        return;
      }
      if (current.nodeType !== 1 && current.nodeType !== 9 && current.nodeType !== 11) return;
      if (current.nodeType === 1) {
        const element = current as Element;
        if (element.tagName === "SCRIPT" || element.tagName === "STYLE") return;
        if (element.getAttribute("aria-hidden") === "true" || element.hasAttribute("hidden")) return;
      }
      for (const child of Array.from(current.childNodes)) walk(child);
    };
    walk(node);
    return clean(parts.join(" "));
  };

  const nameOf = (element: Element): string => {
    const labelledBy = element.getAttribute("aria-labelledby");
    if (labelledBy) {
      const label = labelledBy
        .split(/\s+/)
        .map((id) => document.getElementById(id))
        .filter((label): label is HTMLElement => label !== null)
        .map((label) => textOf(label))
        .join(" ");
      if (clean(label)) return clean(label);
    }
    const label = element.getAttribute("aria-label");
    if (label && clean(label)) return clean(label);
    if (element.tagName === "INPUT" || element.tagName === "TEXTAREA" || element.tagName === "SELECT") {
      const owner = element.id
        ? Array.from(document.querySelectorAll("label")).find((candidate) => candidate.htmlFor === element.id)
        : undefined;
      const label = owner ?? element.closest("label");
      if (label) return textOf(label);
    }
    return textOf(element);
  };

  /** Elements with `role` (explicit or implicit) whose name is `name` or starts with it (a shortcut hint may follow). */
  const byRole = (scope: ParentNode | null | undefined, role: string, name?: string): Element[] => {
    if (!scope) return [];
    const selector = ROLES[role] ?? `[role="${role}"]`;
    return Array.from(scope.querySelectorAll(selector)).filter((element) => {
      const explicit = element.getAttribute("role");
      if (explicit !== null && explicit !== role) return false;
      if (hidden(element)) return false;
      if (name === undefined) return true;
      const actual = nameOf(element);
      return actual === name || actual.startsWith(`${name} `);
    });
  };
  const first = (elements: Element[]): Element | null => elements[0] ?? null;

  /** Term → definition for the description lists in `scope`, outside `skip`. */
  const factsOf = (scope: Element | null, skip: Element | null = null): Record<string, string> => {
    const facts: Record<string, string> = {};
    if (!scope) return facts;
    for (const term of Array.from(scope.querySelectorAll("dt"))) {
      if (skip && skip.contains(term)) continue;
      let definition = term.nextElementSibling;
      while (definition && definition.tagName !== "DD" && definition.tagName !== "DT") definition = definition.nextElementSibling;
      if (definition && definition.tagName === "DD") facts[textOf(term)] = textOf(definition);
    }
    return facts;
  };

  const marks = (): Mark[] => {
    try {
      return performance.getEntriesByType("mark").map((entry) => ({ name: entry.name, startTime: entry.startTime }));
    } catch {
      return [];
    }
  };
  const timeOrigin = (): number => {
    try {
      return performance.timeOrigin;
    } catch {
      return 0;
    }
  };

  const readSetup = (): SetupView | null => {
    const setup = first(byRole(document, "region", "Set up Jet"));
    if (!setup) return null;
    let provisioning: string | null = null;
    for (const status of byRole(setup, "status")) {
      const heading = first(byRole(status, "heading", "Connecting to the local Jet service"));
      if (heading) {
        provisioning = textOf(status, heading) || null;
        break;
      }
    }
    const alert = first(byRole(setup, "alert").filter((element) => element.closest("dialog") === null));
    const plane = first(byRole(setup, "region", "Jet service on this computer"));
    let localPlane: SetupView["localPlane"] = null;
    if (plane) {
      const text = textOf(plane);
      const version = /\bcore (\S+)/.exec(text);
      const notice = first(byRole(plane, "status").filter((element) => element.getAttribute("aria-label") !== "Connection status"));
      localPlane = {
        text,
        coreVersion: version ? version[1] : null,
        status: textOf(first(byRole(plane, "status", "Connection status"))) || null,
        notice: notice ? textOf(notice) : null,
      };
    }
    return {
      provisioning,
      failure: alert ? { title: textOf(first(byRole(alert, "heading"))), text: textOf(alert) } : null,
      localPlane,
    };
  };

  const readMain = (): MainView => {
    const navigation = first(byRole(document, "complementary", "Jet navigation"));
    let planeStatus: string | null = null;
    if (navigation) {
      // The status block is the navigation landmark's live region outside its `nav`.
      const live = Array.from(navigation.querySelectorAll('[aria-live], [role="status"]')).filter(
        (element) => element.closest("nav") === null && !hidden(element),
      );
      const last = live[live.length - 1];
      if (last) planeStatus = textOf(last) || null;
    }
    return {
      readyState: document.readyState,
      title: document.title,
      timeOrigin: timeOrigin(),
      marks: marks(),
      planeStatus,
      planeState: planeStatus === null ? null : (PLANE_STATES.find((state) => planeStatus!.endsWith(state)) ?? null),
      composer: first(byRole(document, "textbox", "Task message")) !== null,
      setup: readSetup(),
    };
  };

  const readSettings = (): SettingsView => {
    const navigation = first(byRole(document, "navigation", "Settings"));
    const panes = byRole(navigation, "button").map((button) => ({
      name: nameOf(button),
      current: button.getAttribute("aria-current") === "page",
      disabled: (button as HTMLButtonElement).disabled === true,
    }));
    const main = document.querySelector("main");
    const title = first(byRole(main, "heading").filter((heading) => heading.tagName === "H1"));
    const region = first(byRole(document, "region", "Versions and capabilities"));
    let versions: SettingsView["versions"] = null;
    if (region) {
      const block = (name: string): Element | null => first(byRole(region, "heading", name))?.parentElement ?? null;
      const serviceBlock = block("Jet service on this computer");
      const updatesBlock = block("App updates");
      const live = first(byRole(serviceBlock, "status"));
      versions = {
        service: serviceBlock
          ? {
              facts: factsOf(serviceBlock),
              live: live ? textOf(live) || null : null,
              buttons: byRole(serviceBlock, "button").map(nameOf),
            }
          : null,
        updates: textOf(first(byRole(updatesBlock, "status"))) || null,
        updateControls: [...byRole(updatesBlock, "button"), ...byRole(updatesBlock, "checkbox")].map(nameOf),
        thisApp: factsOf(region, serviceBlock)["This app"] ?? null,
      };
    }
    return {
      readyState: document.readyState,
      title: document.title,
      panes,
      pane: title ? textOf(title) : null,
      versions,
    };
  };

  switch (command) {
    case "main":
      return readMain();
    case "settings":
      return readSettings();
    case "button": {
      // Returned elements travel back as WebDriver element references.
      const query = argument as ControlQuery;
      const scope = query.scope ? first(byRole(document, query.scope.role, query.scope.name)) : document;
      if (!scope) return null;
      for (const name of query.names) {
        const button = first(byRole(scope, "button", name));
        if (button) return button;
      }
      return null;
    }
    case "reveal": {
      const heading = first(byRole(document, "heading", String(argument)));
      heading?.scrollIntoView({ block: "start" });
      return heading !== null;
    }
    case "text":
      return clean((document.body as HTMLElement | null)?.innerText ?? document.body?.textContent ?? "").slice(0, 20_000);
    default:
      throw new Error(`unknown page command ${String(command)}`);
  }
}

/** The WebDriver script body; `args` are `[command, argument]`. */
export const PAGE_SCRIPT = `return (${jetPage.toString()}).apply(null, arguments);`;
