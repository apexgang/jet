import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import postcss, { type AtRule, type ChildNode, type Container, type Declaration, type Rule } from "postcss";
import { describe, expect, it } from "vitest";

import { contrastRatio } from "./support/contrast";

// The theme contract (Wave 3.4 §6.3): colours are tokens, tokens resolve in
// every appearance and media combination with WCAG contrast, and every media
// adaptation (increased contrast, forced colours, reduced motion and
// transparency) has a rule wherever the base styles need one.

const SOURCE = new URL("../src/", import.meta.url).pathname;
const THEME = "lib/features/shell/theme.css";

type Source = { name: string; css: string };

function svelteFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return svelteFiles(path);
    return entry.name.endsWith(".svelte") ? [path] : [];
  });
}

function sources(): Source[] {
  const styles = svelteFiles(SOURCE).flatMap((file) => {
    const match = /<style[^>]*>([\s\S]*?)<\/style>/.exec(readFileSync(file, "utf8"));
    return match ? [{ name: relative(SOURCE, file), css: match[1] }] : [];
  });
  return [{ name: THEME, css: readFileSync(join(SOURCE, THEME), "utf8") }, ...styles];
}

const ALL = sources();
const THEME_ROOT = postcss.parse(ALL[0].css);

// ---------------------------------------------------------------------------
// Tree helpers

function normalise(selector: string): string {
  return selector.replace(/\s+/g, " ").replace(/'/g, '"').trim();
}

function mediaAncestors(node: ChildNode): string[] {
  const params: string[] = [];
  for (let parent: Container | undefined = node.parent as Container | undefined; parent; parent = parent.parent as Container | undefined) {
    if (parent.type === "atrule" && (parent as AtRule).name === "media") params.push((parent as AtRule).params);
  }
  return params;
}

function inKeyframes(rule: Rule): boolean {
  const parent = rule.parent;
  return parent?.type === "atrule" && /keyframes$/.test((parent as AtRule).name);
}

function underMedia(rule: Rule, feature: RegExp): boolean {
  return mediaAncestors(rule).some((params) => feature.test(params));
}

const FORCED = /\(\s*forced-colors\s*:\s*active\s*\)/;
const REDUCED_MOTION = /\(\s*prefers-reduced-motion\s*:\s*reduce\s*\)/;
const CONTRAST_MORE = /\(\s*prefers-contrast\s*:\s*more\s*\)/;
const LIGHT_SCHEME = /\(\s*prefers-color-scheme\s*:\s*light\s*\)/;

function rules(css: string): Rule[] {
  const found: Rule[] = [];
  postcss.parse(css).walkRules((rule) => {
    if (!inKeyframes(rule)) found.push(rule);
  });
  return found;
}

function declarations(rule: Rule): Declaration[] {
  return (rule.nodes ?? []).filter((node): node is Declaration => node.type === "decl");
}

function isTokenRule(rule: Rule): boolean {
  return declarations(rule).some((declaration) => declaration.prop.startsWith("--"));
}

// ---------------------------------------------------------------------------
// Token cascade evaluator: selector match + specificity + source order.

type Appearance = "system" | "light" | "dark";
type Scheme = "dark" | "light";
type Contrast = "no-preference" | "more";
type Environment = { appearance: Appearance; scheme: Scheme; contrast: Contrast };

type SelectorMatch = { specificity: number; matches(environment: Environment): boolean };

function tokenSelector(selector: string): SelectorMatch | null {
  const normalised = normalise(selector);
  if (normalised === ":root") return { specificity: 10, matches: () => true };
  const only = /^:root\[data-appearance="(light|dark)"\]$/.exec(normalised);
  if (only) return { specificity: 20, matches: (environment) => environment.appearance === only[1] };
  const not = /^:root:not\(\[data-appearance="(light|dark)"\]\)$/.exec(normalised);
  if (not) return { specificity: 20, matches: (environment) => environment.appearance !== not[1] };
  return null;
}

const MEDIA_FEATURE = /^\(\s*(prefers-color-scheme|prefers-contrast|forced-colors|prefers-reduced-transparency)\s*:\s*([a-z-]+)\s*\)$/;

function tokenMedia(params: string): ((environment: Environment) => boolean) | null {
  const conditions = params.trim().split(/\s+and\s+/i).map((part) => MEDIA_FEATURE.exec(part.trim()));
  if (conditions.some((condition) => condition === null)) return null;
  return (environment) =>
    conditions.every((condition) => {
      const [, feature, value] = condition!;
      switch (feature) {
        case "prefers-color-scheme":
          return value === environment.scheme;
        case "prefers-contrast":
          return value === environment.contrast;
        // Tokens are evaluated without forced colours or reduced transparency.
        case "forced-colors":
          return value === "none";
        default:
          return value === "no-preference";
      }
    });
}

type TokenDeclaration = {
  token: string;
  value: string;
  specificity: number;
  order: number;
  base: boolean;
  applies(environment: Environment): boolean;
};

type TokenBlock = { selectors: string[]; media: string[]; tokens: string[] };

function tokenModel() {
  const declarationsFound: TokenDeclaration[] = [];
  const blocks: TokenBlock[] = [];
  const violations: string[] = [];
  let order = 0;
  THEME_ROOT.walkRules((rule) => {
    if (inKeyframes(rule) || !isTokenRule(rule)) return;
    for (let parent = rule.parent as Container | undefined; parent; parent = parent.parent as Container | undefined) {
      if (parent.type === "atrule" && (parent as AtRule).name !== "media") violations.push(`@${(parent as AtRule).name} around ${rule.selector}`);
    }
    const media = mediaAncestors(rule);
    const conditions = media.map(tokenMedia);
    const selectors = rule.selectors.map((selector) => ({ selector, match: tokenSelector(selector) }));
    for (const { selector, match } of selectors) {
      if (!match) violations.push(`selector ${normalise(selector)}`);
    }
    media.forEach((params, index) => {
      if (!conditions[index]) violations.push(`media ${params}`);
    });
    const tokens = declarations(rule).filter((declaration) => declaration.prop.startsWith("--"));
    blocks.push({ selectors: rule.selectors.map(normalise), media, tokens: tokens.map((token) => token.prop) });
    for (const declaration of tokens) {
      order += 1;
      for (const { selector, match } of selectors) {
        if (!match) continue;
        declarationsFound.push({
          token: declaration.prop,
          value: declaration.value.trim(),
          specificity: match.specificity,
          order,
          base: media.length === 0 && normalise(selector) === ":root",
          applies: (environment) =>
            match.matches(environment) && conditions.every((condition) => condition?.(environment) ?? false),
        });
      }
    }
  });
  return { declarations: declarationsFound, blocks, violations };
}

const MODEL = tokenModel();
const BASE_TOKENS = [...new Set(MODEL.declarations.filter((entry) => entry.base).map((entry) => entry.token))];
const VAR_ONLY = /^var\((--[\w-]+)\)$/;

function winners(environment: Environment): Map<string, TokenDeclaration> {
  const result = new Map<string, TokenDeclaration>();
  for (const entry of MODEL.declarations) {
    if (!entry.applies(environment)) continue;
    const current = result.get(entry.token);
    if (
      !current ||
      entry.specificity > current.specificity ||
      (entry.specificity === current.specificity && entry.order > current.order)
    ) {
      result.set(entry.token, entry);
    }
  }
  return result;
}

function resolveAll(environment: Environment): Map<string, string> {
  const winning = winners(environment);
  const resolved = new Map<string, string>();
  const resolve = (token: string, seen: string[]): string => {
    if (seen.includes(token)) throw new Error(`Token cycle: ${[...seen, token].join(" -> ")}`);
    const entry = winning.get(token);
    if (!entry) throw new Error(`${token} is undefined in ${describeEnvironment(environment)}`);
    const reference = VAR_ONLY.exec(entry.value);
    const value = reference ? resolve(reference[1], [...seen, token]) : entry.value;
    if (!reference && /var\(/.test(value)) {
      throw new Error(`${token} mixes var() into a value; tokens must be literals or a single var()`);
    }
    return value;
  };
  for (const token of winning.keys()) resolved.set(token, resolve(token, []));
  return resolved;
}

function describeEnvironment(environment: Environment): string {
  return `appearance ${environment.appearance}, system ${environment.scheme}, contrast ${environment.contrast}`;
}

const HAS_APPEARANCE_OVERRIDE = MODEL.blocks.some((block) => block.selectors.some((selector) => selector.includes("data-appearance")));
const APPEARANCES: Appearance[] = HAS_APPEARANCE_OVERRIDE ? ["system", "light", "dark"] : ["system"];
const ENVIRONMENTS: Environment[] = APPEARANCES.flatMap((appearance) =>
  (["dark", "light"] as const).flatMap((scheme) =>
    (["no-preference", "more"] as const).map((contrast) => ({ appearance, scheme, contrast })),
  ),
);

function effectiveScheme(environment: Environment): string {
  const scheme = environment.appearance === "system" ? environment.scheme : environment.appearance;
  return environment.contrast === "more" ? `${scheme}+more` : scheme;
}

// ---------------------------------------------------------------------------

describe("theme tokens", () => {
  it("declare tokens only in :root blocks under the supported media features", () => {
    expect(MODEL.violations).toEqual([]);
    for (const source of ALL.slice(1)) {
      const tokenRules = rules(source.css).filter(isTokenRule).map((rule) => `${source.name}: ${rule.selector}`);
      expect(tokenRules).toEqual([]);
    }
  });

  it("resolve every base token in every environment", () => {
    expect(BASE_TOKENS.length).toBeGreaterThan(40);
    for (const environment of ENVIRONMENTS) {
      const resolved = resolveAll(environment);
      const missing = BASE_TOKENS.filter((token) => !resolved.get(token));
      expect(missing, describeEnvironment(environment)).toEqual([]);
    }
  });

  it("redefine every literal base token for the light scheme", () => {
    const literal = BASE_TOKENS.filter((token) => {
      const base = MODEL.declarations.find((entry) => entry.base && entry.token === token)!;
      return !VAR_ONLY.test(base.value);
    });
    for (const environment of ENVIRONMENTS.filter((entry) => effectiveScheme(entry) === "light")) {
      const winning = winners(environment);
      const inherited = literal.filter((token) => winning.get(token)?.base);
      expect(inherited, describeEnvironment(environment)).toEqual([]);
    }
  });

  it("resolve an appearance override to the same values as the matching system scheme", () => {
    for (const environment of ENVIRONMENTS.filter((entry) => entry.appearance !== "system")) {
      const system = { ...environment, appearance: "system" as const, scheme: environment.appearance as Scheme };
      expect(Object.fromEntries(resolveAll(environment)), describeEnvironment(environment)).toEqual(
        Object.fromEntries(resolveAll(system)),
      );
    }
  });

  it("declare the same tokens in every increased-contrast block", () => {
    const more = MODEL.blocks.filter((block) => block.media.some((params) => CONTRAST_MORE.test(params)));
    expect(more.length).toBeGreaterThanOrEqual(2);
    const expected = [...more[0].tokens].sort();
    for (const block of more) expect([...block.tokens].sort()).toEqual(expected);
    // Increased contrast changes something in both schemes.
    for (const scheme of ["dark", "light"] as const) {
      const normal = resolveAll({ appearance: "system", scheme, contrast: "no-preference" });
      const raised = resolveAll({ appearance: "system", scheme, contrast: "more" });
      for (const token of expected) expect(raised.get(token), `${scheme} ${token}`).not.toEqual(normal.get(token));
    }
  });

  const PAIRS: Array<{ foreground: string[]; background: string[]; minimum: number }> = [
    { foreground: ["--text", "--muted", "--quiet"], background: ["--background", "--sidebar", "--panel", "--raised"], minimum: 4.5 },
    {
      foreground: ["--success", "--warning", "--danger-text", "--accent-text"],
      background: ["--background", "--sidebar", "--raised", "--selected"],
      minimum: 4.5,
    },
    { foreground: ["--on-accent"], background: ["--accent", "--accent-strong", "--accent-active"], minimum: 4.5 },
    { foreground: ["--on-danger"], background: ["--danger-fill", "--danger-fill-hover"], minimum: 4.5 },
    { foreground: ["--attention-text"], background: ["--attention-bg"], minimum: 4.5 },
    { foreground: ["--working-text", "--warning-text"], background: ["--background", "--panel", "--raised"], minimum: 4.5 },
    { foreground: ["--code-text"], background: ["--code-bg"], minimum: 4.5 },
    { foreground: ["--placeholder"], background: ["--raised"], minimum: 4.5 },
    { foreground: ["--text", "--muted"], background: ["--notice-bg", "--critical-bg"], minimum: 4.5 },
    { foreground: ["--focus"], background: ["--background", "--raised", "--sidebar"], minimum: 3 },
    {
      foreground: ["--border-strong"],
      background: ["--background", "--sidebar", "--panel", "--raised", "--hover", "--selected"],
      minimum: 3,
    },
    { foreground: ["--selected-indicator"], background: ["--sidebar", "--raised"], minimum: 3 },
  ];

  it("meet WCAG contrast in every effective scheme", () => {
    const failures: string[] = [];
    const seen = new Set<string>();
    for (const environment of ENVIRONMENTS) {
      const scheme = effectiveScheme(environment);
      if (seen.has(scheme)) continue;
      seen.add(scheme);
      const resolved = resolveAll(environment);
      for (const pair of PAIRS) {
        for (const foreground of pair.foreground) {
          for (const background of pair.background) {
            const ratio = contrastRatio(resolved.get(foreground)!, resolved.get(background)!);
            if (ratio < pair.minimum) failures.push(`${scheme}: ${foreground} on ${background} ${ratio.toFixed(2)} < ${pair.minimum}`);
          }
        }
      }
    }
    expect([...seen].sort()).toEqual(["dark", "dark+more", "light", "light+more"]);
    expect(failures).toEqual([]);
  });
});

describe("stylesheets", () => {
  const COLOUR_LITERAL = /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?)\(/i;

  function offenders(check: (source: Source, rule: Rule) => string[]): string[] {
    return ALL.flatMap((source) => rules(source.css).flatMap((rule) => check(source, rule)));
  }

  it("use colour literals only in token declarations", () => {
    const found = offenders((source, rule) =>
      declarations(rule)
        .filter((declaration) => !(source.name === THEME && declaration.prop.startsWith("--") && isTokenRule(rule)))
        .filter((declaration) => COLOUR_LITERAL.test(declaration.value))
        .map((declaration) => `${source.name}: ${rule.selector} { ${declaration.toString()} }`),
    );
    expect(found).toEqual([]);
  });

  it("have no decorative glass", () => {
    const found = offenders((source, rule) =>
      declarations(rule)
        .filter((declaration) => /backdrop-filter$/.test(declaration.prop))
        .map((declaration) => `${source.name}: ${rule.selector} { ${declaration.toString()} }`),
    );
    expect(found).toEqual([]);
  });

  it("use --accent-text, not --accent, for text", () => {
    const found = offenders((source, rule) =>
      declarations(rule)
        .filter((declaration) => declaration.prop === "color" && /var\(--accent\)/.test(declaration.value))
        .map((declaration) => `${source.name}: ${rule.selector}`),
    );
    expect(found).toEqual([]);
  });

  function selectorsWhere(source: Source, media: RegExp, predicate: (declaration: Declaration) => boolean): Set<string> {
    const found = new Set<string>();
    for (const rule of rules(source.css)) {
      if (!underMedia(rule, media) || !declarations(rule).some(predicate)) continue;
      for (const selector of rule.selectors) found.add(normalise(selector));
    }
    return found;
  }

  it("replace every removed outline under forced colours", () => {
    const found = offenders((source, rule) => {
      if (underMedia(rule, FORCED)) return [];
      if (!declarations(rule).some((declaration) => declaration.prop === "outline" && /^(0|none)$/.test(declaration.value.trim()))) return [];
      const replacements = selectorsWhere(
        source,
        FORCED,
        (declaration) => declaration.prop.startsWith("outline") && !/^(0|none)$/.test(declaration.value.trim()),
      );
      return rule.selectors.map(normalise).filter((selector) => {
        // The indicator may move to an ancestor that shows focus-within.
        const parts = selector.split(" ");
        const ancestors = parts.slice(0, -1).map((_, index) => `${parts.slice(0, index + 1).join(" ")}:focus-within`);
        return ![selector, ...ancestors].some((candidate) => replacements.has(candidate));
      }).map((selector) => `${source.name}: ${selector}`);
    });
    expect(found).toEqual([]);
  });

  it("stop every animation and delayed or timed transition under reduced motion", () => {
    const moves = (declaration: Declaration) =>
      (/^animation(-name)?$/.test(declaration.prop) && declaration.value.trim() !== "none") ||
      (declaration.prop === "transition" &&
        declaration.value.trim() !== "none" &&
        [...declaration.value.matchAll(/(\d*\.?\d+)(ms|s)\b/g)].some((time) => Number(time[1]) > 0));
    const found = offenders((source, rule) => {
      if (underMedia(rule, REDUCED_MOTION) || !declarations(rule).some(moves)) return [];
      const stilled = selectorsWhere(
        source,
        REDUCED_MOTION,
        (declaration) => /^(animation|transition)$/.test(declaration.prop) && declaration.value.trim() === "none",
      );
      return rule.selectors.map(normalise).filter((selector) => !stilled.has(selector)).map((selector) => `${source.name}: ${selector}`);
    });
    expect(found).toEqual([]);
  });

  it("show disabled controls in GrayText under forced colours instead of opacity", () => {
    const disabled = /:disabled|\[aria-disabled="true"\]/;
    const found = offenders((source, rule) => {
      if (underMedia(rule, FORCED)) return [];
      if (!declarations(rule).some((declaration) => declaration.prop === "opacity")) return [];
      const gray = selectorsWhere(source, FORCED, (declaration) => declaration.prop === "color" && declaration.value.trim() === "GrayText");
      return rule.selectors
        .map(normalise)
        .filter((selector) => disabled.test(selector) && !gray.has(selector))
        .map((selector) => `${source.name}: ${selector}`);
    });
    expect(found).toEqual([]);
  });

  it("adapt the theme to increased contrast, forced colours and reduced transparency", () => {
    const media = new Set<string>();
    THEME_ROOT.walkAtRules("media", (rule) => {
      media.add(rule.params);
    });
    const all = [...media].join("\n");
    expect(all).toMatch(CONTRAST_MORE);
    expect(all).toMatch(LIGHT_SCHEME);
    expect(all).toMatch(FORCED);
    expect(all).toMatch(/\(\s*prefers-reduced-transparency\s*:\s*reduce\s*\)/);
    expect(all).toMatch(REDUCED_MOTION);
  });
});
