use crate::test_support::{actor, events, request, start_core};
use crate::{
	ArtifactLimits, Command, CommandOutcome, RetentionPolicy,
	WorkingTreeRequest,
};
use pretty_assertions::assert_eq;

#[tokio::test]
async fn pressure_rejects_new_runs_without_poisoning_retries_or_reads() {
	let home = tempfile::tempdir().unwrap();
	let core = start_core(&home.path().join("plane.sqlite3")).await;
	let conversation = conversation(&core).await;
	let command = request(Command::CreateRun {
		conversation_id: conversation,
	});
	let before = events(&core).await;
	let core = core.with_artifact_limits(ArtifactLimits {
		free_reserve_bytes: u64::MAX,
		..Default::default()
	});
	assert_eq!(
		core.execute(&actor(), command.clone())
			.await
			.unwrap_err()
			.code,
		"storage.disk_pressure"
	);
	assert_eq!(events(&core).await, before);
	assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 0);
	let core = core.with_artifact_limits(ArtifactLimits::default());
	let outcome = core.execute(&actor(), command.clone()).await.unwrap();
	let core = core.with_artifact_limits(ArtifactLimits {
		free_reserve_bytes: u64::MAX,
		..Default::default()
	});
	assert_eq!(core.execute(&actor(), command).await.unwrap(), outcome);
}

#[tokio::test]
async fn disposable_budget_reserves_concurrent_uploads_and_releases_abandoned_ones()
 {
	use crate::{ArtifactDescriptor, SettingKey, SettingScope, SettingValue};
	let home = tempfile::tempdir().unwrap();
	let core = start_core(&home.path().join("plane.sqlite3")).await;
	let conversation = conversation(&core).await;
	let CommandOutcome::RunCreated(run) = core
		.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: conversation,
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Run")
	};
	core.execute(
		&actor(),
		request(Command::SetSetting {
			key: SettingKey::StorageDisposableMiB,
			scope: SettingScope::Plane,
			value: SettingValue::Count(1),
		}),
	)
	.await
	.unwrap();
	let descriptor = ArtifactDescriptor {
		sha256: "0".repeat(64),
		size: 1024 * 1024,
	};
	let first = core
		.begin_artifact_upload(&actor(), run.run_id, descriptor.clone())
		.await
		.unwrap();
	let other = start_core(&home.path().join("plane.sqlite3")).await;
	let second = other
		.begin_artifact_upload(&actor(), run.run_id, descriptor.clone())
		.await;
	assert_eq!(
		second.err().expect("reserved budget").code,
		"storage.disk_pressure"
	);
	drop(first);
	core.begin_artifact_upload(&actor(), run.run_id, descriptor)
		.await
		.unwrap();
	let mut upload = core.begin_artifact_upload(&actor(), run.run_id,
		ArtifactDescriptor {
			sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
			size: 3,
		}).await.unwrap();
	upload.write_chunk(b"abc").await.unwrap();
	core.execute(
		&actor(),
		request(Command::SetSetting {
			key: SettingKey::StorageDisposableMiB,
			scope: SettingScope::Plane,
			value: SettingValue::Count(0),
		}),
	)
	.await
	.unwrap();
	assert_eq!(
		core.publish_artifact(&actor(), upload)
			.await
			.unwrap_err()
			.code,
		"storage.disk_pressure"
	);
}

#[tokio::test]
async fn pressure_preserves_published_artifacts_and_collects_only_expired_disposable_data()
 {
	use crate::test_support::{
		FixedProbe, ManualClock, equipped, start_core_with,
	};
	use crate::{ArtifactDescriptor, SettingKey, SettingScope, SettingValue};
	let home = tempfile::tempdir().unwrap();
	let path = home.path().join("plane.sqlite3");
	let clock = ManualClock::at(std::time::SystemTime::now());
	let core =
		start_core_with(&path, clock.clone(), FixedProbe::new(equipped()))
			.await;
	let conversation = conversation(&core).await;
	let CommandOutcome::RunCreated(run) = core
		.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: conversation,
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Run")
	};
	let descriptor = ArtifactDescriptor {
		sha256:
			"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
				.into(),
		size: 3,
	};
	let mut upload = core
		.begin_artifact_upload(&actor(), run.run_id, descriptor.clone())
		.await
		.unwrap();
	upload.write_chunk(b"abc").await.unwrap();
	core.publish_artifact(&actor(), upload).await.unwrap();
	core.execute(
		&actor(),
		request(Command::SetSetting {
			key: SettingKey::StorageDisposableMiB,
			scope: SettingScope::Plane,
			value: SettingValue::Count(0),
		}),
	)
	.await
	.unwrap();
	let cache = home.path().join("cache");
	std::fs::create_dir(&cache).unwrap();
	std::fs::write(cache.join("0".repeat(64)), b"cache").unwrap();
	std::fs::write(cache.join("unrecognized"), b"keep").unwrap();
	let protected = home.path().join("workspaces");
	std::fs::create_dir_all(&protected).unwrap();
	std::fs::write(protected.join("dirty"), b"keep").unwrap();
	std::os::unix::fs::symlink(&protected, cache.join("1".repeat(64))).unwrap();
	std::fs::write(
		home.path().join("artifacts/payloads").join("2".repeat(64)),
		b"orphan",
	)
	.unwrap();
	assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 0);
	core.close().await;
	let core =
		start_core_with(&path, clock.clone(), FixedProbe::new(equipped()))
			.await;
	assert_eq!(
		core.begin_artifact_upload(&actor(), run.run_id, descriptor.clone())
			.await
			.err()
			.expect("budget survived restart")
			.code,
		"storage.disk_pressure"
	);
	let core = core.with_artifact_limits(ArtifactLimits {
		free_reserve_bytes: u64::MAX,
		..Default::default()
	});
	clock.advance(std::time::Duration::from_secs(86401));
	assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 2);
	let mut download =
		core.artifact(&actor(), &descriptor.sha256).await.unwrap();
	assert_eq!(download.read_chunk(3).await.unwrap(), b"abc");
	download.finish().unwrap();
	assert_eq!(std::fs::read(protected.join("dirty")).unwrap(), b"keep");
	assert_eq!(std::fs::read(cache.join("unrecognized")).unwrap(), b"keep");
}

async fn conversation(core: &crate::Core) -> crate::ConversationId {
	let CommandOutcome::ConversationCreated(conversation) = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::NoProject,
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Conversation")
	};
	conversation.conversation_id
}
