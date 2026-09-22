import corpus from "../../../../../fixtures/desktop/presentation-v1.json";

type FixturePreview = {
  id: string;
  state: string;
  summary: string;
  planeName: string;
  notice: {
    tone: string;
    title: string;
    message: string;
  };
};

type UnknownRecord = Record<string, unknown>;

function record(value: unknown, field: string): UnknownRecord {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`Fixture field ${field} is invalid.`);
  }
  return value as UnknownRecord;
}

function text(value: unknown, field: string, maximum: number): string {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > maximum ||
    /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/u.test(value)
  ) {
    throw new Error(`Fixture field ${field} is invalid.`);
  }
  return value;
}

function loadFirstLaunchFixture(value: unknown): FixturePreview {
  // ASVS 1.5.2 and 2.2.1: treat the shared JSON corpus as untrusted input,
  // select an allowlisted shape, and render every value through Svelte text
  // interpolation rather than executable HTML.
  const root = record(value, "root");
  if (root.format_version !== 1 || !Array.isArray(root.scenarios)) {
    throw new Error("Unsupported desktop fixture format.");
  }
  const candidate = root.scenarios.find(
    (scenario) => record(scenario, "scenario").id === "first-launch",
  );
  const scenario = record(candidate, "first-launch");
  const plane = record(scenario.plane, "plane");
  const notice = record(scenario.notice, "notice");

  return {
    id: text(scenario.id, "id", 64),
    state: text(scenario.state, "state", 32),
    summary: text(scenario.summary, "summary", 240),
    planeName: text(plane.name, "plane.name", 80),
    notice: {
      tone: text(notice.tone, "notice.tone", 32),
      title: text(notice.title, "notice.title", 100),
      message: text(notice.message, "notice.message", 240),
    },
  };
}

export const firstLaunchFixture = loadFirstLaunchFixture(corpus);
