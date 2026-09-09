# Harness-native extensions

Issue #37 added a backend interface for discovering, inspecting, and changing native extensions on the selected Plane. Jet protocol 1.28 exposes `ExtensionCatalog`, `InspectExtension`, `ChangeExtension`, and `ExtensionChange`. The native metadata stays JSON data. A Craft cannot supply executable GUI customization.

## Review and execution

A client first reads the catalog, then inspects the exact native selector. It displays the selected Plane, Harness, user scope, native source and version information, file hashes, components, and same-user host access. Local sources do not acquire a verified publisher merely because the user selected them. Arbitrary configuration strings are omitted from the preview to keep credentials out of the protocol and durable receipts; users can inspect the referenced native source files. File hashes bind the omitted values to consent.

`ChangeExtension` includes that inspection snapshot, one action (`install`, `update`, `disable`, or `remove`), user scope, and explicit `same_user_executable` trust. Skills can direct native tools, and hooks, MCP servers, and plugins can execute with the Harness user's filesystem, environment, and network access. This grants no Jet broker permissions and supplies no OS containment. The accepted Craft's feature and host-access declarations remain a separate installation boundary.

Core revalidates the snapshot, records a Security audit decision, and persists a staged outbox operation. A Plane admits one unresolved extension change at a time. Existing managed Runs and unresolved orphaned executions block application. New Runs wait for that operation to settle. Changes never request native reload. The worker rechecks Craft disable/revocation and pins the exact executable before invoking it.

Completion records `applied`, `refused`, or `outcome_unknown`, with a second audit record. A process failure, timeout, or interrupted in-flight operation has an unknown outcome and is never automatically retried. Inspect the native state before requesting another change. The barrier covers Jet-managed Runs; another same-user application can independently change native configuration.

## Native sources and selectors

The bundled Crafts report native plugin catalogs plus configured standalone entries. An unavailable plugin CLI does not hide standalone entries. All current actions use the native user scope; project and managed scopes are not writable through this interface.

| Kind | Configured selector | Explicit local source |
| --- | --- | --- |
| Skill directory | `skill:review` | `skill:review@/absolute/checkout/skills/review` |
| MCP server | `mcp:docs` | `mcp:docs@/absolute/native-config` |
| Hook event group | `hook:SessionStart` | `hook:SessionStart@/absolute/hooks-config.json` |
| Codex inline hook group | `hook-config:SessionStart` | `hook-config:SessionStart@/absolute/config.toml` |
| Native plugin | `plugin@marketplace` | A source already configured in the native marketplace |

Explicit sources may be Git checkouts. Inspection records the checkout revision and exact native file hashes, including uncommitted contents. Jet does not fetch an arbitrary Git URL, translate a native package, or operate a marketplace. Installing or updating a standalone entry requires an explicit source, except installing a previously disabled entry restores its saved native content.

Codex reads user skills from `~/.agents/skills`, MCP servers and inline hooks from `$CODEX_HOME/config.toml`, and hook groups from `$CODEX_HOME/hooks.json`. `CODEX_HOME` defaults to `~/.codex`. Claude reads skills from `$CLAUDE_CONFIG_DIR/skills`, MCP servers from `~/.claude.json`, and hooks from `$CLAUDE_CONFIG_DIR/settings.json`; its config directory defaults to `~/.claude`. Setting `CLAUDE_CONFIG_DIR` also relocates its global MCP file to `$CLAUDE_CONFIG_DIR/.claude.json`.

MCP source files retain the Harness's native top-level key (`mcp_servers` in Codex TOML; `mcpServers` in Claude JSON). Hook sources retain `hooks`. The interface supports command-based hooks and the ordinary stdio/HTTP MCP fields validated by the adapter. Unsupported native options are refused before mutation. A hook action addresses one event's matcher groups. Other events, servers, and unrelated settings are preserved; TOML comments are retained.

Disabling standalone extensions preserves their native content outside native discovery: skill directories move to a sibling `.jet-disabled-skills` directory, and configuration entries move to a `.jet-disabled-<filename>` native-format file. Installing a disabled selector restores it. Removal deletes the selected active and disabled content. Native managed-policy enforcement remains with the Harness.

Plugin inspection includes native details and bounded file identities. Install/update is available only when the adapter can inspect the incoming local candidate. Remote entries whose incoming content cannot be bound to review remain discoverable and refuse mutation. Claude Git/URL marketplaces can refresh during native installation, so their cached directories do not qualify as local candidates. Local marketplace overrides are included in review; unresolved package/plugin dependencies are refused. Disable/remove review the currently installed files. Native lifecycle commands remain responsible for plugin installation policy.

## Craft interface

A Craft declaring the optional `extensions` feature accepts one JSON request on stdin with `--extensions-v1`, then writes one JSON reply on stdout. Requests are `catalog`, `inspect`, and `apply`; replies are `catalog`, `applied`, and `refused`. The versioned endpoint does not create a Conversation or Run. Unknown actions, scopes, and trust values are rejected by the strict wire decoder.

The daemon bounds request/reply bytes and supervises the whole process group. Native diagnostics are not returned to clients. Catalog/inspection metadata is limited to 64 KiB. Native file inventories reject symlinks and special files and cap depth, file count, and total content. Native configuration inputs and resulting documents are each limited to 64 KiB. Each mutation revalidates the reviewed source and target; staged skill bytes are compared with the confirmed hashes before publication.

## Review boundaries

The diff spans generated contracts and their exhaustive client translations as well as implementation. The smallest coherent infrastructure stage is the public Command/Query contract, durable store/outbox state, audit integration, and Run admission barrier, tested with a host at the public Core seam. The next review stage is the daemon's supervised, pinned Craft endpoint. The final stage is the two native adapters and their shared bounded file/configuration helpers, exercised through actual Craft subprocesses. These dependencies are delivered together so the new public endpoint is usable and the protocol, clients, and persistence remain in sync.

Native references: [Claude configuration locations](https://code.claude.com/docs/en/agent-sdk/claude-code-features), [Claude marketplace sources and dependencies](https://code.claude.com/docs/en/plugin-marketplaces), [Claude install refresh behavior](https://code.claude.com/docs/en/discover-plugins). Codex plugin RPCs were checked against the installed app-server generated v2 schema.
