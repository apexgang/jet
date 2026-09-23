use std::time::Duration;

use jet_protocol::{
    ClientMessage, CommandRequest, CommandResponse, ExtensionChange, QueryRequest, QueryResponse,
    ServerMessage, StreamId,
};
use serde_json::json;
use tokio::net::{
    unix::{OwnedReadHalf, OwnedWriteHalf},
    UnixListener,
};

use super::*;
use crate::jet::{
    client::{
        unit_tests::{accept, next_message, reply, request_id},
        PlaneClient,
    },
    enrollment::tests::{setup, Setup},
    planes::PlaneBinding,
    settings::{apply_settings_change_for, SettingsReceipt},
};

type Reader = jet_protocol::FrameReader<OwnedReadHalf>;
type Writer = jet_protocol::FrameWriter<OwnedWriteHalf>;

const CLIENT: Uuid = Uuid::from_u128(0x5e77);

fn local() -> PlaneKey {
    PlaneBinding {
        plane: PlaneId::Local,
        identity: None,
    }
}

fn remote(n: u128) -> PlaneKey {
    PlaneBinding {
        plane: PlaneId::Remote(Uuid::from_u128(n)),
        identity: None,
    }
}

// ---------------------------------------------------------------------------
// Real-shape fixtures (bundled Craft catalogs, wave 3.2 §4.8)
// ---------------------------------------------------------------------------

/// `jet-craft-sdk` standalone inventory as the bundled Crafts wrap it.
fn standalone_inventory(home: &str) -> serde_json::Value {
    json!({
        "entries": [
            {"id": "hook:pre-commit", "path": format!("{home}/.codex/hooks.json"), "native_key": "hooks", "enabled": true},
            {"id": "mcp:github", "path": format!("{home}/.codex/.jet-disabled-config.toml"), "native_key": "mcp_servers", "enabled": false},
            {"id": "skill:a", "path": format!("{home}/.agents/skills/a"), "enabled": true},
        ],
        "scope": "user",
        "explicit_sources": "kind:name@/absolute/native/path (including Git checkouts)",
        "permissions": {"same_user_execution": true, "jet_broker": [], "executable_gui": false,
            "executables": "native extension commands", "filesystem": "all paths accessible to the Harness user",
            "environment": "Harness process environment", "network": "destinations reachable by the Harness user"}
    })
}

/// Codex: app-server `plugin/list` under `plugins`.
fn codex_catalog() -> String {
    json!({
        "plugins": {"marketplaces": [
            {"name": "openai-curated", "path": "/home/u/.codex/marketplaces/curated", "plugins": [
                {"id": "linear@openai-curated", "name": "linear", "installed": true, "enabled": true},
                {"id": "figma@openai-curated", "name": "figma", "installed": false}
            ]},
            {"name": "remote", "plugins": [{"id": "notion@remote", "name": "notion"}]}
        ]},
        "standalone": standalone_inventory("/home/u"),
    })
    .to_string()
}

/// Claude: `claude plugin list --available --json` under `plugins`.
fn claude_catalog() -> String {
    json!({
        "plugins": {
            "installed": [
                {"id": "formatter@team", "scope": "user", "installPath": "/home/u/.claude/plugins/cache/formatter", "version": "1.0.0"},
                {"id": "project-only@team", "scope": "project", "installPath": "/work/.claude/plugins/p"}
            ],
            "available": [
                {"name": "reviewer", "marketplace": "team", "description": "Reviews"},
                {"id": "formatter@team", "scope": "user"}
            ]
        },
        "standalone": standalone_inventory("/home/u"),
    })
    .to_string()
}

fn unavailable_plugins() -> String {
    json!({"plugins": {"plugin_catalog_unavailable": true}, "standalone": standalone_inventory("/home/u")})
        .to_string()
}

fn ids(projection: &Projection, kind: EntryKind) -> Vec<(String, Option<bool>)> {
    projection
        .entries
        .iter()
        .filter(|entry| entry.kind == kind)
        .map(|entry| (entry.id.clone(), entry.state))
        .collect()
}

#[test]
fn codex_catalogs_project_identifiers_and_never_paths() {
    let projection = project_catalog(&codex_catalog());
    assert!(!projection.unreadable && !projection.plugins_unavailable && !projection.truncated);
    assert_eq!(
        ids(&projection, EntryKind::Standalone),
        [
            ("hook:pre-commit".to_owned(), Some(true)),
            ("mcp:github".to_owned(), Some(false)),
            ("skill:a".to_owned(), Some(true)),
        ]
    );
    assert_eq!(
        ids(&projection, EntryKind::Plugin),
        [
            ("linear@openai-curated".to_owned(), Some(true)),
            ("figma@openai-curated".to_owned(), Some(false)),
            ("notion@remote".to_owned(), None),
        ]
    );
}

#[test]
fn claude_catalogs_keep_user_scope_plugins_and_where_they_were_listed() {
    let projection = project_catalog(&claude_catalog());
    assert!(!projection.unreadable);
    assert_eq!(
        ids(&projection, EntryKind::Plugin),
        [
            ("formatter@team".to_owned(), Some(true)),
            // Claude's name@marketplace selector when no id is given.
            ("reviewer@team".to_owned(), Some(false)),
        ]
    );
    // A plain array says nothing about installation.
    let array = json!({"plugins": [{"id": "tool@official", "version": "1"}]}).to_string();
    assert_eq!(
        ids(&project_catalog(&array), EntryKind::Plugin),
        [("tool@official".to_owned(), None)]
    );
}

#[test]
fn an_unavailable_plugin_list_still_shows_standalone_entries() {
    let projection = project_catalog(&unavailable_plugins());
    assert!(projection.plugins_unavailable);
    assert!(!projection.unreadable);
    assert!(ids(&projection, EntryKind::Plugin).is_empty());
    assert_eq!(ids(&projection, EntryKind::Standalone).len(), 3);
}

#[test]
fn unrecognized_or_oversized_catalogs_are_reported_not_guessed() {
    for metadata in [
        "not json".to_owned(),
        "[]".to_owned(),
        json!({"other": 1}).to_string(),
        json!({"plugins": "all"}).to_string(),
        json!({"plugins": {"marketplaces": 3}}).to_string(),
        json!({"standalone": {"items": []}}).to_string(),
        "x".repeat(MAX_NATIVE_METADATA_BYTES + 1),
    ] {
        assert!(project_catalog(&metadata).unreadable, "{metadata:.40}");
    }
    // A malformed entry is skipped and reported; the rest still shows.
    let partial = json!({"standalone": {"entries": [
        {"id": "skill:ok", "enabled": true},
        {"id": "skill:bell\u{7}", "enabled": true},
        {"id": "skill:no-state"},
        {"id": "x".repeat(MAX_STANDALONE_ID_BYTES + 1), "enabled": true},
    ]}, "plugins": [{"id": "p".repeat(MAX_PLUGIN_ID_BYTES + 1)}, {"id": 4}]})
    .to_string();
    let projection = project_catalog(&partial);
    assert!(projection.unreadable);
    assert_eq!(
        ids(&projection, EntryKind::Standalone),
        [("skill:ok".to_owned(), Some(true))]
    );
    assert!(ids(&projection, EntryKind::Plugin).is_empty());
}

#[test]
fn catalogs_are_capped_and_say_so() {
    let entries: Vec<_> = (0..MAX_STANDALONE + 3)
        .map(|n| json!({"id": format!("skill:s{n}"), "enabled": true}))
        .collect();
    let plugins: Vec<_> = (0..MAX_PLUGINS + 1)
        .map(|n| json!({"id": format!("p{n}@m")}))
        .collect();
    let metadata = json!({"standalone": {"entries": entries}, "plugins": plugins}).to_string();
    let projection = project_catalog(&metadata);
    assert!(projection.truncated);
    assert_eq!(
        ids(&projection, EntryKind::Standalone).len(),
        MAX_STANDALONE
    );
    assert_eq!(ids(&projection, EntryKind::Plugin).len(), MAX_PLUGINS);
}

#[test]
fn the_catalog_view_carries_tokens_and_no_native_path() {
    let Setup { bridge, .. } = setup();
    for (metadata, harness, name) in [
        (codex_catalog(), "codex", "Codex"),
        (claude_catalog(), "claude-code", "Claude Code"),
    ] {
        let catalog = ExtensionCatalog {
            craft_id: harness.into(),
            harness: harness.into(),
            native_metadata: metadata,
        };
        let view = catalog_view(&bridge, local(), harness, &catalog, Instant::now()).unwrap();
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["harness"], name);
        assert_eq!(json["standalone"][0]["id"], "hook:pre-commit");
        assert_eq!(json["standalone"][1]["enabled"], false);
        assert!(Uuid::parse_str(json["standalone"][0]["entryToken"].as_str().unwrap()).is_ok());
        let text = json.to_string();
        for native in [
            "/home/u",
            "native_key",
            "explicit_sources",
            "permissions",
            "installPath",
            "marketplaces",
        ] {
            assert!(!text.contains(native), "{native} crossed: {text}");
        }
        assert_eq!(json["issues"], json!([]));
    }

    let catalog = ExtensionCatalog {
        craft_id: "codex".into(),
        harness: "codex".into(),
        native_metadata: unavailable_plugins(),
    };
    let view = serde_json::to_value(
        catalog_view(&bridge, local(), "codex", &catalog, Instant::now()).unwrap(),
    )
    .unwrap();
    assert_eq!(view["plugins"], json!([]));
    assert_eq!(view["standalone"].as_array().unwrap().len(), 3);
    assert_eq!(view["issues"][0]["section"], "plugins");
    assert_eq!(
        view["issues"][0]["error"]["code"],
        "extensions.plugin_catalog_unavailable"
    );
}

// ---------------------------------------------------------------------------
// Inspection projection
// ---------------------------------------------------------------------------

fn file(path: &str) -> serde_json::Value {
    json!({"path": path, "sha256": "a".repeat(64)})
}

/// `standalone_extensions.rs` inspection of a skill.
fn standalone_inspection(files: serde_json::Value) -> String {
    json!({
        "extension_id": "skill:a", "source": "/home/u/.agents/skills/a",
        "publisher": "user-selected native source", "version": null, "files": files,
        "components": {"skills": ["a"]}, "destination": "/home/u/.agents/skills/a",
        "current": {"installed": null, "disabled": null}, "disabled": false,
        "permissions": {"same_user_execution": true}, "scope": "user", "trust": "requires_same_user_consent"
    })
    .to_string()
}

#[test]
fn standalone_inspections_show_their_files_and_are_reviewable_only_with_files() {
    let inspected = project_inspection(&standalone_inspection(json!([
        file("SKILL.md"),
        file("run.sh")
    ])))
    .unwrap();
    assert!(inspected.reviewable);
    assert_eq!(inspected.disabled, Some(false));
    assert_eq!(inspected.supported_actions, None);
    assert_eq!(inspected.facts.file_count, 2);
    assert_eq!(
        inspected.facts.source.as_deref(),
        Some("/home/u/.agents/skills/a")
    );
    assert_eq!(
        inspected.facts.publisher.as_deref(),
        Some("user-selected native source")
    );
    assert_eq!(inspected.facts.version, None);

    // No files: discoverable, never confirmable (extension_review_complete).
    for files in [json!([]), json!(null)] {
        let inspected = project_inspection(&standalone_inspection(files)).unwrap();
        assert!(!inspected.reviewable);
        assert_eq!(inspected.facts.file_count, 0);
    }
    // A file that can't be shown exactly makes the change unconfirmable.
    let hidden = project_inspection(&standalone_inspection(json!([
        file("ok"),
        {"path": "bell\u{7}", "sha256": "a".repeat(64)}
    ])))
    .unwrap();
    assert!(!hidden.reviewable);
    assert_eq!(hidden.facts.files.len(), 1);
    assert_eq!(hidden.facts.file_count, 2);
    let many: Vec<_> = (0..MAX_FILES + 1).map(|n| file(&format!("f{n}"))).collect();
    let many = project_inspection(&standalone_inspection(json!(many))).unwrap();
    assert!(!many.reviewable);
    assert_eq!(many.facts.files.len(), MAX_FILES);
    assert_eq!(many.facts.file_count, (MAX_FILES + 1) as u32);
}

#[test]
fn claude_plugin_inspections_name_their_supported_actions_and_candidate_facts() {
    let metadata = json!({
        "installed_files": [file("plugin.json")],
        "candidate": {"source": {"source": "directory", "path": "/m"}, "path": "/m/plugins/reviewer",
            "version": "2.1.0", "publisher": {"name": "Team", "email": "team@example.com"},
            "files": [file("plugin.json")]},
        "files": [file("plugin.json")],
        "extension_id": "reviewer@team",
        "summary": {"name": "reviewer"},
        "permissions": {"same_user_execution": true},
        "supported_actions": ["install", "update", "disable", "remove", "launch", "install"],
    })
    .to_string();
    let inspected = project_inspection(&metadata).unwrap();
    assert!(inspected.reviewable);
    assert_eq!(
        inspected.supported_actions,
        Some(vec!["install", "update", "disable", "remove"])
    );
    assert_eq!(inspected.facts.version.as_deref(), Some("2.1.0"));
    assert_eq!(inspected.facts.publisher.as_deref(), Some("Team"));
    assert_eq!(
        inspected.facts.source.as_deref(),
        Some("/m/plugins/reviewer")
    );

    // Codex: files may be null when the plugin has no local source.
    let codex = json!({"extension_id": "notion@remote", "summary": {}, "details": {"plugin": {}},
        "files": null, "permissions": {}})
    .to_string();
    assert!(!project_inspection(&codex).unwrap().reviewable);
    assert!(project_inspection("[]").is_none());
}

// ---------------------------------------------------------------------------
// Tokens and Plane binding
// ---------------------------------------------------------------------------

#[test]
fn entry_tokens_resolve_only_under_their_plane_and_expire() {
    let state = ExtensionsState::default();
    let now = Instant::now();
    let tokens = state
        .grant_catalog(remote(0xa), "codex", ["skill:a".to_owned()], now)
        .unwrap();
    let token = tokens[0];
    assert_eq!(
        state
            .entry(token, PlaneId::Remote(Uuid::from_u128(0xb)), now)
            .unwrap_err()
            .code,
        "extensions.inspection_expired"
    );
    assert_eq!(
        state.entry(token, PlaneId::Local, now).unwrap_err().code,
        "extensions.inspection_expired"
    );
    let entry = state
        .entry(token, PlaneId::Remote(Uuid::from_u128(0xa)), now)
        .unwrap();
    assert_eq!(entry.extension_id, "skill:a");
    assert_eq!(entry.craft_id, "codex");
    assert!(state
        .entry(
            token,
            PlaneId::Remote(Uuid::from_u128(0xa)),
            now + GRANT_LIFETIME
        )
        .is_err());

    // Reloading the same Craft replaces its tokens; other catalogs stay.
    let other = state
        .grant_catalog(remote(0xa), "claude-code", ["skill:b".to_owned()], now)
        .unwrap()[0];
    let fresh = state
        .grant_catalog(remote(0xa), "codex", ["skill:a".to_owned()], now)
        .unwrap()[0];
    let plane = PlaneId::Remote(Uuid::from_u128(0xa));
    assert!(state.entry(token, plane, now).is_err());
    assert!(state.entry(fresh, plane, now).is_ok());
    assert!(state.entry(other, plane, now).is_ok());

    // At most eight catalogs are kept; the oldest goes first.
    for n in 0..CATALOG_CAPACITY as u64 {
        state
            .grant_catalog(
                remote(0xa),
                &format!("craft-{n}"),
                ["x".to_owned()],
                now + Duration::from_millis(n + 1),
            )
            .unwrap();
    }
    assert_eq!(state.catalogs.lock().unwrap().len(), CATALOG_CAPACITY);
    assert!(state.entry(other, plane, now).is_err());
}

fn inspection(plane: PlaneKey, files: serde_json::Value) -> InspectionGrant {
    let metadata = standalone_inspection(files);
    let inspected = project_inspection(&metadata).unwrap();
    InspectionGrant {
        plane,
        extension_id: "skill:a".into(),
        catalog: ExtensionCatalog {
            craft_id: "codex".into(),
            harness: "codex".into(),
            native_metadata: metadata,
        },
        harness: "Codex".into(),
        facts: inspected.facts,
        reviewable: inspected.reviewable,
        created_at: Instant::now(),
    }
}

#[test]
fn a_change_is_prepared_only_from_a_reviewable_inspection_of_the_same_plane() {
    let Setup { bridge, .. } = setup();
    let now = Instant::now();
    let reviewable = bridge
        .extensions
        .grant_inspection(inspection(local(), json!([file("SKILL.md")])), now)
        .unwrap();
    let empty = bridge
        .extensions
        .grant_inspection(inspection(local(), json!([])), now)
        .unwrap();
    let remote_id = Uuid::from_u128(0xa).to_string();
    let code = |result: Result<SettingsReviewView, PublicError>| result.unwrap_err().code;

    assert_eq!(
        code(prepare_extension_change_for(
            &bridge,
            "local",
            &empty.to_string(),
            "remove"
        )),
        "extensions.not_reviewable"
    );
    assert_eq!(
        code(prepare_extension_change_for(
            &bridge,
            &remote_id,
            &reviewable.to_string(),
            "remove"
        )),
        "extensions.inspection_expired"
    );
    assert_eq!(
        code(prepare_extension_change_for(
            &bridge,
            "local",
            "not-a-uuid",
            "remove"
        )),
        "extensions.inspection_expired"
    );
    assert_eq!(
        code(prepare_extension_change_for(
            &bridge,
            "local",
            &reviewable.to_string(),
            "launch"
        )),
        "extensions.action_invalid"
    );
    assert!(bridge.settings.reviews_for_test().is_empty());

    let review =
        prepare_extension_change_for(&bridge, "local", &reviewable.to_string(), "disable").unwrap();
    let review = serde_json::to_value(review).unwrap();
    assert_eq!(review["subject"]["kind"], "change_extension");
    let preview = &review["subject"]["preview"];
    assert_eq!(preview["extensionId"], "skill:a");
    assert_eq!(preview["action"], "disable");
    assert_eq!(preview["harness"], "Codex");
    assert_eq!(preview["files"][0]["path"], "SKILL.md");
    // The confirmation is native: the exact inspection, user scope and trust.
    let reviews = bridge.settings.reviews_for_test();
    let [SettingsAction::ChangeExtension { confirmation }] = reviews.as_slice() else {
        panic!("expected one extension review, got {reviews:?}");
    };
    assert_eq!(confirmation.extension_id, "skill:a");
    assert_eq!(confirmation.action, ExtensionAction::Disable);
    assert_eq!(confirmation.scope, ExtensionScope::User);
    assert_eq!(confirmation.trust, ExtensionTrust::SameUserExecutable);
    assert_eq!(
        confirmation.catalog.native_metadata,
        standalone_inspection(json!([file("SKILL.md")]))
    );
}

#[tokio::test]
async fn inputs_are_refused_without_io() {
    // The local socket does not exist: any I/O would be offline.
    let Setup { bridge, .. } = setup();
    assert_eq!(
        load_extension_catalog_for(&bridge, "local", "../codex")
            .await
            .unwrap_err()
            .code,
        "agents.craft_missing"
    );
    assert_eq!(
        load_extension_catalog_for(&bridge, "nonsense", "codex")
            .await
            .unwrap_err()
            .code,
        "plane.unknown"
    );
    assert_eq!(
        inspect_extension_for(&bridge, "local", &Uuid::from_u128(3).to_string())
            .await
            .unwrap_err()
            .code,
        "extensions.inspection_expired"
    );
    assert_eq!(
        load_extension_change_for(&bridge, "local", &Uuid::from_u128(3).to_string())
            .await
            .unwrap_err()
            .code,
        "extensions.change_unknown"
    );
}

#[test]
fn queued_changes_are_listed_per_plane_and_craft_until_finished() {
    let state = ExtensionsState::default();
    let confirmation = |id: &str| ExtensionConfirmation {
        catalog: ExtensionCatalog {
            craft_id: "codex".into(),
            harness: "codex".into(),
            native_metadata: "{}".into(),
        },
        extension_id: id.into(),
        action: ExtensionAction::Remove,
        scope: ExtensionScope::User,
        trust: ExtensionTrust::SameUserExecutable,
    };
    let first = Uuid::from_u128(1);
    state.record_change(local(), first, &confirmation("skill:a"));
    state.record_change(local(), first, &confirmation("skill:a"));
    state.record_change(remote(0xa), Uuid::from_u128(2), &confirmation("skill:b"));
    let pending = state.pending(PlaneId::Local, "codex");
    assert_eq!(
        serde_json::to_value(&pending).unwrap(),
        json!([{"changeId": first.to_string(), "extensionId": "skill:a", "action": "remove"}])
    );
    assert!(state.pending(PlaneId::Local, "claude-code").is_empty());
    assert_eq!(
        state
            .change(Uuid::from_u128(2), PlaneId::Local)
            .unwrap_err()
            .code,
        "extensions.change_unknown"
    );
    state.finish(first);
    assert!(state.pending(PlaneId::Local, "codex").is_empty());
    // A finished change can still be read.
    assert!(state.change(first, PlaneId::Local).is_ok());

    for n in 0..CHANGE_CAPACITY as u128 {
        state.record_change(local(), Uuid::from_u128(100 + n), &confirmation("skill:c"));
    }
    let changes = state.changes.lock().unwrap();
    assert_eq!(changes.len(), CHANGE_CAPACITY);
    // The finished change was evicted first.
    assert!(!changes.iter().any(|change| change.change_id == first));
}

// ---------------------------------------------------------------------------
// Fake jetd: catalog → inspect → review → change → status
// ---------------------------------------------------------------------------

struct Fake {
    setup: Setup,
    plane_id: String,
    listener: UnixListener,
}

fn fake_plane() -> Fake {
    let setup = setup();
    let socket = setup.directory.path().join("fake-jetd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let id = Uuid::from_u128(0xfa4e);
    setup.bridge.planes.insert_for_test(
        id,
        "Fake Plane",
        PlaneClient::new(
            socket,
            CLIENT,
            [Duration::from_millis(1)],
            Duration::from_millis(1),
        ),
    );
    Fake {
        setup,
        plane_id: id.to_string(),
        listener,
    }
}

async fn answer(
    writer: &mut Writer,
    stream: StreamId,
    message: &ClientMessage,
    result: QueryResponse,
) {
    reply(
        writer,
        stream,
        ServerMessage::QueryResult {
            id: request_id(message),
            result,
        },
    )
    .await;
}

async fn assert_closed(reader: &mut Reader) {
    assert!(reader.read().await.is_err(), "no further request expected");
}

#[tokio::test]
async fn an_extension_change_is_inspected_by_token_reviewed_and_sent_under_the_review_id() {
    let fake = fake_plane();
    let bridge = &fake.setup.bridge;
    let inspected_metadata = standalone_inspection(json!([file("SKILL.md")]));

    // 1. The catalog: one connection, one ExtensionCatalog query.
    let (view, ()) = tokio::join!(
        load_extension_catalog_for(bridge, &fake.plane_id, "codex"),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = next_message(&mut reader).await;
            assert!(matches!(
                &message,
                ClientMessage::Query { query: QueryRequest::ExtensionCatalog { craft_id }, .. } if craft_id == "codex"
            ));
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::ExtensionCatalog(ExtensionCatalog {
                    craft_id: "codex".into(),
                    harness: "codex".into(),
                    native_metadata: codex_catalog(),
                }),
            )
            .await;
        }
    );
    let view = serde_json::to_value(view.unwrap()).unwrap();
    let token = view["standalone"][2]["entryToken"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(view["standalone"][2]["id"], "skill:a");

    // Another Plane can't use the token; nothing is sent anywhere.
    assert_eq!(
        inspect_extension_for(bridge, "local", &token)
            .await
            .unwrap_err()
            .code,
        "extensions.inspection_expired"
    );

    // 2. Inspection by token: the identifier comes from the grant.
    let (inspection, ()) = tokio::join!(
        inspect_extension_for(bridge, &fake.plane_id, &token),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = next_message(&mut reader).await;
            assert!(matches!(
                &message,
                ClientMessage::Query {
                    query: QueryRequest::InspectExtension { craft_id, extension_id },
                    ..
                } if craft_id == "codex" && extension_id == "skill:a"
            ));
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::ExtensionCatalog(ExtensionCatalog {
                    craft_id: "codex".into(),
                    harness: "codex".into(),
                    native_metadata: inspected_metadata.clone(),
                }),
            )
            .await;
        }
    );
    let inspection = serde_json::to_value(inspection.unwrap()).unwrap();
    assert_eq!(inspection["reviewable"], true);
    assert_eq!(inspection["extensionId"], "skill:a");
    assert_eq!(inspection["fileCount"], 1);
    let inspection_id = inspection["inspectionId"].as_str().unwrap();

    // 3. The review needs no I/O.
    let review =
        prepare_extension_change_for(bridge, &fake.plane_id, inspection_id, "remove").unwrap();
    let review = serde_json::to_value(review).unwrap();
    let review_id = review["reviewId"].as_str().unwrap().to_owned();
    let command_id = Uuid::parse_str(&review_id).unwrap();
    let change_id = Uuid::from_u128(0xc4);

    // 4. Apply sends exactly the inspection under the review ID.
    let (receipt, ()) = tokio::join!(
        apply_settings_change_for(bridge, &fake.plane_id, &review_id),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = next_message(&mut reader).await;
            match &message {
                ClientMessage::Command {
                    command_id: sent,
                    command: CommandRequest::ChangeExtension { confirmation },
                    ..
                } if *sent == command_id
                    && confirmation.extension_id == "skill:a"
                    && confirmation.action == ExtensionAction::Remove
                    && confirmation.catalog.native_metadata == inspected_metadata => {}
                other => panic!("unexpected request {other:?}"),
            }
            reply(
                &mut writer,
                stream,
                ServerMessage::CommandResult {
                    id: request_id(&message),
                    result: CommandResponse::ExtensionChangeQueued { change_id },
                },
            )
            .await;
            assert_closed(&mut reader).await;
        }
    );
    let SettingsReceipt::Applied { detail } = receipt.unwrap() else {
        panic!("expected an applied receipt");
    };
    assert_eq!(
        serde_json::to_value(detail).unwrap(),
        json!({"kind": "extension_change_queued", "changeId": change_id.to_string()})
    );
    assert_eq!(
        bridge
            .extensions
            .pending(PlaneId::Remote(Uuid::from_u128(0xfa4e)), "codex")
            .len(),
        1
    );

    // 5. Status: staged, then applied (which finishes it).
    let asked = change_id.to_string();
    for (state, expected) in [
        (ExtensionChangeState::Staged, "staged"),
        (ExtensionChangeState::Applied, "applied"),
    ] {
        let (status, ()) = tokio::join!(
            load_extension_change_for(bridge, &fake.plane_id, &asked),
            async {
                let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
                let (stream, message) = next_message(&mut reader).await;
                assert!(matches!(
                    &message,
                    ClientMessage::Query { query: QueryRequest::ExtensionChange { change_id: asked }, .. } if *asked == change_id
                ));
                answer(
                    &mut writer,
                    stream,
                    &message,
                    QueryResponse::ExtensionChange(ExtensionChange {
                        change_id,
                        craft_id: "codex".into(),
                        extension_id: "skill:a".into(),
                        action: ExtensionAction::Remove,
                        state,
                    }),
                )
                .await;
            }
        );
        let status = serde_json::to_value(status.unwrap()).unwrap();
        assert_eq!(status["state"], expected);
        assert_eq!(status["action"], "remove");
    }
    assert!(bridge
        .extensions
        .pending(PlaneId::Remote(Uuid::from_u128(0xfa4e)), "codex")
        .is_empty());
    // A change ID is read only through the Plane it was queued on.
    assert_eq!(
        load_extension_change_for(bridge, "local", &change_id.to_string())
            .await
            .unwrap_err()
            .code,
        "extensions.change_unknown"
    );
}

#[tokio::test]
async fn a_catalog_of_another_craft_is_refused() {
    let fake = fake_plane();
    let (view, ()) = tokio::join!(
        load_extension_catalog_for(&fake.setup.bridge, &fake.plane_id, "codex"),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message) = next_message(&mut reader).await;
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::ExtensionCatalog(ExtensionCatalog {
                    craft_id: "claude-code".into(),
                    harness: "claude-code".into(),
                    native_metadata: claude_catalog(),
                }),
            )
            .await;
        }
    );
    assert_eq!(view.unwrap_err().category, "internal");
}
