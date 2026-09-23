use jet_protocol::{
    AccountBinding, AccountBindingStatus, BrokerPermission, CapabilitySnapshot,
    CraftInstallationConfirmation, CraftInstallationPreview, CredentialItem, CredentialState,
    CredentialStoreKind, InstalledCraft, ObservedConsumption, Platform, QuotaMeasure, UsagePoint,
    UsageSeries,
};
use serde_json::json;

use super::*;
use crate::jet::{
    enrollment::tests::{setup, Setup},
    planes::PlaneBinding,
};

const BINDING: Uuid = Uuid::from_u128(0xb1);

fn local() -> PlaneKey {
    PlaneBinding {
        plane: PlaneId::Local,
        identity: None,
    }
}

pub(crate) fn capabilities(harnesses: &[&str]) -> CapabilitySnapshot {
    CapabilitySnapshot {
        resource_budgets: None,
        observed_at_unix_ms: 1,
        core_version: "0.43.0".into(),
        platform: Platform {
            operating_system: "linux".into(),
            architecture: "x86_64".into(),
        },
        external_tools: Vec::new(),
        credential_store: CredentialStoreStatus::Locked {
            kind: CredentialStoreKind::SecretService,
        },
        crafts: vec![InstalledCraft {
            subagent_control: None,
            craft_id: "codex".into(),
            version: "1.4.2".into(),
            harnesses: harnesses.iter().map(|id| (*id).to_owned()).collect(),
        }],
        harnesses: harnesses.iter().map(|id| (*id).to_owned()).collect(),
        degraded: Vec::new(),
    }
}

fn window(binding_id: Uuid, freshness: UsageFreshness) -> QuotaWindow {
    QuotaWindow {
        binding_id,
        provider: "openai".into(),
        window: "5h".into(),
        scope: QuotaScope::Model {
            model: "gpt-5".into(),
        },
        conversation_id: None,
        run_id: None,
        measure: QuotaMeasure {
            unit: QuotaUnit::Share,
            used: 62,
            limit: Some(100),
        },
        window_seconds: Some(18_000),
        resets_at_unix_ms: Some(1_700_000_000_000),
        estimation: UsageEstimation::Measured,
        finality: UsageFinality::Final,
        observed_at_unix_ms: 1_699_999_000_000,
        freshness,
    }
}

fn usage(windows: Vec<QuotaWindow>) -> PlaneUsage {
    PlaneUsage {
        cursor: 7,
        plane_id: Uuid::from_u128(1),
        quota_windows: windows,
        consumption: ObservedConsumption::default(),
    }
}

// ---------------------------------------------------------------------------
// Names and views
// ---------------------------------------------------------------------------

#[test]
fn harnesses_are_named_by_product_and_unknown_ids_stay_bounded() {
    assert_eq!(harness_display_name("codex"), Some("Codex"));
    assert_eq!(harness_display_name("claude-code"), Some("Claude Code"));
    assert_eq!(harness_display_name("aider"), None);
    assert_eq!(harness_view("aider").name, "aider");
    assert_eq!(harness_view("bad\u{7}id").name, "Unknown Harness");
    let options = auth_provider_options(&["codex".into(), "codex-cli".into(), "claude".into()]);
    assert_eq!(
        options
            .iter()
            .map(|option| (option.provider, option.harness, option.label))
            .collect::<Vec<_>>(),
        [
            ("openai", "Codex", "Codex login"),
            ("anthropic", "Claude Code", "Claude Code login")
        ]
    );
}

#[test]
fn the_agents_view_keeps_sections_that_loaded_and_names_what_failed() {
    let status = crate::jet::client::unit_tests::status(4);
    let accounts = AccountBindingList {
        cursor: 12,
        bindings: vec![AccountBindingStatus {
            binding: AccountBinding {
                binding_id: BINDING,
                provider: "anthropic".into(),
                label: "Claude Code login".into(),
                provider_account: None,
                credential_reference: CredentialReference::PlatformStore {
                    item: CredentialItem {
                        service: "secret-service-item".into(),
                        account: "someone@example.com".into(),
                    },
                },
                created_at_unix_ms: 5,
            },
            credential_state: CredentialState::WaitingForUnlock {
                kind: CredentialStoreKind::SecretService,
            },
        }],
    };
    let view = agents_view(
        &status,
        Some(capabilities(&["codex"])),
        Some(accounts),
        None,
        vec![issue("usage", PublicError::internal())],
    );
    let view = serde_json::to_value(view).unwrap();
    assert_eq!(
        view["crafts"],
        json!([{"craftId": "codex", "version": "1.4.2", "harnesses": [{"id": "codex", "name": "Codex"}]}])
    );
    assert_eq!(
        view["credentialStore"],
        json!({"state": "locked", "label": "Secure storage locked"})
    );
    assert_eq!(
        view["bindOptions"],
        json!([{"provider": "openai", "harness": "Codex"}])
    );
    assert_eq!(
        view["accounts"][0],
        json!({
            "id": BINDING.to_string(),
            "label": "Claude Code login",
            "provider": "anthropic",
            "providerAccount": null,
            "credentialSource": "platform_store",
            "state": "locked",
            "stateLabel": "Unlock required",
            "createdAtUnixMs": "5"
        })
    );
    // The keyring item never crosses.
    assert!(!view.to_string().contains("secret-service-item"));
    assert!(!view.to_string().contains("someone@example.com"));
    assert_eq!(view["cursor"], "12");
    assert_eq!(view["usage"], json!(null));
    assert_eq!(view["issues"][0]["section"], "usage");
}

#[test]
fn an_unreachable_providers_reason_never_crosses() {
    let usage = usage(vec![window(
        BINDING,
        UsageFreshness::Unreachable {
            reason: "provider said: token sk-live-123 expired".into(),
        },
    )]);
    let windows = quota_windows(&usage, BINDING).unwrap();
    let json = serde_json::to_value(&windows).unwrap();
    assert_eq!(
        json[0],
        json!({
            "provider": "openai",
            "window": "5h",
            "scope": {"kind": "model", "model": "gpt-5"},
            "unit": "share",
            "used": "62",
            "limit": "100",
            "windowSeconds": "18000",
            "resetsAtUnixMs": "1700000000000",
            "estimation": "measured",
            "finality": "final",
            "observedAtUnixMs": "1699999000000",
            "freshness": "unreachable"
        })
    );
    assert!(!json.to_string().contains("sk-live"));
}

#[test]
fn a_quota_window_of_another_account_is_an_internal_error() {
    let usage = usage(vec![
        window(BINDING, UsageFreshness::Fresh),
        window(Uuid::from_u128(0xb2), UsageFreshness::Stale),
    ]);
    assert_eq!(
        quota_windows(&usage, BINDING).unwrap_err().category,
        "internal"
    );
}

#[test]
fn unbinding_names_only_the_cleanup_left_to_the_user() {
    let platform = CredentialReference::PlatformStore {
        item: CredentialItem {
            service: "s".into(),
            account: "a".into(),
        },
    };
    assert_eq!(cleanup_after_unbind(&platform), "keyring_item_remains");
    assert_eq!(
        cleanup_after_unbind(&CredentialReference::HarnessNative),
        "none"
    );
    assert_eq!(
        cleanup_after_unbind(&CredentialReference::ExternalHelper {
            helper: "op".into()
        }),
        "helper"
    );
    assert_eq!(
        cleanup_after_unbind(&CredentialReference::SessionOnly {
            established_at_daemon_start: 2
        }),
        "session"
    );
}

// ---------------------------------------------------------------------------
// Usage history
// ---------------------------------------------------------------------------

fn point(start: i64) -> UsagePoint {
    UsagePoint {
        start_unix_ms: start,
        tokens: UsageTokens {
            input: 10,
            cached_input: 2,
            output: 3,
            reasoning: 1,
        },
        measurements: 1,
        estimated: 0,
        interim: 0,
    }
}

#[test]
fn history_asks_hourly_up_to_a_week_and_returns_the_answers_resolution() {
    let now = 10 * DAY_MS;
    assert_eq!(
        history_request(1, now),
        (
            UsageHistoryRange {
                from_unix_ms: 9 * DAY_MS,
                until_unix_ms: now
            },
            UsageResolution::Hour
        )
    );
    assert_eq!(history_request(7, now).1, UsageResolution::Hour);
    assert_eq!(history_request(30, now).1, UsageResolution::Day);
    assert_eq!(history_request(90, now).1, UsageResolution::Day);
    for invalid in [0, 2, 14, 365] {
        assert_eq!(parse_days(invalid).unwrap_err().code, "agents.days_invalid");
    }

    // Asked hourly; the Plane answered daily. The view says daily.
    let history = UsageHistory {
        cursor: 3,
        plane_id: Uuid::from_u128(1),
        resolution: UsageResolution::Day,
        series: vec![UsageSeries {
            model: Some("gpt-5".into()),
            points: vec![point(DAY_MS), point(0)],
        }],
    };
    let view = serde_json::to_value(history_view(&history)).unwrap();
    assert_eq!(view["resolution"], "day");
    assert_eq!(view["truncated"], false);
    assert_eq!(view["series"][0]["points"][0]["startUnixMs"], "0");
    assert_eq!(
        view["series"][0]["points"][1]["tokens"],
        json!({"input": "10", "cachedInput": "2", "output": "3", "reasoning": "1"})
    );
}

#[test]
fn history_keeps_the_most_recent_points_when_capped() {
    let series = |n: usize| UsageSeries {
        model: None,
        points: (0..n).map(|index| point(index as i64)).collect(),
    };
    let history = UsageHistory {
        cursor: 3,
        plane_id: Uuid::from_u128(1),
        resolution: UsageResolution::Hour,
        series: (0..40).map(|_| series(100)).collect(),
    };
    let view = history_view(&history);
    assert!(view.truncated);
    assert_eq!(view.series.len(), MAX_HISTORY_SERIES);
    let total: usize = view.series.iter().map(|series| series.points.len()).sum();
    assert_eq!(total, MAX_HISTORY_POINTS);
    // Series 1-24 fill the budget; the rest have no points left.
    assert_eq!(view.series[23].points.len(), 100);
    assert_eq!(view.series[23].points[99].start_unix_ms, "99");
    assert!(view.series[24].points.is_empty());

    let history = UsageHistory {
        series: vec![series(MAX_HISTORY_POINTS + 5)],
        ..history
    };
    let view = history_view(&history);
    assert!(view.truncated);
    assert_eq!(view.series[0].points[0].start_unix_ms, "5");
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

#[test]
fn auto_continue_policies_are_checked_against_the_documented_bounds() {
    let retry = |delay: u64, max: u64, retries: u64, message: &str| json!({"mode": "retry", "delay_ms": delay, "max_delay_ms": max, "max_retries": retries, "message": message});
    assert_eq!(
        parse_policy(json!({"mode": "off"})).unwrap(),
        AutoContinuePolicy::Off
    );
    assert_eq!(
        parse_policy(retry(1, 86_400_000, 100, "Continue.\n")).unwrap(),
        AutoContinuePolicy::Retry {
            delay_ms: 1,
            max_delay_ms: 86_400_000,
            max_retries: 100,
            message: "Continue.\n".into(),
        }
    );
    let long = "a".repeat(MAX_MESSAGE_BYTES + 1);
    let mut extra = retry(1, 10, 1, "go");
    extra["target"] = json!("x");
    for invalid in [
        retry(0, 10, 1, "go"),
        retry(10, 86_400_001, 1, "go"),
        retry(20, 10, 1, "go"),
        retry(1, 10, 0, "go"),
        retry(1, 10, 101, "go"),
        retry(1, 10, 1, "  \n "),
        retry(1, 10, 1, &long),
        retry(1, 10, 1, "bell\u{7}"),
        extra,
        json!({"mode": "off", "delay_ms": 1}),
        json!({"mode": "always"}),
        retry(u64::from(u32::MAX) + 1, 10, 1, "go"),
    ] {
        assert_eq!(
            parse_policy(invalid.clone()).unwrap_err().code,
            "agents.policy_invalid",
            "{invalid}"
        );
    }
    assert!(parse_policy(retry(1, 10, 1, &"a".repeat(MAX_MESSAGE_BYTES))).is_ok());
}

#[test]
fn craft_sources_and_disable_inputs_are_narrow() {
    assert!(validate_release("openai/codex-craft", "v1.4.2").is_ok());
    assert!(validate_release("a.b_c-d/e", "2026.09.23-rc.1").is_ok());
    let long = format!("{}/x", "a".repeat(101));
    for (repository, tag) in [
        ("openai", "v1"),
        ("openai/codex/extra", "v1"),
        ("/codex", "v1"),
        ("openai/", "v1"),
        (long.as_str(), "v1"),
        ("open ai/codex", "v1"),
        ("openai/codex", ""),
        ("openai/codex", "v 1"),
        ("openai/codex", "v1\n"),
        ("https://github.com/openai/codex", "v1"),
    ] {
        assert_eq!(
            validate_release(repository, tag).unwrap_err().code,
            "agents.source_invalid",
            "{repository} {tag}"
        );
    }
    assert!(validate_release("openai/codex", &"t".repeat(129)).is_err());

    assert!(validate_craft_id("codex").is_ok());
    assert!(validate_craft_id("claude-code_2.x").is_ok());
    let long_id = "c".repeat(129);
    for invalid in ["", "Codex", "../codex", "codex craft", long_id.as_str()] {
        assert_eq!(
            validate_craft_id(invalid).unwrap_err().code,
            "agents.craft_missing"
        );
    }
    assert_eq!(parse_mode("wait").unwrap(), CraftDisableMode::Wait);
    assert_eq!(parse_mode("force").unwrap(), CraftDisableMode::Force);
    assert_eq!(parse_mode("now").unwrap_err().code, "agents.mode_invalid");

    // The webview can name a release or a native token, never a path.
    for source in [
        json!({"type": "local", "specification": "/tmp/craft-spec.toml", "artifact": "/tmp/craft"}),
        json!({"type": "github_release", "repository": "a/b", "tag": "v1", "path": "/x"}),
        json!({"type": "local", "source_token": "t", "specification": "/tmp/craft-spec.toml"}),
    ] {
        assert!(serde_json::from_value::<CraftSourceInput>(source).is_err());
    }
}

#[test]
fn a_craft_preview_shows_its_facts_but_never_the_local_paths() {
    let preview = CraftInstallationPreview {
        craft_id: "example".into(),
        version: "0.1.0".into(),
        enabled_features: vec!["tools".into()],
        confirmation: CraftInstallationConfirmation {
            source: CraftSource::Local {
                specification: "/home/someone/private/craft-spec.toml".into(),
                artifact: "/home/someone/private/target/craft".into(),
            },
            repository: "local".into(),
            publisher_claim: "Example".into(),
            commit: "abc123".into(),
            artifact_sha256: "e".repeat(64),
            broker_permissions: vec![BrokerPermission::ArtifactRead],
            host_access: vec![CraftHostAccess::Network {
                destination: "api.example.com:443".into(),
            }],
            trust: CraftTrust::DeveloperSource,
        },
    };
    let view = serde_json::to_value(preview_view(&preview)).unwrap();
    assert_eq!(view["source"], "local");
    assert_eq!(view["trust"], "developer_source");
    assert_eq!(view["brokerPermissions"], json!(["artifact_read"]));
    assert_eq!(
        view["hostAccess"],
        json!([{"kind": "network", "value": "api.example.com:443"}])
    );
    assert!(!view.to_string().contains("/home/someone"));
}

// ---------------------------------------------------------------------------
// Local Craft sources
// ---------------------------------------------------------------------------

#[test]
fn local_sources_are_canonical_regular_files_with_the_expected_name() {
    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let specification = real.join("craft-spec.toml");
    let artifact = real.join("craft");
    std::fs::write(&specification, "id = \"example\"").unwrap();
    std::fs::write(&artifact, "binary").unwrap();
    let linked = directory.path().join("linked");
    std::os::unix::fs::symlink(&real, &linked).unwrap();

    // Symlinks resolve to the files the Plane will actually read.
    let (spec, art) =
        local_source_paths(&linked.join("craft-spec.toml"), &linked.join("craft")).unwrap();
    let canonical = std::fs::canonicalize(&real).unwrap();
    assert_eq!(spec, canonical.join("craft-spec.toml").to_str().unwrap());
    assert_eq!(art, canonical.join("craft").to_str().unwrap());
    assert_eq!(file_name(&spec), "craft-spec.toml");

    let code = |spec: &Path, artifact: &Path| local_source_paths(spec, artifact).unwrap_err().code;
    // A directory is not a Craft file.
    assert_eq!(code(&specification, &real), "agents.source_invalid");
    // The specification must be named craft-spec.toml.
    let wrong = real.join("spec.toml");
    std::fs::write(&wrong, "").unwrap();
    assert_eq!(code(&wrong, &artifact), "agents.source_invalid");
    assert_eq!(
        code(&real.join("missing.toml"), &artifact),
        "agents.source_invalid"
    );

    let long = format!("{}.bin", "n".repeat(200));
    let shown = file_name(&format!("/x/{long}"));
    assert!(shown.len() <= MAX_SOURCE_NAME_BYTES);
    assert!(shown.ends_with('…'));
}

#[test]
fn a_local_source_token_is_single_use_and_bound_to_its_plane() {
    let state = AgentsState::default();
    let now = Instant::now();
    let token = state
        .grant_local_source(local(), "/s/craft-spec.toml".into(), "/s/craft".into(), now)
        .unwrap();
    let remote = PlaneId::Remote(Uuid::from_u128(2));
    assert_eq!(
        state
            .take_local_source(token, remote, now)
            .unwrap_err()
            .code,
        "agents.source_expired"
    );
    // Refused through another Plane, still usable through its own.
    assert_eq!(
        state.take_local_source(token, PlaneId::Local, now).unwrap(),
        ("/s/craft-spec.toml".into(), "/s/craft".into())
    );
    assert_eq!(
        state
            .take_local_source(token, PlaneId::Local, now)
            .unwrap_err()
            .code,
        "agents.source_expired"
    );

    let old = state
        .grant_local_source(local(), "a".into(), "b".into(), now)
        .unwrap();
    assert!(state
        .take_local_source(old, PlaneId::Local, now + LOCAL_SOURCE_LIFETIME)
        .is_err());

    let first = state
        .grant_local_source(local(), "a".into(), "b".into(), now)
        .unwrap();
    for n in 1..=LOCAL_SOURCE_CAPACITY as u64 {
        state
            .grant_local_source(
                local(),
                "a".into(),
                "b".into(),
                now + Duration::from_millis(n),
            )
            .unwrap();
    }
    assert_eq!(
        state.local_sources.lock().unwrap().len(),
        LOCAL_SOURCE_CAPACITY
    );
    assert!(state.take_local_source(first, PlaneId::Local, now).is_err());
}

#[tokio::test]
async fn local_files_are_refused_for_a_remote_plane_before_any_dialog() {
    let Setup { bridge, .. } = setup();
    let remote = Uuid::from_u128(0xa).to_string();
    assert_eq!(
        local_plane(&bridge, &remote).unwrap_err().code,
        "agents.local_source_remote"
    );
    assert_eq!(
        local_plane(&bridge, "nonsense").unwrap_err().code,
        "plane.unknown"
    );
    assert_eq!(local_plane(&bridge, "local").unwrap().plane, PlaneId::Local);

    // A token issued for this computer never reaches another Plane.
    let token = bridge
        .agents
        .grant_local_source(
            local(),
            "/s/craft-spec.toml".into(),
            "/s/craft".into(),
            Instant::now(),
        )
        .unwrap();
    let error = discover_craft_for(
        &bridge,
        &remote,
        json!({"type": "local", "source_token": token.to_string()}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "plane.unknown");
    assert!(bridge
        .agents
        .local_sources
        .lock()
        .unwrap()
        .contains_key(&token));
}

#[tokio::test]
async fn agents_inputs_are_refused_without_io() {
    // The local Plane's socket does not exist: any I/O would be offline.
    let Setup { bridge, .. } = setup();
    let code = |result: Result<SettingsReviewView, PublicError>| result.unwrap_err().code;
    assert_eq!(
        code(prepare_craft_disable_for(&bridge, "local", "../x", "wait").await),
        "agents.craft_missing"
    );
    assert_eq!(
        code(prepare_craft_disable_for(&bridge, "local", "codex", "later").await),
        "agents.mode_invalid"
    );
    assert_eq!(
        code(prepare_account_unbind_for(&bridge, "local", "not-a-uuid").await),
        "agents.account_missing"
    );
    assert_eq!(
        code(
            prepare_auto_continue_for(
                &bridge,
                "local",
                &BINDING.to_string(),
                json!({"mode": "retry"})
            )
            .await
        ),
        "agents.policy_invalid"
    );
    assert_eq!(
        code(prepare_account_bind_for(&bridge, "local", &"p".repeat(65)).await),
        "account.provider_unavailable"
    );
    assert_eq!(
        code(
            discover_craft_for(
                &bridge,
                "local",
                json!({"type": "github_release", "repository": "nope", "tag": "v1"})
            )
            .await
        ),
        "agents.source_invalid"
    );
    assert_eq!(
        code(
            discover_craft_for(
                &bridge,
                "local",
                json!({"type": "local", "source_token": Uuid::from_u128(4).to_string()})
            )
            .await
        ),
        "agents.source_expired"
    );
    assert_eq!(
        load_usage_history_for(&bridge, "local", None, 14)
            .await
            .unwrap_err()
            .code,
        "agents.days_invalid"
    );
    assert_eq!(
        load_account_detail_for(&bridge, "local", "x")
            .await
            .unwrap_err()
            .code,
        "agents.account_missing"
    );
}
