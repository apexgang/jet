use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn current_diff_remains_readable_when_pressure_prevents_artifact_ingestion()
 {
	let dir = tempfile::tempdir().unwrap();
	let (core, _sender) = start(dir.path()).await;
	let root = dir.path().join("repo");
	let run_id = active_run(&core, &root).await;
	std::fs::write(root.join("README.md"), "Protected work\n").unwrap();
	let objects_before = git(&root, &["count-objects", "-v"]);
	let reader =
		crate::test_support::start_core(&dir.path().join("plane.sqlite3"))
			.await
			.with_artifact_limits(ArtifactLimits {
				free_reserve_bytes: u64::MAX,
				..Default::default()
			});
	let QueryResult::ChangeDiff(diff) = reader
		.query(
			&actor(),
			Query::ChangeDiff {
				run_id,
				scope: DiffScope::Current,
			},
		)
		.await
		.unwrap()
	else {
		panic!("Diff")
	};
	assert_eq!(git(&root, &["count-objects", "-v"]), objects_before);
	assert_eq!(
		diff.artifact.availability,
		ArtifactAvailability::DiskPressure
	);
	assert_eq!(
		std::fs::read_to_string(root.join("README.md")).unwrap(),
		"Protected work\n"
	);
}

#[tokio::test]
async fn query_only_patches_obey_a_zero_disposable_budget() {
	let dir = tempfile::tempdir().unwrap();
	let (core, _sender) = start(dir.path()).await;
	let root = dir.path().join("repo");
	let run_id = active_run(&core, &root).await;
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
	std::fs::write(root.join("README.md"), "Disposable diff\n").unwrap();
	let QueryResult::ChangeDiff(diff) = core
		.query(
			&actor(),
			Query::ChangeDiff {
				run_id,
				scope: DiffScope::Current,
			},
		)
		.await
		.unwrap()
	else {
		panic!("Diff")
	};
	assert_eq!(
		diff.artifact.availability,
		ArtifactAvailability::DiskPressure
	);
	assert_eq!(
		std::fs::read_dir(dir.path().join("cache")).unwrap().count(),
		0
	);
}
