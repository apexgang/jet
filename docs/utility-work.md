# Utility work

Issue #38 implements ADR-0099 through a durable `RequestUtility` Command and a
`Utility` Query (Jet protocol 1.22). The Command returns a job UUID immediately.
The daemon processes up to 32 outstanding jobs, one at a time on a separate
worker. Each attempt has a 30-second deadline. A restart settles an interrupted
attempt without sending another inference request. Replaying the Command
returns the same job, including after restart.

The result records the Plane, purpose, selected Provider and Account binding,
exact Model, and versioned policy. Missing selections are explicit nulls.
Unavailable work never selects another account, Provider, or Plane. The Model
is committed before inference. Results are suggestions: naming and Git text
have no automatic write, commit, or pull-request authority. Autodelete results
are unapproved drafts and have no deletion authority.

## Policy

Configure these through the existing authenticated Settings Commands:

| Setting | Value | Default |
| --- | --- | --- |
| `utility.account_binding` | Account binding UUID, or empty text | Empty |
| `utility.content_consent` | The exact binding UUID explicitly authorized to receive Conversation/diff content, or empty text | Empty |
| `utility.automatic_naming` | Flag; existing Plane/Project/Conversation inheritance | Enabled |
| `utility.git_text` | Plane flag, independent of automatic committing | Disabled |
| `utility.autodelete_compilation` | Plane flag | Disabled |

The binding and disclosure consent are Plane-wide and cannot be overridden by a
Project or Conversation. Changing the Utility binding does not transfer consent.
Setting or clearing these new policy keys produces a Security audit decision.
Both policy and binding availability are checked before selection and again
immediately before inference. A policy change before dispatch produces a local
fallback or refusal.

Before enabling `utility.content_consent`, disclose that the selected Provider
will receive bounded Conversation opening input and checkpoint patches for the
enabled purposes. Current Run records do not establish an authoritative
originating Provider, so naming and Git requests conservatively require this
persistent consent for every selected Provider. Autodelete receives only the
submitted rule prompt and does not require Conversation-content consent.

## Inputs and outputs

| Purpose | Input allowlist, in UTF-8 bytes | Accepted output | Unavailable/invalid output |
| --- | --- | --- | --- |
| `naming` | Run identity resolves title ≤256 and initial prompt ≤4096 | `{"text":"…"}`, 1–256 bytes, no controls or surrounding whitespace | Existing Run name, or a stable Run identifier if the source disappeared |
| `git_text` | Run identity and turn resolve an immutable checkpoint patch ≤16384, plus Plane Git message instructions ≤2048 | `{"subject":"…","body":"…"}`, subject 1–256 bytes on one line; body ≤4096 with only newline controls | `Update Run changes (turn N)`, empty body |
| `autodelete` | Nonempty rule prompt ≤4096 | `{"inactive_days":N}`, whole days 1–36500 | Refusal; no rule or deletion is authorized |

Text inputs truncate at UTF-8 boundaries. Git requests use a retained checkpoint,
never the live checkout. Missing or metadata-only checkpoints use the fallback.
No credentials, diagnostic streams, or raw terminal output are added to model
input. The entire reply is limited to 8192 bytes; the core rejects unknown
fields, duplicate fields, malformed JSON, invalid values, and a reported Model
that differs from the selection. It never decodes model output as a Command.

The first Autodelete draft schema supports only an inactivity-day predicate.
The Craft requests null for ambiguous prompts, additional conditions, or other
predicates; the core refuses null. A well-formed draft still requires separate
review and approval under ADR-0015.

## Bundled Crafts

An accepted `claude-code` installation handles `anthropic`; an accepted `codex`
installation handles `openai`. Its accepted declaration must disclose the
optional `utility` feature, the `curl` executable, and the Provider endpoint.
Older accepted installations remain unavailable for Utility work until their
updated declaration and executable digest are accepted through installation.

Utility uses an isolated, versioned stdin/stdout exchange:

- `--utility-model` returns `CraftUtilityModel` without receiving user content.
- `--utility` accepts one `CraftUtilityRequest` (≤128 KiB including escaping)
  and returns one `CraftUtilityReply` (≤64 KiB including escaping).

These modes never launch Claude Code or Codex, connect to a native session, or
receive a broker, workspace root, or tool definitions. The daemon checks the
accepted executable digest before selection and execution. Process-group
supervision cleans up the Craft, authentication helper, and transport on
completion or cancellation.

The Craft uses one fixed HTTPS request through curl, with no redirects, retries,
proxy, curlrc, or diagnostic output. Authentication and JSON travel over stdin,
not arguments or temporary files. HTTP responses are limited to 64 KiB and 20
seconds. Provider refusal, truncation, non-text/tool output, and substituted
Models fail closed.

Credential resolution uses exactly the selected reference:

- Platform-store references read the binding's own service/account item through
  `security` on macOS or `secret-tool` on Linux.
- An explicitly configured external helper is invoked as
  `HELPER jet-utility-api-key PROVIDER BINDING_UUID`; stdout must contain only
  the API key (≤4096 bytes, with an optional final newline).
- Harness-native references use only `OPENAI_API_KEY` or `ANTHROPIC_API_KEY`
  from the Craft environment. A subscription/OAuth-only native login does not
  supply this API credential; Utility is unavailable in that case.
- Session-only references have no Utility resolver and fail closed.

A failed credential source never falls back to another source. The key is
transport authentication only and never enters a Utility document or prompt.

The bundled model choices were checked against provider documentation:
[GPT-5.4 nano](https://developers.openai.com/api/docs/models/gpt-5.4-nano)
uses `gpt-5.4-nano-2026-03-17` with reasoning `none`, and
[Claude Haiku 4.5](https://platform.claude.com/docs/en/models/haiku-4-5/migration-guide)
uses `claude-haiku-4-5-20251001` with thinking disabled. Both request strict
structured output using the respective
[OpenAI](https://developers.openai.com/api/docs/guides/structured-outputs) and
[Anthropic](https://platform.claude.com/docs/en/build-with-claude/structured-outputs)
formats. Updating a model choice requires a new accepted Craft artifact.

## Validation and review stages

The implementation separates durable core admission/policy/validation, the
isolated Craft transport, and daemon/client/wire integration. Contract and GUI
model regeneration is mechanical. Tests at the core interface use real SQLite
state and fake inference, including restart ambiguity and consent revocation.
Subprocess tests use real bundled Crafts with a fake external transport. A
real-daemon test exercises accepted installation, admission, worker dispatch,
query, and receipt replay after restart. Tests make no paid inference requests.
