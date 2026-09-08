use jet_store::audit_head_path;
use pretty_assertions::assert_eq;
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime};

use crate::test_support::{
	FixedCraftRepository, actor, request, start_core_installing,
};
use crate::{
	AuditSequence, BrokerPermission, CapabilityObservation, Command,
	CommandEnvelope, CommandId, CommandOutcome, CraftHostAccess, CraftSource,
	Query, QueryResult, SettingKey, SettingScope, SettingValue,
};

fn published_specification(sha256: &str) -> Vec<u8> {
	format!(
		r#"schema = {{ major = 1, minor = 0 }}
id = "demo"
harness = "demo"
version = "1.2.3"
publisher = "Example Org"
broker_permissions = ["artifact_read"]
host_access = [{{ kind = "network", destination = "api.example.com" }}]
features = [{{ name = "turns", required = true }}, {{ name = "future-ui" }}]

[protocol]
family = "craft"
versions = [{{ major = 1, minor = 5 }}]
capabilities = ["runs"]

[[artifacts]]
name = "jet-craft-demo-linux-aarch64"
operating_system = "linux"
architecture = "aarch64"
sha256 = "{sha256}"
"#
	)
	.into_bytes()
}

async fn preview(
	core: &crate::Core,
	source: CraftSource,
) -> crate::CraftInstallationPreview {
	let result = core
		.query(&actor(), Query::DiscoverCraft { source })
		.await
		.unwrap();
	let QueryResult::CraftInstallationPreview(preview) = result else {
		panic!("unexpected result {result:?}");
	};
	*preview
}

#[tokio::test]
async fn a_qualifying_release_is_discovered_as_an_exact_confirmation() {
	let dir = tempfile::tempdir().unwrap();
	let artifact = b"verified executable".to_vec();
	let sha256 = format!("{:x}", Sha256::digest(&artifact));
	let repository = FixedCraftRepository::release(
		"apex/jet-craft-demo",
		"v1.2.3",
		"0123456789abcdef0123456789abcdef01234567",
		published_specification(&sha256),
		"jet-craft-demo-linux-aarch64",
		sha256.clone(),
		artifact,
	);
	let core =
		start_core_installing(&dir.path().join("plane.sqlite3"), repository)
			.await;
	let source = CraftSource::GitHubRelease {
		repository: "apex/jet-craft-demo".into(),
		tag: "v1.2.3".into(),
	};

	let result = core
		.query(
			&actor(),
			Query::DiscoverCraft {
				source: source.clone(),
			},
		)
		.await
		.unwrap();
	let QueryResult::CraftInstallationPreview(preview) = result else {
		panic!("unexpected result {result:?}");
	};

	assert_eq!(preview.craft_id, "demo");
	assert_eq!(preview.version, "1.2.3");
	assert_eq!(preview.enabled_features, vec!["turns"]);
	assert_eq!(
		preview.confirmation(),
		crate::CraftInstallationConfirmation {
			source,
			repository: "https://github.com/apex/jet-craft-demo".into(),
			publisher_claim: "Example Org".into(),
			commit: "0123456789abcdef0123456789abcdef01234567".into(),
			artifact_sha256: sha256,
			broker_permissions: vec![BrokerPermission::ArtifactRead],
			host_access: vec![CraftHostAccess::Network {
				destination: "api.example.com".into(),
			}],
			trust: crate::CraftTrust::SameUserExecutable,
		},
	);
}

#[tokio::test]
async fn an_exact_confirmation_publishes_an_audited_installed_craft() {
	let dir = tempfile::tempdir().unwrap();
	let artifact = b"verified executable".to_vec();
	let sha256 = format!("{:x}", Sha256::digest(&artifact));
	let repository = FixedCraftRepository::release(
		"apex/jet-craft-demo",
		"v1.2.3",
		"0123456789abcdef0123456789abcdef01234567",
		published_specification(&sha256),
		"jet-craft-demo-linux-aarch64",
		sha256.clone(),
		artifact,
	);
	let core =
		start_core_installing(&dir.path().join("plane.sqlite3"), repository)
			.await;
	let proposal = preview(
		&core,
		CraftSource::GitHubRelease {
			repository: "apex/jet-craft-demo".into(),
			tag: "v1.2.3".into(),
		},
	)
	.await;

	let command_id = uuid::Uuid::now_v7();
	let confirmation = proposal.confirmation();
	let outcome = core
		.execute(
			&actor(),
			CommandEnvelope::new(
				CommandId(command_id),
				Command::InstallCraft {
					confirmation: confirmation.clone(),
				},
				b"confirmed Craft installation",
			)
			.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(
		outcome,
		CommandOutcome::CraftInstallationQueued {
			craft_id: "demo".into(),
			version: "1.2.3".into(),
			artifact_sha256: sha256.clone(),
		}
	);
	assert!(dir.path().join("crafts/artifacts").join(&sha256).is_file());
	assert!(!dir.path().join("crafts/demo.json").exists());
	let replay = core
		.execute(
			&actor(),
			CommandEnvelope::new(
				CommandId(command_id),
				Command::InstallCraft { confirmation },
				b"confirmed Craft installation",
			)
			.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(replay, outcome);
	core.perform_craft_installations().await.unwrap();

	let capabilities = core
		.query(
			&actor(),
			Query::Capabilities {
				observation: CapabilityObservation::Fresh,
			},
		)
		.await
		.unwrap();
	let QueryResult::Capabilities(capabilities) = capabilities else {
		panic!("unexpected result {capabilities:?}");
	};
	assert!(capabilities.crafts.iter().any(|craft| {
		craft.craft.0 == "demo"
			&& craft.version == "1.2.3"
			&& craft.harnesses == vec![crate::HarnessId("demo".into())]
	}));
	let audit = core
		.query(
			&actor(),
			Query::SecurityAudit {
				after: AuditSequence(0),
			},
		)
		.await
		.unwrap();
	let QueryResult::SecurityAudit(audit) = audit else {
		panic!("unexpected result {audit:?}");
	};
	assert_eq!(
		audit.entries.last().unwrap().decision,
		"craft.installation_approved"
	);
	assert_eq!(audit.entries.last().unwrap().target.kind, "craft");
	assert_eq!(
		audit.entries.last().unwrap().target.identity.as_deref(),
		Some("demo")
	);
	crate::craft_artifact_collection::collect_unreferenced(
		&core.store,
		dir.path().join("crafts"),
		SystemTime::now() + Duration::from_secs(25 * 60 * 60),
	)
	.await
	.unwrap();
	assert!(dir.path().join("crafts/artifacts").join(&sha256).exists());
}

#[tokio::test]
async fn unreferenced_craft_artifacts_are_collected_only_after_a_grace_period()
{
	let dir = tempfile::tempdir().unwrap();
	let core = start_core_installing(
		&dir.path().join("plane.sqlite3"),
		FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			Vec::new(),
			"artifact",
			"ab".repeat(32),
			Vec::new(),
		),
	)
	.await;
	let home = dir.path().join("crafts");
	let artifacts = home.join("artifacts");
	let staging = home.join("staging");
	std::fs::create_dir_all(&artifacts).unwrap();
	std::fs::create_dir_all(&staging).unwrap();
	let orphan = artifacts.join("ab".repeat(32));
	let interrupted = staging.join(format!("{}.artifact", uuid::Uuid::nil()));
	std::fs::write(&orphan, b"orphan").unwrap();
	std::fs::write(&interrupted, b"partial").unwrap();

	crate::craft_artifact_collection::collect_unreferenced(
		&core.store,
		home.clone(),
		SystemTime::now(),
	)
	.await
	.unwrap();
	assert!(orphan.exists());
	assert!(interrupted.exists());
	crate::craft_artifact_collection::collect_unreferenced(
		&core.store,
		home,
		SystemTime::now() + Duration::from_secs(25 * 60 * 60),
	)
	.await
	.unwrap();
	assert!(!orphan.exists());
	assert!(!interrupted.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn craft_collection_does_not_follow_an_artifact_directory_symlink() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core_installing(
		&dir.path().join("plane.sqlite3"),
		FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			Vec::new(),
			"artifact",
			"ab".repeat(32),
			Vec::new(),
		),
	)
	.await;
	let home = dir.path().join("crafts");
	let outside = dir.path().join("outside");
	std::fs::create_dir_all(&home).unwrap();
	std::fs::create_dir_all(&outside).unwrap();
	let outside_artifact = outside.join("ab".repeat(32));
	std::fs::write(&outside_artifact, b"outside").unwrap();
	std::os::unix::fs::symlink(&outside, home.join("artifacts")).unwrap();

	let _ = crate::craft_artifact_collection::collect_unreferenced(
		&core.store,
		home,
		SystemTime::now() + Duration::from_secs(25 * 60 * 60),
	)
	.await;

	assert!(outside_artifact.exists());
}

#[tokio::test]
async fn an_unknown_required_declaration_rejects_the_release() {
	let dir = tempfile::tempdir().unwrap();
	let artifact = b"verified executable".to_vec();
	let sha256 = format!("{:x}", Sha256::digest(&artifact));
	let specification = String::from_utf8(published_specification(&sha256))
		.unwrap()
		.replace(
			"{ name = \"future-ui\" }",
			"{ name = \"future-ui\", required = true }",
		)
		.into_bytes();
	let repository = FixedCraftRepository::release(
		"apex/jet-craft-demo",
		"v1.2.3",
		"0123456789abcdef0123456789abcdef01234567",
		specification,
		"jet-craft-demo-linux-aarch64",
		sha256,
		artifact,
	);
	let core =
		start_core_installing(&dir.path().join("plane.sqlite3"), repository)
			.await;

	let error = core
		.query(
			&actor(),
			Query::DiscoverCraft {
				source: CraftSource::GitHubRelease {
					repository: "apex/jet-craft-demo".into(),
					tag: "v1.2.3".into(),
				},
			},
		)
		.await
		.unwrap_err();

	assert_eq!(error.code, "craft.release_invalid");
}

#[tokio::test]
async fn an_incompatible_or_ambiguous_craft_protocol_rejects_the_release() {
	for versions in [
		"[{ major = 99, minor = 5 }]",
		"[{ major = 1, minor = 5 }, { major = 1, minor = 4 }]",
	] {
		let dir = tempfile::tempdir().unwrap();
		let artifact = b"verified executable".to_vec();
		let sha256 = format!("{:x}", Sha256::digest(&artifact));
		let specification = String::from_utf8(published_specification(&sha256))
			.unwrap()
			.replace("[{ major = 1, minor = 5 }]", versions)
			.into_bytes();
		let repository = FixedCraftRepository::release(
			"apex/jet-craft-demo",
			"v1.2.3",
			"0123456789abcdef0123456789abcdef01234567",
			specification,
			"jet-craft-demo-linux-aarch64",
			sha256,
			artifact,
		);
		let core = start_core_installing(
			&dir.path().join("plane.sqlite3"),
			repository,
		)
		.await;

		let error = core
			.query(
				&actor(),
				Query::DiscoverCraft {
					source: CraftSource::GitHubRelease {
						repository: "apex/jet-craft-demo".into(),
						tag: "v1.2.3".into(),
					},
				},
			)
			.await
			.unwrap_err();

		assert_eq!(error.code, "craft.release_invalid");
	}
}

#[tokio::test]
async fn an_unknown_additive_specification_field_does_not_disable_the_craft() {
	let dir = tempfile::tempdir().unwrap();
	let artifact = b"verified executable".to_vec();
	let sha256 = format!("{:x}", Sha256::digest(&artifact));
	let specification = String::from_utf8(published_specification(&sha256))
		.unwrap()
		.replace(
			"schema = { major = 1, minor = 0 }",
			"schema = { major = 1, minor = 0, patch = 7 }",
		)
		.replace(
			"{ name = \"turns\", required = true }",
			"{ name = \"turns\", required = true, presentation = \"compact\" }",
		)
		.replace(
			"[protocol]",
			"future_presentation = \"compact\"\n\n[protocol]\ntransport = \"stdio\"",
		)
		.replace(
			"{ major = 1, minor = 5 }",
			"{ major = 1, minor = 5, revision = 2 }",
		)
		.replace(
			"architecture = \"aarch64\"",
			"architecture = \"aarch64\"\ncontent_type = \"application/octet-stream\"",
		)
		.into_bytes();
	let repository = FixedCraftRepository::release(
		"apex/jet-craft-demo",
		"v1.2.3",
		"0123456789abcdef0123456789abcdef01234567",
		specification,
		"jet-craft-demo-linux-aarch64",
		sha256,
		artifact,
	);
	let core =
		start_core_installing(&dir.path().join("plane.sqlite3"), repository)
			.await;

	let proposal = preview(
		&core,
		CraftSource::GitHubRelease {
			repository: "apex/jet-craft-demo".into(),
			tag: "v1.2.3".into(),
		},
	)
	.await;

	assert_eq!(proposal.enabled_features, vec!["turns"]);
}

#[tokio::test]
async fn local_artifacts_require_developer_mode_and_remain_explicitly_unverified()
 {
	let dir = tempfile::tempdir().unwrap();
	let artifact = b"locally built executable".to_vec();
	let sha256 = format!("{:x}", Sha256::digest(&artifact));
	let specification_path = dir.path().join("craft-spec.toml");
	let artifact_path = dir.path().join("jet-craft-demo-linux-aarch64");
	std::fs::write(&specification_path, published_specification(&sha256))
		.unwrap();
	std::fs::write(&artifact_path, &artifact).unwrap();
	let repository = FixedCraftRepository::release(
		"apex/jet-craft-demo",
		"v1.2.3",
		"0123456789abcdef0123456789abcdef01234567",
		published_specification(&sha256),
		"jet-craft-demo-linux-aarch64",
		sha256.clone(),
		artifact,
	);
	let core =
		start_core_installing(&dir.path().join("plane.sqlite3"), repository)
			.await;
	let source = CraftSource::Local {
		specification: specification_path,
		artifact: artifact_path,
	};

	let refused = core
		.query(
			&actor(),
			Query::DiscoverCraft {
				source: source.clone(),
			},
		)
		.await
		.unwrap_err();
	assert_eq!(refused.code, "craft.developer_mode_required");
	core.execute(
		&actor(),
		request(Command::SetSetting {
			key: SettingKey::DeveloperMode,
			scope: SettingScope::Plane,
			value: SettingValue::Flag(true),
		}),
	)
	.await
	.unwrap();

	let proposal = preview(&core, source).await;
	assert_eq!(
		proposal.confirmation().trust,
		crate::CraftTrust::DeveloperSource
	);
	assert_eq!(proposal.confirmation().commit, "developer-source");
	core.execute(
		&actor(),
		request(Command::InstallCraft {
			confirmation: proposal.confirmation(),
		}),
	)
	.await
	.unwrap();
	core.perform_craft_installations().await.unwrap();
	let QueryResult::Capabilities(capabilities) = core
		.query(
			&actor(),
			Query::Capabilities {
				observation: CapabilityObservation::Fresh,
			},
		)
		.await
		.unwrap()
	else {
		panic!("unexpected capabilities result");
	};
	assert!(
		capabilities
			.crafts
			.iter()
			.any(|craft| craft.craft.0 == "demo")
	);
}

#[cfg(unix)]
#[tokio::test]
async fn developer_mode_does_not_follow_a_local_artifact_symlink() {
	let dir = tempfile::tempdir().unwrap();
	let artifact = b"locally built executable".to_vec();
	let sha256 = format!("{:x}", Sha256::digest(&artifact));
	let specification_path = dir.path().join("craft-spec.toml");
	let real_directory = dir.path().join("real");
	std::fs::create_dir(&real_directory).unwrap();
	let real_artifact_path =
		real_directory.join("jet-craft-demo-linux-aarch64");
	let artifact_path = dir.path().join("jet-craft-demo-linux-aarch64");
	std::fs::write(&specification_path, published_specification(&sha256))
		.unwrap();
	std::fs::write(&real_artifact_path, artifact).unwrap();
	std::os::unix::fs::symlink(&real_artifact_path, &artifact_path).unwrap();
	let repository = FixedCraftRepository::release(
		"apex/jet-craft-demo",
		"v1.2.3",
		"0123456789abcdef0123456789abcdef01234567",
		published_specification(&sha256),
		"jet-craft-demo-linux-aarch64",
		sha256,
		Vec::new(),
	);
	let core =
		start_core_installing(&dir.path().join("plane.sqlite3"), repository)
			.await;
	core.execute(
		&actor(),
		request(Command::SetSetting {
			key: SettingKey::DeveloperMode,
			scope: SettingScope::Plane,
			value: SettingValue::Flag(true),
		}),
	)
	.await
	.unwrap();

	let error = core
		.query(
			&actor(),
			Query::DiscoverCraft {
				source: CraftSource::Local {
					specification: specification_path,
					artifact: artifact_path,
				},
			},
		)
		.await
		.unwrap_err();

	assert_eq!(error.code, "craft.local_file_invalid");
}

#[tokio::test]
async fn local_installation_rechecks_developer_mode_in_its_transaction() {
	let dir = tempfile::tempdir().unwrap();
	let artifact = b"locally built executable".to_vec();
	let sha256 = format!("{:x}", Sha256::digest(&artifact));
	let specification_path = dir.path().join("craft-spec.toml");
	let artifact_path = dir.path().join("jet-craft-demo-linux-aarch64");
	std::fs::write(&specification_path, published_specification(&sha256))
		.unwrap();
	std::fs::write(&artifact_path, artifact).unwrap();
	let repository = FixedCraftRepository::release(
		"apex/jet-craft-demo",
		"v1.2.3",
		"0123456789abcdef0123456789abcdef01234567",
		published_specification(&sha256),
		"jet-craft-demo-linux-aarch64",
		sha256,
		Vec::new(),
	);
	let core =
		start_core_installing(&dir.path().join("plane.sqlite3"), repository)
			.await;
	core.execute(
		&actor(),
		request(Command::SetSetting {
			key: SettingKey::DeveloperMode,
			scope: SettingScope::Plane,
			value: SettingValue::Flag(true),
		}),
	)
	.await
	.unwrap();
	let proposal = preview(
		&core,
		CraftSource::Local {
			specification: specification_path,
			artifact: artifact_path,
		},
	)
	.await;
	let prepared =
		crate::craft_installation::prepare(&core, &proposal.confirmation())
			.await
			.unwrap();
	core.execute(
		&actor(),
		request(Command::SetSetting {
			key: SettingKey::DeveloperMode,
			scope: SettingScope::Plane,
			value: SettingValue::Flag(false),
		}),
	)
	.await
	.unwrap();
	let now_unix_ms = core.now_unix_ms();

	let error = core
		.store
		.write(async |tx| {
			crate::craft_installation::record(
				tx,
				&actor(),
				CommandId(uuid::Uuid::now_v7()),
				prepared,
				now_unix_ms,
			)
			.await
		})
		.await
		.unwrap_err();

	assert_eq!(error.code, "craft.developer_mode_required");
}

#[tokio::test]
async fn a_degraded_plane_does_not_stage_a_craft_before_refusing_it() {
	let dir = tempfile::tempdir().unwrap();
	let store_path = dir.path().join("plane.sqlite3");
	let artifact = b"locally built executable".to_vec();
	let sha256 = format!("{:x}", Sha256::digest(&artifact));
	let specification_path = dir.path().join("craft-spec.toml");
	let artifact_path = dir.path().join("jet-craft-demo-linux-aarch64");
	std::fs::write(&specification_path, published_specification(&sha256))
		.unwrap();
	std::fs::write(&artifact_path, artifact.clone()).unwrap();
	let repository = FixedCraftRepository::release(
		"apex/jet-craft-demo",
		"v1.2.3",
		"0123456789abcdef0123456789abcdef01234567",
		published_specification(&sha256),
		"jet-craft-demo-linux-aarch64",
		sha256,
		artifact,
	);
	let trusted = start_core_installing(&store_path, repository.clone()).await;
	trusted
		.execute(
			&actor(),
			request(Command::SetSetting {
				key: SettingKey::DeveloperMode,
				scope: SettingScope::Plane,
				value: SettingValue::Flag(true),
			}),
		)
		.await
		.unwrap();
	let proposal = preview(
		&trusted,
		CraftSource::Local {
			specification: specification_path,
			artifact: artifact_path,
		},
	)
	.await;
	trusted.close().await;
	drop(trusted);
	std::fs::remove_file(audit_head_path(&store_path)).unwrap();
	let degraded = start_core_installing(&store_path, repository).await;

	let error = degraded
		.execute(
			&actor(),
			request(Command::InstallCraft {
				confirmation: proposal.confirmation(),
			}),
		)
		.await
		.unwrap_err();

	assert_eq!(error.code, "security.audit_degraded");
	assert!(!dir.path().join("crafts").exists());
}

#[tokio::test]
async fn changed_confirmation_and_changed_artifact_are_both_refused() {
	let dir = tempfile::tempdir().unwrap();
	let declared_artifact = b"declared executable".to_vec();
	let sha256 = format!("{:x}", Sha256::digest(&declared_artifact));
	let repository = FixedCraftRepository::release(
		"apex/jet-craft-demo",
		"v1.2.3",
		"0123456789abcdef0123456789abcdef01234567",
		published_specification(&sha256),
		"jet-craft-demo-linux-aarch64",
		sha256,
		b"changed after publication".to_vec(),
	);
	let core =
		start_core_installing(&dir.path().join("plane.sqlite3"), repository)
			.await;
	let proposal = preview(
		&core,
		CraftSource::GitHubRelease {
			repository: "apex/jet-craft-demo".into(),
			tag: "v1.2.3".into(),
		},
	)
	.await;
	let mut changed_confirmation = proposal.confirmation();
	changed_confirmation.publisher_claim = "Impostor".into();

	let changed = core
		.execute(
			&actor(),
			request(Command::InstallCraft {
				confirmation: changed_confirmation,
			}),
		)
		.await
		.unwrap_err();
	assert_eq!(changed.code, "craft.installation_stale");
	let artifact = core
		.execute(
			&actor(),
			request(Command::InstallCraft {
				confirmation: proposal.confirmation(),
			}),
		)
		.await
		.unwrap_err();
	assert_eq!(artifact.code, "craft.artifact_mismatch");
}
