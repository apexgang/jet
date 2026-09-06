// Test-only interpreter for the emitted JSON Schema vocabulary, not a GUI codec.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";
import type { CraftCommand, CraftEvent, ProtocolOffer } from "../CraftModels.ts";

const root = JSON.parse(readFileSync(new URL("../craft-v1.schema.json", import.meta.url), "utf8"));
type Schema = boolean | Record<string, any>;
function resolve(s: Schema): Record<string, any> {
  if (typeof s === "boolean") return {};
  return s.$ref ? resolve(root.$defs[s.$ref.split("/").at(-1)]) : s;
}
function properties(s: Schema, kind?: string): Record<string, Schema> {
  const node = resolve(s);
  const tag = node.properties?.kind;
  if (kind !== undefined && ((tag?.const !== undefined && tag.const !== kind) || tag?.not?.enum?.includes(kind))) return {};
  return Object.assign({}, node.properties, ...(node.oneOf ?? node.anyOf ?? []).map((part: Schema) => properties(part, kind)));
}
function uniqueFields(source: string, schema: Schema): boolean {
  // Inspect original tokens before dictionaries can discard duplicate known
  // fields. Opaque native payloads have no schema properties to interpret.
  const tokens = source.match(/"(?:\\.|[^"\\])*"|[{}\[\],:]|[^{}\[\],:\s]+/g) ?? [];
  let index = 0;
  let valid = true;
  function objectKind(): string | undefined {
    let depth = 0, kind: string | undefined;
    for (let cursor = index; cursor < tokens.length; cursor++) {
      const token = tokens[cursor];
      if (token === "{" || token === "[") depth++;
      else if (token === "}" || token === "]") { if (depth-- === 0) break; }
      else if (depth === 0 && token.startsWith('"') && tokens[cursor + 1] === ":" && tokens[cursor + 2]?.startsWith('"') && JSON.parse(token) === "kind") kind = JSON.parse(tokens[cursor + 2]);
    }
    return kind;
  }
  function visit(s: Schema): void {
    const token = tokens[index++];
    if (token === "{") {
      const fields = properties(s, objectKind()), seen = new Set<string>();
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
  if (s.$ref) return matches(root.$defs[s.$ref.split("/").at(-1)], value);
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
  if (s.type === "array") return Array.isArray(value) && value.every(item => matches(s.items, item));
  if (s.type === "object") {
    if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
    const object = value as Record<string, unknown>;
    if ((s.required ?? []).some((key: string) => !(key in object))) return false;
    return Object.entries(s.properties ?? {}).every(([key, part]) => !(key in object) || matches(part as Schema, object[key]));
  }
  return true;
}

const fixtures = JSON.parse(readFileSync(new URL("../fixtures.json", import.meta.url), "utf8"));
for (const fixture of fixtures) {
  // Keep the original payload for forwarding; the parsed tree is only a view.
  const s = root.$defs[fixture.schema];
  const unique = uniqueFields(fixture.payload, s);
  const view: CraftCommand | CraftEvent | ProtocolOffer = JSON.parse(fixture.payload);
  assert.equal(unique && matches(s, view), fixture.valid, fixture.payload);
}
console.log(`TypeScript: ${fixtures.length} shared Craft fixtures passed`);
