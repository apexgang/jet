// Test-only interpreter for the emitted JSON Schema vocabulary, not a GUI codec.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

type Schema = boolean | Record<string, any>;

// One contract per call: a corpus is only meaningful against its own schema.
export function check<Model>(schemaURL: URL, fixturesURL: URL, contract: string): void {
  const root = JSON.parse(readFileSync(schemaURL, "utf8"));
  function resolve(s: Schema): Record<string, any> {
    if (typeof s === "boolean") return {};
    if (!s.$ref) return s;
    const base = resolve(root.$defs[s.$ref.split("/").at(-1)]);
    const { $ref, ...own } = s;
    return { ...base, ...own,
      properties: { ...base.properties, ...own.properties },
      required: [...(base.required ?? []), ...(own.required ?? [])] };
  }
  // Branches are tagged by whichever property carries a constant: "kind" on
  // Craft messages, "type" on an Actor, "breach" on an audit finding.
  function properties(s: Schema, tags: Record<string, string>): Record<string, Schema> {
    const node = resolve(s);
    for (const [field, constraint] of Object.entries(node.properties ?? {}) as [string, Record<string, any>][]) {
      const tag = tags[field];
      if (tag === undefined) continue;
      if (constraint.const !== undefined && constraint.const !== tag) return {};
      if (constraint.not?.enum?.includes(tag)) return {};
    }
    return Object.assign({}, node.properties, ...(node.oneOf ?? node.anyOf ?? []).map((part: Schema) => properties(part, tags)));
  }
  function uniqueFields(source: string, schema: Schema): boolean {
    // Inspect original tokens before dictionaries can discard duplicate known
    // fields. Opaque native payloads have no schema properties to interpret.
    const tokens = source.match(/"(?:\\.|[^"\\])*"|[{}\[\],:]|[^{}\[\],:\s]+/g) ?? [];
    let index = 0;
    let valid = true;
    function objectTags(): Record<string, string> {
      let depth = 0;
      const tags: Record<string, string> = {};
      for (let cursor = index; cursor < tokens.length; cursor++) {
        const token = tokens[cursor];
        if (token === "{" || token === "[") depth++;
        else if (token === "}" || token === "]") { if (depth-- === 0) break; }
        else if (depth === 0 && token.startsWith('"') && tokens[cursor + 1] === ":" && tokens[cursor + 2]?.startsWith('"')) tags[JSON.parse(token)] = JSON.parse(tokens[cursor + 2]);
      }
      return tags;
    }
    function visit(s: Schema): void {
      const token = tokens[index++];
      if (token === "{") {
        const fields = properties(s, objectTags()), seen = new Set<string>();
        while (index < tokens.length && tokens[index] !== "}") {
          const key = JSON.parse(tokens[index++]);
          if (key in fields && seen.has(key)) valid = false;
          seen.add(key);
          if (tokens[index++] !== ":") { valid = false; return; }
          visit(fields[key] ?? {});
          if (tokens[index] === ",") index++;
        }
        if (tokens[index++] !== "}") valid = false;
      } else if (token === "[") {
        while (index < tokens.length && tokens[index] !== "]") {
          visit(resolve(s).items ?? {});
          if (tokens[index] === ",") index++;
        }
        if (tokens[index++] !== "]") valid = false;
      }
    }
    visit(schema);
    return valid && index === tokens.length;
  }
  function matches(s: Schema, value: unknown): boolean {
    if (typeof s === "boolean") return s;
    if (s.$ref && !matches(root.$defs[s.$ref.split("/").at(-1)], value)) return false;
    if (s.not && matches(s.not, value)) return false;
    if (s.anyOf && !s.anyOf.some((part: Schema) => matches(part, value))) return false;
    if (s.oneOf && s.oneOf.filter((part: Schema) => matches(part, value)).length !== 1) return false;
    if ("const" in s && s.const !== value) return false;
    if (s.enum && !s.enum.includes(value)) return false;
    if (Array.isArray(s.type)) return s.type.some((type: string) => matches({ ...s, type }, value));
    if (s.type === "null") return value === null;
    if (s.type === "string" && typeof value !== "string") return false;
    if (s.type === "boolean" && typeof value !== "boolean") return false;
    if (s.type === "integer" && (typeof value !== "number" || !Number.isInteger(value))) return false;
    if (s.type === "number" && typeof value !== "number") return false;
    if (typeof value === "number" && ((s.minimum !== undefined && value < s.minimum) || (s.maximum !== undefined && value > s.maximum))) return false;
    if (s.pattern !== undefined && typeof value === "string" && !new RegExp(s.pattern).test(value)) return false;
    if (s.type === "array") return Array.isArray(value) && value.every(item => matches(s.items, item));
    if (s.type === "object") {
      if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
      const object = value as Record<string, unknown>;
      if ((s.required ?? []).some((key: string) => !(key in object))) return false;
      if (s.additionalProperties === false && Object.keys(object).some(key => !(key in (s.properties ?? {})))) return false;
      return Object.entries(s.properties ?? {}).every(([key, part]) => !(key in object) || matches(part as Schema, object[key]));
    }
    return true;
  }

  const fixtures = JSON.parse(readFileSync(fixturesURL, "utf8"));
  for (const fixture of fixtures) {
    // Keep the original payload for forwarding; the parsed tree is only a view.
    const s = root.$defs[fixture.schema];
    const unique = uniqueFields(fixture.payload, s);
    const view: Model = JSON.parse(fixture.payload);
    assert.equal(unique && matches(s, view), fixture.valid, fixture.payload);
  }
  console.log(`TypeScript: ${fixtures.length} shared ${contract} fixtures passed`);
}
