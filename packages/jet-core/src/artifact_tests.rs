use crate::test_support::{actor, events, request, start_core};
use crate::{
	ArtifactDescriptor, Command, CommandOutcome, RetentionPolicy,
	WorkingTreeRequest,
};
use pretty_assertions::assert_eq;

const ABC: &str =
	"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

async fn run(core: &crate::Core) -> crate::RunId {
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
	let CommandOutcome::RunCreated(run) = core
		.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: conversation.conversation_id,
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Run")
	};
	run.run_id
}

#[tokio::test]
async fn collection_preserves_an_active_upload_past_the_grace_period() {
	use crate::test_support::{
		FixedProbe, ManualClock, equipped, start_core_with,
	};
	let home = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(std::time::SystemTime::now());
	let core = start_core_with(
		&home.path().join("plane.sqlite3"),
		clock.clone(),
		FixedProbe::new(equipped()),
	)
	.await;
	let run_id = run(&core).await;
	let descriptor = ArtifactDescriptor {
		sha256: ABC.into(),
		size: 3,
	};
	let mut upload = core
		.begin_artifact_upload(&actor(), run_id, descriptor.clone())
		.await
		.unwrap();
	clock.advance(std::time::Duration::from_secs(86401));
	assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 0);
	upload.write_chunk(b"abc").await.unwrap();
	assert_eq!(
		core.publish_artifact(&actor(), upload).await.unwrap(),
		descriptor
	);
	assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 0);
}

#[tokio::test]
async fn incomplete_mismatched_and_oversized_uploads_never_publish() {
	let home = tempfile::tempdir().unwrap();
	let core = start_core(&home.path().join("plane.sqlite3")).await;
	let run_id = run(&core).await;
	let before = events(&core).await;
	for (bytes, error) in [
		(b"ab".as_slice(), "artifact.size_mismatch"),
		(b"abd".as_slice(), "artifact.hash_mismatch"),
	] {
		let mut upload = core
			.begin_artifact_upload(
				&actor(),
				run_id,
				ArtifactDescriptor {
					sha256: ABC.into(),
					size: 3,
				},
			)
			.await
			.unwrap();
		upload.write_chunk(bytes).await.unwrap();
		assert_eq!(
			core.publish_artifact(&actor(), upload)
				.await
				.unwrap_err()
				.code,
			error
		);
	}
	let mut upload = core
		.begin_artifact_upload(
			&actor(),
			run_id,
			ArtifactDescriptor {
				sha256: ABC.into(),
				size: 3,
			},
		)
		.await
		.unwrap();
	assert_eq!(
		upload.write_chunk(b"abcd").await.unwrap_err().code,
		"artifact.invalid_chunk"
	);
	assert_eq!(
		core.publish_artifact(&actor(), upload)
			.await
			.unwrap_err()
			.code,
		"artifact.size_mismatch"
	);
	let mut interrupted = core
		.begin_artifact_upload(
			&actor(),
			run_id,
			ArtifactDescriptor {
				sha256: ABC.into(),
				size: 3,
			},
		)
		.await
		.unwrap();
	interrupted.write_chunk(b"ab").await.unwrap();
	drop(interrupted);
	assert_eq!(
		core.artifact(&actor(), ABC).await.unwrap_err().code,
		"artifact.not_found"
	);
	assert_eq!(events(&core).await, before);
}

#[tokio::test]
async fn corrupt_storage_is_rejected_on_reads_and_duplicate_publication() {
	let home = tempfile::tempdir().unwrap();
	let core = start_core(&home.path().join("plane.sqlite3")).await;
	let run_id = run(&core).await;
	let descriptor = ArtifactDescriptor {
		sha256: ABC.into(),
		size: 3,
	};
	let mut upload = core
		.begin_artifact_upload(&actor(), run_id, descriptor.clone())
		.await
		.unwrap();
	upload.write_chunk(b"abc").await.unwrap();
	core.publish_artifact(&actor(), upload).await.unwrap();
	let before = events(&core).await;
	// Simulate same-size disk corruption, which a length check cannot detect.
	std::fs::write(home.path().join("artifacts/payloads").join(ABC), b"bad")
		.unwrap();
	let mut download = core.artifact(&actor(), ABC).await.unwrap();
	assert_eq!(download.read_chunk(3).await.unwrap(), b"bad");
	assert_eq!(download.finish().unwrap_err().code, "artifact.corrupt");
	let mut duplicate = core
		.begin_artifact_upload(&actor(), run_id, descriptor)
		.await
		.unwrap();
	duplicate.write_chunk(b"abc").await.unwrap();
	assert_eq!(
		core.publish_artifact(&actor(), duplicate)
			.await
			.unwrap_err()
			.code,
		"artifact.corrupt"
	);
	assert_eq!(events(&core).await, before);
}

#[tokio::test]
async fn collection_removes_crash_orphans_only_after_grace_and_never_follows_links()
 {
	use crate::test_support::{
		FixedProbe, ManualClock, equipped, start_core_with,
	};
	let home = tempfile::tempdir().unwrap();
	let clock = ManualClock::at(std::time::SystemTime::now());
	let core = start_core_with(
		&home.path().join("plane.sqlite3"),
		clock.clone(),
		FixedProbe::new(equipped()),
	)
	.await;
	assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 0);
	let directory = home.path().join("artifacts/payloads");
	std::fs::write(directory.join(ABC), b"abc").unwrap();
	std::fs::write(
		directory.join(format!(".pending-{}", uuid::Uuid::new_v4())),
		b"interrupted",
	)
	.unwrap();
	let protected = home.path().join("protected");
	std::fs::write(&protected, b"keep").unwrap();
	std::os::unix::fs::symlink(&protected, directory.join("0".repeat(64)))
		.unwrap();
	assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 0);
	clock.advance(std::time::Duration::from_secs(86401));
	assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 2);
	assert_eq!(core.collect_artifacts(&actor()).await.unwrap(), 0);
	assert_eq!(std::fs::read(protected).unwrap(), b"keep");
	assert_eq!(
		core.artifact(&actor(), ABC).await.unwrap_err().code,
		"artifact.not_found"
	);
}

#[tokio::test]
async fn run_budget_counts_new_bytes_and_rejects_new_content_after_restart() {
	let home = tempfile::tempdir().unwrap();
	let limits = crate::ArtifactLimits {
		artifact_bytes: 3,
		run_bytes: 3,
		..Default::default()
	};
	let core = start_core(&home.path().join("plane.sqlite3"))
		.await
		.with_artifact_limits(limits);
	let run_id = run(&core).await;
	let descriptor = ArtifactDescriptor {
		sha256: ABC.into(),
		size: 3,
	};
	for _ in 0..2 {
		let mut upload = core
			.begin_artifact_upload(&actor(), run_id, descriptor.clone())
			.await
			.unwrap();
		upload.write_chunk(b"abc").await.unwrap();
		assert_eq!(
			core.publish_artifact(&actor(), upload).await.unwrap(),
			descriptor
		);
	}
	assert!(
		matches!(core.begin_artifact_upload(&actor(), run_id, ArtifactDescriptor { sha256: ABC.into(), size: 4 }).await, Err(e) if e.code == "artifact.size_exceeded")
	);
	core.close().await;
	let core = start_core(&home.path().join("plane.sqlite3"))
		.await
		.with_artifact_limits(limits);
	let mut upload = core.begin_artifact_upload(&actor(), run_id, ArtifactDescriptor { sha256: "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb".into(), size: 1 }).await.unwrap();
	upload.write_chunk(b"a").await.unwrap();
	assert_eq!(
		core.publish_artifact(&actor(), upload)
			.await
			.unwrap_err()
			.code,
		"artifact.run_budget_exceeded"
	);
}

#[tokio::test]
async fn publication_exposes_verified_bytes_and_one_durable_run_reference() {
	let home = tempfile::tempdir().unwrap();
	let core = start_core(&home.path().join("plane.sqlite3")).await;
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
	let CommandOutcome::RunCreated(run) = core
		.execute(
			&actor(),
			request(Command::CreateRun {
				conversation_id: conversation.conversation_id,
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Run")
	};
	let descriptor = ArtifactDescriptor {
		sha256: ABC.into(),
		size: 3,
	};
	let mut upload = core
		.begin_artifact_upload(&actor(), run.run_id, descriptor.clone())
		.await
		.unwrap();
	upload.write_chunk(b"abc").await.unwrap();
	assert_eq!(
		core.artifact(&actor(), ABC).await.unwrap_err().code,
		"artifact.not_found"
	);
	assert_eq!(
		core.publish_artifact(&actor(), upload).await.unwrap(),
		descriptor
	);
	let before = events(&core).await;
	let mut duplicate = core
		.begin_artifact_upload(&actor(), run.run_id, descriptor.clone())
		.await
		.unwrap();
	duplicate.write_chunk(b"abc").await.unwrap();
	assert_eq!(
		core.publish_artifact(&actor(), duplicate).await.unwrap(),
		descriptor
	);
	assert_eq!(events(&core).await, before);
	let mut download = core.artifact(&actor(), ABC).await.unwrap();
	assert_eq!(download.descriptor(), &descriptor);
	assert_eq!(download.read_chunk(2).await.unwrap(), b"ab");
	assert_eq!(download.read_chunk(2).await.unwrap(), b"c");
	assert_eq!(download.read_chunk(2).await.unwrap(), b"");
	download.finish().unwrap();
	core.close().await;
	let core = start_core(&home.path().join("plane.sqlite3")).await;
	assert_eq!(
		core.artifact(&actor(), ABC).await.unwrap().descriptor(),
		&descriptor
	);
}
