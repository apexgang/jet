import corpus from "../../../../../fixtures/desktop/presentation-v1.json";

export const fixtureStates = [
  "first_launch",
  "ready",
  "active",
  "queued",
  "approval",
  "completed",
  "offline",
  "stale_cursor",
  "denied",
  "unsupported",
  "recovery",
] as const;

export type FixtureState = (typeof fixtureStates)[number];
export type FixtureConnection = "connecting" | "online" | "offline" | "recovering";
export type TimelineKind = "user" | "agent" | "activity" | "approval" | "result" | "recovery";
export type NoticeTone = "informational" | "warning" | "critical";

export type DesktopFixtureScenario = {
  id: string;
  state: FixtureState;
  summary: string;
  plane: {
    id: string;
    name: string;
    connection: FixtureConnection;
    cursor: string;
  };
  project: { id: string; name: string } | null;
  conversation: { id: string; title: string } | null;
  run: { id: string; lifecycle: string; activity: string | null } | null;
  queue: Array<{ id: string; position: number; state: string; summary: string }>;
  timeline: Array<{ id: string; kind: TimelineKind; text: string }>;
  capabilities: { harnesses: string[]; missingFeatures: string[] };
  notice: { tone: NoticeTone; title: string; message: string } | null;
  primaryAction: {
    id: string;
    label: string;
    availability: "enabled" | "disabled";
    reason: string | null;
  } | null;
  backendDependencies: string[];
};

type UnknownRecord = Record<string, unknown>;

const connectionStates = ["connecting", "online", "offline", "recovering"] as const;
const timelineKinds = ["user", "agent", "activity", "approval", "result", "recovery"] as const;
const noticeTones = ["informational", "warning", "critical"] as const;
const actionAvailability = ["enabled", "disabled"] as const;
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;

function record(value: unknown, field: string): UnknownRecord {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`Fixture field ${field} is invalid.`);
  }
  return value as UnknownRecord;
}

function optionalRecord(value: unknown, field: string): UnknownRecord | null {
  return value === undefined || value === null ? null : record(value, field);
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

function optionalText(value: unknown, field: string, maximum: number): string | null {
  return value === undefined || value === null ? null : text(value, field, maximum);
}

function oneOf<const Values extends readonly string[]>(
  value: unknown,
  field: string,
  values: Values,
): Values[number] {
  const candidate = text(value, field, 64);
  if (!(values as readonly string[]).includes(candidate)) {
    throw new Error(`Fixture field ${field} is unsupported.`);
  }
  return candidate as Values[number];
}

function uuid(value: unknown, field: string): string {
  const candidate = text(value, field, 36);
  if (!uuidPattern.test(candidate)) {
    throw new Error(`Fixture field ${field} is not a UUID.`);
  }
  return candidate;
}

function canonicalCursor(value: unknown, field: string): string {
  const candidate = text(value, field, 20);
  if (!/^(0|[1-9][0-9]*)$/u.test(candidate)) {
    throw new Error(`Fixture field ${field} is not a canonical cursor.`);
  }
  return candidate;
}

function array(value: unknown, field: string, maximum: number): unknown[] {
  if (!Array.isArray(value) || value.length > maximum) {
    throw new Error(`Fixture field ${field} is invalid.`);
  }
  return value;
}

function stringArray(value: unknown, field: string, maximum: number): string[] {
  const values = array(value, field, maximum).map((item, index) =>
    text(item, `${field}[${index}]`, 96),
  );
  if (new Set(values).size !== values.length) {
    throw new Error(`Fixture field ${field} contains duplicates.`);
  }
  return values;
}

function positiveInteger(value: unknown, field: string, maximum: number): number {
  if (!Number.isSafeInteger(value) || Number(value) < 1 || Number(value) > maximum) {
    throw new Error(`Fixture field ${field} is invalid.`);
  }
  return Number(value);
}

function parseScenario(value: unknown, index: number): DesktopFixtureScenario {
  // ASVS 1.2.1, 2.1.1, and 2.2.1: select a bounded allowlisted shape from
  // the shared JSON corpus. Components render these values only through
  // Svelte text interpolation, never executable HTML.
  const item = record(value, `scenarios[${index}]`);
  const plane = record(item.plane, `${item.id}.plane`);
  const project = optionalRecord(item.project, `${item.id}.project`);
  const conversation = optionalRecord(item.conversation, `${item.id}.conversation`);
  const run = optionalRecord(item.run, `${item.id}.run`);
  const capabilities = record(item.capabilities, `${item.id}.capabilities`);
  const notice = optionalRecord(item.notice, `${item.id}.notice`);
  const primaryAction = optionalRecord(item.primary_action, `${item.id}.primary_action`);
  const contract = record(item.contract, `${item.id}.contract`);

  const queue = array(item.queue, `${item.id}.queue`, 128).map((entry, queueIndex) => {
    const turn = record(entry, `${item.id}.queue[${queueIndex}]`);
    return {
      id: uuid(turn.id, `${item.id}.queue[${queueIndex}].id`),
      position: positiveInteger(turn.position, `${item.id}.queue[${queueIndex}].position`, 128),
      state: text(turn.state, `${item.id}.queue[${queueIndex}].state`, 32),
      summary: text(turn.summary, `${item.id}.queue[${queueIndex}].summary`, 240),
    };
  });

  if (queue.some((turn, queueIndex) => turn.position !== queueIndex + 1)) {
    throw new Error(`Fixture ${item.id} has non-contiguous queue positions.`);
  }

  const timeline = array(item.timeline, `${item.id}.timeline`, 512).map((entry, entryIndex) => {
    const timelineEntry = record(entry, `${item.id}.timeline[${entryIndex}]`);
    return {
      id: text(timelineEntry.id, `${item.id}.timeline[${entryIndex}].id`, 96),
      kind: oneOf(
        timelineEntry.kind,
        `${item.id}.timeline[${entryIndex}].kind`,
        timelineKinds,
      ),
      text: text(timelineEntry.text, `${item.id}.timeline[${entryIndex}].text`, 2_000),
    };
  });

  return {
    id: text(item.id, `scenarios[${index}].id`, 64),
    state: oneOf(item.state, `${item.id}.state`, fixtureStates),
    summary: text(item.summary, `${item.id}.summary`, 240),
    plane: {
      id: uuid(plane.id, `${item.id}.plane.id`),
      name: text(plane.name, `${item.id}.plane.name`, 80),
      connection: oneOf(plane.connection, `${item.id}.plane.connection`, connectionStates),
      cursor: canonicalCursor(plane.cursor, `${item.id}.plane.cursor`),
    },
    project: project
      ? {
          id: uuid(project.id, `${item.id}.project.id`),
          name: text(project.name, `${item.id}.project.name`, 120),
        }
      : null,
    conversation: conversation
      ? {
          id: uuid(conversation.id, `${item.id}.conversation.id`),
          title: text(conversation.title, `${item.id}.conversation.title`, 240),
        }
      : null,
    run: run
      ? {
          id: uuid(run.id, `${item.id}.run.id`),
          lifecycle: text(run.lifecycle, `${item.id}.run.lifecycle`, 32),
          activity: optionalText(run.activity, `${item.id}.run.activity`, 64),
        }
      : null,
    queue,
    timeline,
    capabilities: {
      harnesses: stringArray(capabilities.harnesses, `${item.id}.capabilities.harnesses`, 32),
      missingFeatures: stringArray(
        capabilities.missing_features,
        `${item.id}.capabilities.missing_features`,
        64,
      ),
    },
    notice: notice
      ? {
          tone: oneOf(notice.tone, `${item.id}.notice.tone`, noticeTones),
          title: text(notice.title, `${item.id}.notice.title`, 100),
          message: text(notice.message, `${item.id}.notice.message`, 240),
        }
      : null,
    primaryAction: primaryAction
      ? {
          id: text(primaryAction.id, `${item.id}.primary_action.id`, 96),
          label: text(primaryAction.label, `${item.id}.primary_action.label`, 100),
          availability: oneOf(
            primaryAction.availability,
            `${item.id}.primary_action.availability`,
            actionAvailability,
          ),
          reason: optionalText(primaryAction.reason, `${item.id}.primary_action.reason`, 240),
        }
      : null,
    backendDependencies: stringArray(
      contract.backend_dependencies,
      `${item.id}.contract.backend_dependencies`,
      64,
    ),
  };
}

function loadCorpus(value: unknown): Map<FixtureState, DesktopFixtureScenario> {
  const root = record(value, "root");
  const protocolVersion = record(root.protocol_version, "protocol_version");
  if (
    root.format_version !== 1 ||
    protocolVersion.major !== 1 ||
    !Number.isSafeInteger(protocolVersion.minor) ||
    Number(protocolVersion.minor) < 1
  ) {
    throw new Error("Unsupported desktop fixture contract.");
  }

  const scenarios = array(root.scenarios, "scenarios", fixtureStates.length).map(parseScenario);
  const byState = new Map(scenarios.map((scenario) => [scenario.state, scenario]));
  const identifiers = new Set(scenarios.map((scenario) => scenario.id));
  if (
    scenarios.length !== fixtureStates.length ||
    byState.size !== fixtureStates.length ||
    identifiers.size !== scenarios.length ||
    fixtureStates.some((state) => !byState.has(state))
  ) {
    throw new Error("Desktop fixture state coverage is invalid.");
  }
  return byState;
}

const desktopFixtures = loadCorpus(corpus);

export function fixtureForState(state: FixtureState): DesktopFixtureScenario {
  const scenario = desktopFixtures.get(state);
  if (!scenario) {
    throw new Error(`Desktop fixture state ${state} is missing.`);
  }
  return scenario;
}

export const firstLaunchFixture = fixtureForState("first_launch");
