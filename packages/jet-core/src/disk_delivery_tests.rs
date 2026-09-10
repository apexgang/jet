use super::*;
use crate::run_state::Observation;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn recovered_space_cannot_make_an_uncaptured_boundary_safe_for_delivery()
{
	let dir = tempfile::tempdir().unwrap();
	let f = TurnFixture::start(dir.path(), &[]).await;
	let head = git(&f.root, &["rev-parse", "HEAD"]);
	std::fs::write(f.root.join("README.md"), "Pre-existing user edit\n")
		.unwrap();
	// Record real turn boundaries through a Core whose reserve cannot be met.
	let pressured = start_core(&dir.path().join("plane.sqlite3"))
		.await
		.with_artifact_limits(ArtifactLimits {
			free_reserve_bytes: u64::MAX,
			..Default::default()
		});
	pressured
		.observe_run(f.run_id, Observation::TurnEnded(TurnOutcome::Completed))
		.await
		.unwrap();
	pressured
		.observe_run(f.run_id, Observation::TurnStarted)
		.await
		.unwrap();
	f.core
		.execute(
			&actor(),
			request(Command::SetSetting {
				key: SettingKey::GitAutoCommit,
				scope: SettingScope::Project {
					project_id: f.project_id,
				},
				value: SettingValue::Flag(true),
			}),
		)
		.await
		.unwrap();
	std::fs::write(f.root.join("agent.txt"), "Agent edit after recovery\n")
		.unwrap();
	f.finish(TurnOutcome::Completed).await;
	for scope in [
		DiffScope::Turn { turn: 2 },
		DiffScope::Historical {
			from_turn: 1,
			to_turn: 2,
		},
	] {
		let QueryResult::ChangeDiff(diff) = f
			.core
			.query(
				&actor(),
				Query::ChangeDiff {
					run_id: f.run_id,
					scope,
				},
			)
			.await
			.unwrap()
		else {
			panic!("Diff")
		};
		assert_eq!(
			(
				diff.before.uncommitted.availability,
				diff.after.uncommitted.availability,
				diff.artifact.availability,
				diff.patch,
				diff.patch_truncated
			),
			(
				ArtifactAvailability::DiskPressure,
				ArtifactAvailability::Stored,
				ArtifactAvailability::DiskPressure,
				String::new(),
				true
			)
		);
	}
	f.core.perform_git_deliveries().await.unwrap();
	let outcomes: Vec<_> = deliveries(&f.core, f.conversation_id)
		.await
		.into_iter()
		.map(|delivery| delivery.outcome)
		.collect();
	assert_eq!(
		outcomes,
		vec![GitDeliveryOutcome::Failed {
			code: "git.incomplete_or_dirty_baseline".into(),
		}]
	);
	assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), head);
	assert_eq!(
		std::fs::read_to_string(f.root.join("README.md")).unwrap(),
		"Pre-existing user edit\n"
	);
}
