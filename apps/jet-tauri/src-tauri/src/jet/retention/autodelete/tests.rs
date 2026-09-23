use std::time::Duration;

use jet_protocol::{
    AutodeleteRulePreview, ClientMessage, CommandRequest, CommandResponse, ErrorCategory,
    QueryRequest, QueryResponse, ResolvedSetting, RetentionProtection, ServerMessage, SettingScope,
    SettingSelection, SettingSnapshot, SettingSource, StreamId, UtilityJob, UtilityPolicy,
    WireError,
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
};

type Reader = jet_protocol::FrameReader<OwnedReadHalf>;
type Writer = jet_protocol::FrameWriter<OwnedWriteHalf>;

const CLIENT: Uuid = Uuid::from_u128(0xad1);
const RULE: Uuid = Uuid::from_u128(0xad2);
const JOB: Uuid = Uuid::from_u128(0xad3);

fn rule(state: AutodeleteRuleState, updated_at: i64) -> AutodeleteRule {
    AutodeleteRule {
        rule_id: RULE,
        prompt: "Old scratch tasks".into(),
        utility_job_id: JOB,
        state,
        scope: AutodeleteScope::Forget,
        created_at_unix_ms: 1,
        updated_at_unix_ms: updated_at,
    }
}

fn draft(days: u32) -> AutodeleteRuleState {
    AutodeleteRuleState::Draft {
        inactive_days: days,
    }
}

fn local() -> PlaneBinding {
    PlaneBinding {
        plane: PlaneId::Local,
        identity: None,
    }
}

// ---------------------------------------------------------------------------
// Pure validation, tokens and views
// ---------------------------------------------------------------------------

#[test]
fn prompts_are_1_to_4096_bytes_of_text_and_days_are_bounded() {
    let compile = |prompt: String| {
        parse_change(AutodeleteChange::Compile {
            rule_id: None,
            prompt,
        })
    };
    assert_eq!(
        compile(String::new()).unwrap_err().code,
        "autodelete.prompt_invalid"
    );
    assert!(compile("a".into()).is_ok());
    assert!(compile("a".repeat(4096)).is_ok());
    // Bytes, not characters: 1366 three-byte characters are 4098 bytes.
    assert!(compile("€".repeat(1365)).is_ok());
    assert!(compile("€".repeat(1366)).is_err());
    assert_eq!(
        compile("a".repeat(4097)).unwrap_err().code,
        "autodelete.prompt_invalid"
    );
    assert!(compile("line one\n\tline two".into()).is_ok());
    assert!(compile("bell\u{7}".into()).is_err());

    let days = |inactive_days| {
        parse_change(AutodeleteChange::SetInactiveDays {
            rule_id: RULE.to_string(),
            inactive_days,
        })
    };
    assert_eq!(days(0).unwrap_err().code, "autodelete.days_invalid");
    assert!(days(1).is_ok());
    assert!(days(36_500).is_ok());
    assert_eq!(days(36_501).unwrap_err().code, "autodelete.days_invalid");

    let unknown = parse_change(AutodeleteChange::Delete {
        rule_id: "../rule".into(),
    })
    .unwrap_err();
    assert_eq!(unknown.code, "autodelete.rule_unknown");
    let expired = parse_change(AutodeleteChange::Approve {
        token_id: "nope".into(),
    })
    .unwrap_err();
    assert_eq!(
        (expired.code.as_str(), expired.category),
        ("autodelete.token_expired", "conflict")
    );

    let change: AutodeleteChange = serde_json::from_value(json!({
        "kind": "set_inactive_days", "rule_id": RULE.to_string(), "inactive_days": 30
    }))
    .unwrap();
    assert!(matches!(
        change,
        AutodeleteChange::SetInactiveDays {
            inactive_days: 30,
            ..
        }
    ));
    // The webview can't name the days it approves.
    assert!(serde_json::from_value::<AutodeleteChange>(json!({
        "kind": "approve", "token_id": "x", "inactive_days": 30
    }))
    .is_err());
}

#[test]
fn tokens_survive_a_read_of_the_same_interpretation_and_expire_when_it_changes() {
    let state = RuleState::default();
    let first = state
        .refresh_tokens(local(), &[&rule(draft(30), 5)])
        .unwrap();
    let token = first[&RULE].approve.unwrap();
    assert_eq!(first[&RULE].everywhere, None);

    // A polling reload of an unchanged draft keeps the token.
    let again = state
        .refresh_tokens(local(), &[&rule(draft(30), 5)])
        .unwrap();
    assert_eq!(again[&RULE].approve, Some(token));

    // Edited elsewhere: same number, new update time, new token.
    let edited = state
        .refresh_tokens(local(), &[&rule(draft(30), 6)])
        .unwrap();
    assert_ne!(edited[&RULE].approve, Some(token));
    let error = state
        .claim(local(), ParsedChange::Approve { token })
        .unwrap_err();
    assert_eq!(error.code, "autodelete.token_expired");

    // Only an approved Forget rule can be authorized to delete everywhere.
    let approved = AutodeleteRuleState::Approved {
        inactive_days: 30,
        approved_at_unix_ms: 9,
    };
    let tokens = state
        .refresh_tokens(local(), &[&rule(approved.clone(), 7)])
        .unwrap();
    assert_eq!(tokens[&RULE].approve, None);
    assert!(tokens[&RULE].everywhere.is_some());
    let everywhere = AutodeleteRule {
        scope: AutodeleteScope::Everywhere,
        ..rule(approved, 8)
    };
    let tokens = state.refresh_tokens(local(), &[&everywhere]).unwrap();
    assert_eq!(tokens[&RULE].everywhere, None);

    // A rule that disappears takes its tokens with it.
    state.refresh_tokens(local(), &[]).unwrap();
    assert_eq!(
        state
            .claim(local(), ParsedChange::Delete { rule: RULE })
            .unwrap_err()
            .code,
        "autodelete.rule_unknown"
    );
}

#[test]
fn a_restored_store_drops_that_planes_tokens_and_unconfirmed_changes_only() {
    let state = RuleState::default();
    let remote = PlaneBinding {
        plane: PlaneId::Remote(Uuid::from_u128(0xad9)),
        identity: Some(Uuid::from_u128(0xada)),
    };
    let local_token = state
        .refresh_tokens(local(), &[&rule(draft(30), 5)])
        .unwrap()[&RULE]
        .approve
        .unwrap();
    let remote_token = state
        .refresh_tokens(remote, &[&rule(draft(30), 5)])
        .unwrap()[&RULE]
        .approve
        .unwrap();
    // An unconfirmed change holds the local rule's slot.
    state
        .claim(
            local(),
            ParsedChange::SetDays {
                rule: RULE,
                days: 40,
            },
        )
        .unwrap();

    state.plane_restored(PlaneId::Local);

    assert_eq!(
        state
            .claim(local(), ParsedChange::Approve { token: local_token })
            .unwrap_err()
            .code,
        "autodelete.token_expired"
    );
    // The slot is free again, and the rule must be read anew first.
    assert_eq!(
        state
            .claim(
                local(),
                ParsedChange::SetDays {
                    rule: RULE,
                    days: 50
                }
            )
            .unwrap_err()
            .code,
        "autodelete.rule_unknown"
    );
    assert!(state.pending_views(local()).unwrap().is_empty());
    assert!(state
        .claim(
            remote,
            ParsedChange::Approve {
                token: remote_token
            }
        )
        .is_ok());
}

#[test]
fn rule_views_carry_tokens_safe_reasons_and_attribution_only_for_the_drafted_days() {
    let tokens = RuleTokens::issue(RuleBinding::of(&rule(draft(30), 5)));
    let candidates: Vec<AutodeleteCandidate> = (0..32)
        .map(|index| AutodeleteCandidate {
            conversation_id: Uuid::from_u128(index + 1),
            last_active_at_unix_ms: 3,
            protections: vec![RetentionProtection::EnabledSchedule],
        })
        .collect();
    let job = JobAttribution {
        provider: Some("anthropic".into()),
        model: Some("claude-haiku".into()),
        drafted_days: Some(30),
    };
    let subject = rule(draft(30), 5);
    let view = rule_view(
        &subject,
        &candidates,
        &tokens,
        attribution_view(&subject, Some(&job)),
    )
    .unwrap();
    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(json["state"]["kind"], "draft");
    assert_eq!(json["state"]["inactiveDays"], 30);
    assert_eq!(
        json["state"]["approveToken"],
        tokens.approve.unwrap().to_string()
    );
    assert_eq!(json["scope"], "forget");
    assert_eq!(json["candidatesCapped"], true);
    assert_eq!(
        json["candidates"][0]["protections"][0]["kind"],
        "enabled_schedule"
    );
    assert_eq!(
        json["attribution"],
        json!({"provider": "anthropic", "model": "claude-haiku"})
    );
    // The job ID never crosses.
    assert!(!json.to_string().contains(&JOB.to_string()));

    // Set by hand to another number: the model did not draft what is shown.
    assert_eq!(attribution_view(&rule(draft(45), 6), Some(&job)), None);
    let unnamed = JobAttribution {
        model: None,
        ..job.clone()
    };
    assert_eq!(attribution_view(&subject, Some(&unnamed)), None);

    let too_many = [candidates.clone(), candidates].concat();
    assert!(rule_view(&subject, &too_many, &tokens, None).is_err());

    let refused = |reason: &str| {
        let subject = rule(
            AutodeleteRuleState::Refused {
                reason: reason.into(),
            },
            5,
        );
        let tokens = RuleTokens::issue(RuleBinding::of(&subject));
        serde_json::to_value(rule_view(&subject, &[], &tokens, None).unwrap()).unwrap()["state"]
            .clone()
    };
    assert_eq!(
        refused("utility.disabled"),
        json!({"kind": "refused", "reason": "utility.disabled"})
    );
    assert_eq!(refused("<b>Nope</b>")["reason"], "autodelete.refused");
}

// ---------------------------------------------------------------------------
// Fake jetd
// ---------------------------------------------------------------------------

struct Fake {
    setup: Setup,
    plane_id: String,
    binding: PlaneBinding,
    listener: UnixListener,
}

fn fake_plane() -> Fake {
    let setup = setup();
    let socket = setup.directory.path().join("rules-jetd.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let id = Uuid::from_u128(0xad5);
    setup.bridge.planes.insert_for_test(
        id,
        "Build box",
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
        binding: PlaneBinding {
            plane: PlaneId::Remote(id),
            identity: None,
        },
        listener,
    }
}

impl Fake {
    fn bridge(&self) -> &JetBridge {
        &self.setup.bridge
    }

    fn state(&self) -> &RuleState {
        &self.bridge().retention.rules
    }

    /// Makes `rules` the last read, as a rules load would.
    fn known(&self, rules: &[&AutodeleteRule]) {
        self.state().refresh_tokens(self.binding, rules).unwrap();
    }

    fn approve_token(&self) -> Option<Uuid> {
        self.state().tokens.lock().unwrap()[&self.binding.plane].rules[&RULE].approve
    }

    async fn change(&self, change: AutodeleteChange) -> Result<RuleChangeOutcome, PublicError> {
        change_rule_for(self.bridge(), &self.plane_id, change).await
    }

    async fn load(&self) -> Result<AutodeleteRulesView, PublicError> {
        load_rules_for(self.bridge(), &self.plane_id).await
    }

    async fn assert_untouched(&self) {
        let pending = tokio::time::timeout(Duration::from_millis(20), self.listener.accept()).await;
        assert!(pending.is_err(), "the Plane was contacted");
    }
}

fn set_days_for(rule: Uuid, days: u32) -> AutodeleteChange {
    AutodeleteChange::SetInactiveDays {
        rule_id: rule.to_string(),
        inactive_days: days,
    }
}

fn set_days(days: u32) -> AutodeleteChange {
    set_days_for(RULE, days)
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

async fn refuse(writer: &mut Writer, stream: StreamId, message: &ClientMessage, code: &str) {
    reply(
        writer,
        stream,
        ServerMessage::Error {
            id: Some(request_id(message)),
            error: WireError {
                category: ErrorCategory::Conflict,
                code: code.into(),
                retryable: false,
                message: "daemon text never crosses".into(),
                revision_conflict: None,
                restart: None,
                recovery_actions: Vec::new(),
            },
        },
    )
    .await;
}

async fn recorded(
    writer: &mut Writer,
    stream: StreamId,
    message: &ClientMessage,
    rule: AutodeleteRule,
) {
    reply(
        writer,
        stream,
        ServerMessage::CommandResult {
            id: request_id(message),
            result: CommandResponse::AutodeleteRuleRecorded { rule },
        },
    )
    .await;
}

/// The next request is a rule Command; returns its Command ID and body.
async fn expect_command(reader: &mut Reader) -> (StreamId, ClientMessage, Uuid, CommandRequest) {
    let (stream, message) = next_message(reader).await;
    let ClientMessage::Command {
        command_id,
        command,
        ..
    } = &message
    else {
        panic!("unexpected request {message:?}");
    };
    let (command_id, command) = (*command_id, command.clone());
    (stream, message, command_id, command)
}

/// One `set_inactive_days` round: the fake daemon answers with a draft of
/// `days`, or drops the connection (a lost reply) when `lost`.
async fn days_round(
    fake: &Fake,
    days: u32,
    lost: bool,
) -> (Result<RuleChangeOutcome, PublicError>, Uuid) {
    tokio::join!(fake.change(set_days(days)), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message, command_id, command) = expect_command(&mut reader).await;
        assert_eq!(
            command,
            CommandRequest::SetAutodeleteRuleInactiveDays {
                rule_id: RULE,
                inactive_days: days,
            }
        );
        if !lost {
            let answered = rule(draft(days), i64::from(days));
            recorded(&mut writer, stream, &message, answered).await;
        }
        command_id
    })
}

// ---------------------------------------------------------------------------
// Rule Command IDs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unconfirmed_30_blocks_60_until_it_resolves_and_a_later_30_is_new() {
    let fake = fake_plane();
    fake.known(&[&rule(draft(14), 1)]);

    let (first, thirty) = days_round(&fake, 30, true).await;
    assert_eq!(first.unwrap_err().category, "offline");

    // A different change to the same rule waits, with nothing sent.
    let mismatch = fake.change(set_days(60)).await.unwrap();
    let RuleChangeOutcome::Refused { error } = mismatch else {
        panic!("expected a refusal");
    };
    assert_eq!(
        (error.code.as_str(), error.category),
        ("autodelete.retry_mismatch", "conflict")
    );
    assert_eq!(error.plane_id.as_deref(), Some(fake.plane_id.as_str()));
    fake.assert_untouched().await;

    // The unconfirmed change is listed for "Try again".
    assert_eq!(
        serde_json::to_value(fake.state().pending_views(fake.binding).unwrap()).unwrap(),
        json!([{
            "ruleId": RULE.to_string(),
            "change": {"kind": "set_inactive_days", "rule_id": RULE.to_string(), "inactive_days": 30},
        }])
    );

    // Try again resends the same Command ID.
    let (retried, again) = days_round(&fake, 30, false).await;
    assert_eq!(again, thirty);
    let RuleChangeOutcome::Recorded { rule: view } = retried.unwrap() else {
        panic!("expected the rule");
    };
    assert_eq!(
        view.state,
        RuleStateView::Draft {
            inactive_days: 30,
            approve_token: fake.approve_token().unwrap().to_string(),
        }
    );
    assert!(fake.state().pending_views(fake.binding).unwrap().is_empty());

    let (sixty, sixty_id) = days_round(&fake, 60, false).await;
    assert!(matches!(sixty.unwrap(), RuleChangeOutcome::Recorded { .. }));
    assert_ne!(sixty_id, thirty);

    // 30 again after 60 applied is a new request that really applies.
    let (later, later_id) = days_round(&fake, 30, false).await;
    assert!(matches!(later.unwrap(), RuleChangeOutcome::Recorded { .. }));
    assert_ne!(later_id, thirty);
    assert_ne!(later_id, sixty_id);
}

#[tokio::test]
async fn a_new_rule_keeps_its_id_across_retries_and_a_deleted_one_is_forgotten() {
    let fake = fake_plane();
    fake.known(&[]);
    let compile = || AutodeleteChange::Compile {
        rule_id: None,
        prompt: "Old scratch tasks".into(),
    };
    let expect_compile = |command: CommandRequest| match command {
        CommandRequest::CompileAutodeleteRule { rule_id, prompt } => {
            assert_eq!(prompt, "Old scratch tasks");
            rule_id
        }
        other => panic!("unexpected command {other:?}"),
    };

    let (lost, (first_id, first_rule)) = tokio::join!(fake.change(compile()), async {
        let (mut reader, _writer) = accept(&fake.listener, CLIENT).await;
        let (_, _, command_id, command) = expect_command(&mut reader).await;
        (command_id, expect_compile(command))
    });
    assert!(lost.is_err());
    // Another new rule waits for the first to be confirmed.
    let other = fake
        .change(AutodeleteChange::Compile {
            rule_id: None,
            prompt: "Something else".into(),
        })
        .await
        .unwrap();
    assert!(
        matches!(other, RuleChangeOutcome::Refused { ref error } if error.code == "autodelete.retry_mismatch")
    );
    assert_eq!(
        serde_json::to_value(fake.state().pending_views(fake.binding).unwrap()).unwrap(),
        json!([{
            "ruleId": null,
            "change": {"kind": "compile", "rule_id": null, "prompt": "Old scratch tasks"},
        }])
    );

    let (created, (again_id, again_rule)) = tokio::join!(fake.change(compile()), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message, command_id, command) = expect_command(&mut reader).await;
        let rule_id = expect_compile(command);
        let compiling = AutodeleteRule {
            rule_id,
            ..rule(AutodeleteRuleState::Compiling, 2)
        };
        recorded(&mut writer, stream, &message, compiling).await;
        (command_id, rule_id)
    });
    assert_eq!((again_id, again_rule), (first_id, first_rule));
    let RuleChangeOutcome::Recorded { rule: view } = created.unwrap() else {
        panic!("expected the new rule");
    };
    assert_eq!(view.state, RuleStateView::Compiling);
    assert_eq!(view.rule_id, first_rule.to_string());

    // The new rule is known: it can be deleted before any reload.
    let (deleted, delete_id) = tokio::join!(
        fake.change(AutodeleteChange::Delete {
            rule_id: first_rule.to_string()
        }),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message, command_id, command) = expect_command(&mut reader).await;
            assert_eq!(
                command,
                CommandRequest::DeleteAutodeleteRule {
                    rule_id: first_rule
                }
            );
            reply(
                &mut writer,
                stream,
                ServerMessage::CommandResult {
                    id: request_id(&message),
                    result: CommandResponse::AutodeleteRuleDeleted {
                        rule_id: first_rule,
                    },
                },
            )
            .await;
            command_id
        }
    );
    assert_eq!(
        deleted.unwrap(),
        RuleChangeOutcome::Deleted {
            rule_id: first_rule.to_string()
        }
    );
    assert_ne!(delete_id, first_id);

    // Deleted: changing it again is refused without contacting the Plane.
    let gone = fake.change(set_days_for(first_rule, 3)).await.unwrap();
    assert!(
        matches!(gone, RuleChangeOutcome::Refused { ref error } if error.code == "autodelete.rule_unknown")
    );
    fake.assert_untouched().await;

    // The same wording again is a new rule with new IDs.
    let (_, (fresh_id, fresh_rule)) = tokio::join!(fake.change(compile()), async {
        let (mut reader, _writer) = accept(&fake.listener, CLIENT).await;
        let (_, _, command_id, command) = expect_command(&mut reader).await;
        (command_id, expect_compile(command))
    });
    assert_ne!(fresh_id, first_id);
    assert_ne!(fresh_rule, first_rule);
}

#[tokio::test]
async fn a_definite_refusal_frees_the_rule_and_an_unknown_one_is_refused_locally() {
    let fake = fake_plane();
    fake.known(&[&rule(AutodeleteRuleState::Compiling, 1)]);
    let (outcome, ()) = tokio::join!(fake.change(set_days(30)), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message, _, _) = expect_command(&mut reader).await;
        refuse(&mut writer, stream, &message, "recovery.read_only").await;
    });
    let RuleChangeOutcome::Refused { error } = outcome.unwrap() else {
        panic!("expected a refusal");
    };
    assert_eq!(error.code, "recovery.read_only");
    assert_eq!(error.plane_id.as_deref(), Some(fake.plane_id.as_str()));
    assert!(fake.state().pending_views(fake.binding).unwrap().is_empty());

    let unknown = fake
        .change(set_days_for(Uuid::from_u128(0xbad), 30))
        .await
        .unwrap();
    assert!(
        matches!(unknown, RuleChangeOutcome::Refused { ref error } if error.code == "autodelete.rule_unknown")
    );
    let invalid = fake.change(set_days(0)).await.unwrap();
    assert!(
        matches!(invalid, RuleChangeOutcome::Refused { ref error } if error.code == "autodelete.days_invalid")
    );
    fake.assert_untouched().await;
}

#[tokio::test]
async fn a_refusal_never_echoes_a_raw_plane_handle() {
    let fake = fake_plane();
    fake.known(&[&rule(AutodeleteRuleState::Compiling, 1)]);
    let hostile = format!("{}\u{7}{}", "x".repeat(4096), "\n");
    for handle in [hostile.as_str(), "../jetd.sock", "LOCAL"] {
        // An invalid days value would be refused first if the handle were
        // not parsed before anything else.
        let outcome = change_rule_for(fake.bridge(), handle, set_days(0))
            .await
            .unwrap();
        let RuleChangeOutcome::Refused { error } = outcome else {
            panic!("expected a refusal");
        };
        assert_eq!(error.code, "plane.unknown");
        assert_eq!(error.plane_id, None);
    }
    // A valid handle is echoed in its canonical form.
    let invalid = fake.change(set_days(0)).await.unwrap();
    let RuleChangeOutcome::Refused { error } = invalid else {
        panic!("expected a refusal");
    };
    assert_eq!(error.plane_id.as_deref(), Some(fake.plane_id.as_str()));
    fake.assert_untouched().await;
}

// ---------------------------------------------------------------------------
// Reads: tokens, approval, drafting and attribution
// ---------------------------------------------------------------------------

fn job(id: Uuid, days: Option<u32>) -> UtilityJob {
    UtilityJob {
        job_id: id,
        plane_id: Uuid::from_u128(0xad5),
        purpose: UtilityPurpose::Autodelete,
        provider: Some("anthropic".into()),
        binding_id: Some(Uuid::from_u128(0xb1)),
        model: Some("claude-haiku".into()),
        policy: UtilityPolicy {
            version: 1,
            enabled: true,
            cross_provider_consent: false,
        },
        outcome: match days {
            Some(inactive_days) => UtilityOutcome::Draft { inactive_days },
            None => UtilityOutcome::Refused {
                reason: "utility.disabled".into(),
            },
        },
    }
}

async fn serve_setting(
    reader: &mut Reader,
    writer: &mut Writer,
    key: SettingKey,
    value: SettingValue,
) {
    let (stream, message) = next_message(reader).await;
    assert!(
        matches!(
            message,
            ClientMessage::Query {
                query: QueryRequest::Settings {
                    scope: SettingScope::Plane,
                    selection: SettingSelection::Key { key: asked },
                },
                ..
            } if asked == key
        ),
        "unexpected request {message:?}"
    );
    answer(
        writer,
        stream,
        &message,
        QueryResponse::Settings(SettingSnapshot {
            cursor: 4,
            scope: SettingScope::Plane,
            settings: vec![ResolvedSetting {
                key,
                value,
                source: SettingSource::BuiltIn,
            }],
        }),
    )
    .await;
}

/// Serves one rules load: the list, the two drafting Settings, then every
/// `utility` Query through `jobs` (`None` refuses it). Returns how many
/// `utility` Queries arrived before the connection closed.
async fn serve_load(
    fake: &Fake,
    rules: Vec<AutodeleteRulePreview>,
    jobs: impl Fn(Uuid) -> Option<UtilityJob>,
) -> usize {
    let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
    let (stream, message) = next_message(&mut reader).await;
    assert!(matches!(
        message,
        ClientMessage::Query {
            query: QueryRequest::AutodeleteRules,
            ..
        }
    ));
    answer(
        &mut writer,
        stream,
        &message,
        QueryResponse::AutodeleteRules(AutodeleteRules { cursor: 17, rules }),
    )
    .await;
    serve_setting(
        &mut reader,
        &mut writer,
        SettingKey::UtilityAutodeleteCompilation,
        SettingValue::Flag(true),
    )
    .await;
    serve_setting(
        &mut reader,
        &mut writer,
        SettingKey::UtilityAccountBinding,
        SettingValue::Text(String::new()),
    )
    .await;
    let mut lookups = 0;
    while let Ok(frame) = reader.read().await {
        let jet_protocol::Frame::Control { stream_id, payload } = frame else {
            panic!("expected a control frame");
        };
        let message: ClientMessage = jet_protocol::decode_control(&payload).unwrap();
        let ClientMessage::Query {
            query: QueryRequest::Utility { job_id },
            ..
        } = &message
        else {
            panic!("unexpected request {message:?}");
        };
        lookups += 1;
        match jobs(*job_id) {
            Some(job) => {
                let result = QueryResponse::Utility(job);
                answer(&mut writer, stream_id, &message, result).await;
            }
            None => refuse(&mut writer, stream_id, &message, "utility.not_found").await,
        }
    }
    lookups
}

fn preview(rule: AutodeleteRule) -> AutodeleteRulePreview {
    AutodeleteRulePreview {
        rule,
        candidates: Vec::new(),
    }
}

#[tokio::test]
async fn approve_sends_the_native_days_behind_a_token_that_survives_an_unchanged_read() {
    let fake = fake_plane();
    let drafted = |id| Some(job(id, Some(30)));
    let (view, _) = tokio::join!(
        fake.load(),
        serve_load(&fake, vec![preview(rule(draft(30), 5))], drafted)
    );
    let view = serde_json::to_value(view.unwrap()).unwrap();
    assert_eq!(view["planeLabel"], "Build box");
    assert_eq!(view["cursor"], "17");
    assert_eq!(
        view["drafting"],
        json!({"enabled": true, "bindingConfigured": false})
    );
    assert_eq!(
        view["rules"][0]["attribution"],
        json!({"provider": "anthropic", "model": "claude-haiku"})
    );
    let token = view["rules"][0]["state"]["approveToken"]
        .as_str()
        .unwrap()
        .to_owned();

    // A poll reads the same draft: same token, and the attribution comes
    // from the cache without another Query.
    let (again, lookups) = tokio::join!(
        fake.load(),
        serve_load(&fake, vec![preview(rule(draft(30), 5))], |_| None)
    );
    assert_eq!(lookups, 0);
    let again = serde_json::to_value(again.unwrap()).unwrap();
    assert_eq!(again["rules"][0]["state"]["approveToken"], token);
    assert_eq!(again["rules"][0]["attribution"]["model"], "claude-haiku");

    // Approving sends the 30 days the shell read, nothing from the webview.
    let approved_state = AutodeleteRuleState::Approved {
        inactive_days: 30,
        approved_at_unix_ms: 40,
    };
    let (approved, ()) = tokio::join!(
        fake.change(AutodeleteChange::Approve {
            token_id: token.clone()
        }),
        async {
            let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
            let (stream, message, _, command) = expect_command(&mut reader).await;
            assert_eq!(
                command,
                CommandRequest::ApproveAutodeleteRule {
                    rule_id: RULE,
                    inactive_days: 30,
                }
            );
            let answered = rule(approved_state.clone(), 40);
            recorded(&mut writer, stream, &message, answered).await;
        }
    );
    let RuleChangeOutcome::Recorded { rule: view } = approved.unwrap() else {
        panic!("expected the approved rule");
    };
    let RuleStateView::Approved {
        everywhere_token: Some(everywhere),
        ..
    } = view.state
    else {
        panic!("expected an approved Forget rule");
    };
    // The used approval token is gone.
    let reused = fake
        .change(AutodeleteChange::Approve { token_id: token })
        .await
        .unwrap();
    assert!(
        matches!(reused, RuleChangeOutcome::Refused { ref error } if error.code == "autodelete.token_expired")
    );
    fake.assert_untouched().await;

    // Delete everywhere, with the reply lost: Try again resolves by its
    // token even after a reload no longer lists it.
    let authorize = || AutodeleteChange::AuthorizeEverywhere {
        token_id: everywhere.clone(),
    };
    let (lost, first_id) = tokio::join!(fake.change(authorize()), async {
        let (mut reader, _writer) = accept(&fake.listener, CLIENT).await;
        let (_, _, command_id, command) = expect_command(&mut reader).await;
        assert_eq!(
            command,
            CommandRequest::AuthorizeAutodeleteEverywhere { rule_id: RULE }
        );
        command_id
    });
    assert!(lost.is_err());
    let everywhere_rule = AutodeleteRule {
        scope: AutodeleteScope::Everywhere,
        ..rule(approved_state, 41)
    };
    let (reloaded, _) = tokio::join!(
        fake.load(),
        serve_load(&fake, vec![preview(everywhere_rule.clone())], drafted)
    );
    let reloaded = serde_json::to_value(reloaded.unwrap()).unwrap();
    assert_eq!(
        reloaded["rules"][0]["state"]["everywhereToken"],
        serde_json::Value::Null
    );
    assert_eq!(
        reloaded["pending"],
        json!([{
            "ruleId": RULE.to_string(),
            "change": {"kind": "authorize_everywhere", "token_id": everywhere},
        }])
    );
    let (retried, again_id) = tokio::join!(fake.change(authorize()), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message, command_id, _) = expect_command(&mut reader).await;
        recorded(&mut writer, stream, &message, everywhere_rule).await;
        command_id
    });
    assert_eq!(again_id, first_id);
    assert!(matches!(
        retried.unwrap(),
        RuleChangeOutcome::Recorded { .. }
    ));
}

#[tokio::test]
async fn attribution_issues_at_most_64_queries_and_a_failure_only_leaves_it_out() {
    let fake = fake_plane();
    let failing = Uuid::from_u128(0x5000);
    let rules: Vec<AutodeleteRulePreview> = (0..64)
        .map(|index| {
            preview(AutodeleteRule {
                rule_id: Uuid::from_u128(0x1000 + index),
                utility_job_id: if index == 0 {
                    failing
                } else {
                    Uuid::from_u128(0x2000 + index)
                },
                ..rule(draft(30), 5)
            })
        })
        .collect();
    let (view, lookups) = tokio::join!(
        fake.load(),
        serve_load(&fake, rules.clone(), |id| (id != failing)
            .then(|| job(id, Some(30))))
    );
    assert_eq!(lookups, 64);
    let view = serde_json::to_value(view.unwrap()).unwrap();
    assert_eq!(view["rules"][0]["attribution"], serde_json::Value::Null);
    assert_eq!(view["rules"][1]["attribution"]["provider"], "anthropic");

    // Answered jobs are cached; only the failed lookup is asked again.
    let (_, lookups) = tokio::join!(
        fake.load(),
        serve_load(&fake, rules, |id| Some(job(id, Some(30))))
    );
    assert_eq!(lookups, 1);

    // More than 64 rules is not a list this Plane may send.
    let too_many: Vec<AutodeleteRulePreview> = (0..65)
        .map(|index| {
            preview(AutodeleteRule {
                rule_id: Uuid::from_u128(0x3000 + index),
                ..rule(AutodeleteRuleState::Compiling, 5)
            })
        })
        .collect();
    let (refused, ()) = tokio::join!(fake.load(), async {
        let (mut reader, mut writer) = accept(&fake.listener, CLIENT).await;
        let (stream, message) = next_message(&mut reader).await;
        let result = QueryResponse::AutodeleteRules(AutodeleteRules {
            cursor: 1,
            rules: too_many,
        });
        answer(&mut writer, stream, &message, result).await;
    });
    assert_eq!(refused.unwrap_err().code, "client.state_unavailable");
}

#[tokio::test]
async fn a_refused_rule_reads_without_attribution_or_utility_queries() {
    let fake = fake_plane();
    let refused = rule(
        AutodeleteRuleState::Refused {
            reason: "utility.disabled".into(),
        },
        5,
    );
    let (view, lookups) = tokio::join!(
        fake.load(),
        serve_load(&fake, vec![preview(refused)], |id| Some(job(id, None)))
    );
    assert_eq!(lookups, 0);
    let view = serde_json::to_value(view.unwrap()).unwrap();
    assert_eq!(
        view["rules"][0]["state"],
        json!({"kind": "refused", "reason": "utility.disabled"})
    );
    assert_eq!(view["rules"][0]["attribution"], serde_json::Value::Null);
    assert_eq!(view["pending"], json!([]));
}
