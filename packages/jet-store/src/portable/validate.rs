//! Checks the rows a Recovered copy retains, beyond SQLite page integrity.
use crate::StoreError;
use sqlx::SqliteConnection;

pub(super) async fn validate(
	connection: &mut SqliteConnection,
) -> Result<(), StoreError> {
	// PRAGMAs cannot be described by SQLx macros. integrity_check alone does
	// not check foreign keys (ASVS 2.2.2, 2.3.3).
	if !sqlx::query("PRAGMA foreign_key_check")
		.fetch_all(&mut *connection)
		.await?
		.is_empty()
	{
		return Err(invalid());
	}
	let invalid_events = sqlx::query_scalar!(r#"
        SELECT COUNT(*) FROM events e WHERE
            payload_version != 1 OR NOT json_valid(payload)
            OR NOT EXISTS (SELECT 1 FROM conversations c WHERE c.conversation_id = e.conversation_id)
            OR (run_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.run_id = e.run_id AND r.conversation_id = e.conversation_id))
            OR CASE kind
                WHEN 'turn.input' THEN json_type(payload, '$.text') IS NOT 'text' OR json_type(payload, '$.turn_id') IS NOT 'text'
                WHEN 'run.output' THEN json_type(payload, '$.native_json') IS NOT 'text' OR json_type(payload, '$.presentation_json') IS NOT 'array'
                    OR EXISTS (SELECT 1 FROM json_each(payload, '$.presentation_json') WHERE type != 'text')
                WHEN 'artifact.published' THEN NOT EXISTS (
                    SELECT 1 FROM artifact_references a WHERE a.run_id = e.run_id
                    AND a.sha256 = json_extract(e.payload, '$.artifact.sha256')
                    AND a.size = json_extract(e.payload, '$.artifact.size'))
                ELSE 1 END
        "#).fetch_one(&mut *connection).await?;
	let invalid_settings = sqlx::query_scalar!("SELECT COUNT(*) FROM settings s WHERE scope = 'conversation' AND NOT EXISTS (SELECT 1 FROM conversations c WHERE c.conversation_id = s.scope_id)").fetch_one(&mut *connection).await?;
	let invalid_schedules = sqlx::query_scalar!(
		r#"
        SELECT COUNT(*) FROM scheduled_tasks WHERE
            json_type(state, '$.prompt') IS NOT 'text'
            OR json_type(state, '$.time_zone') IS NOT 'text'
            OR json_type(state, '$.local_time') IS NOT 'text'
            OR json_extract(state, '$.conversation_id') IS NOT conversation_id
            OR json_extract(state, '$.schedule_id') IS NOT schedule_id
        "#
	)
	.fetch_one(&mut *connection)
	.await?;
	if invalid_events != 0 || invalid_settings != 0 || invalid_schedules != 0 {
		return Err(invalid());
	}
	Ok(())
}
fn invalid() -> StoreError {
	StoreError::Integrity(
		"incompatible or inconsistent Recovery content".into(),
	)
}

pub(super) async fn columns(
	connection: &mut SqliteConnection,
) -> Result<(), StoreError> {
	// Bootstrap schema introspection fails closed if a retained table gains
	// a column whose portability has not been reviewed (ASVS 14.2.4).
	const COLUMNS: &[(&str, &[&str])] = &[
		(
			"conversations",
			&[
				"conversation_id",
				"retention",
				"created_at_unix_ms",
				"working_tree",
				"project_id",
				"import_id",
				"name",
				"name_source",
				"revision",
				"fork_source_conversation_id",
				"fork_source_run_id",
				"fork_checkpoint_turn",
				"authority",
				"authority_epoch",
			],
		),
		(
			"runs",
			&[
				"run_id",
				"conversation_id",
				"lifecycle",
				"created_at_unix_ms",
				"ended_at_unix_ms",
				"revision",
				"name",
				"name_source",
			],
		),
		(
			"events",
			&[
				"sequence",
				"event_id",
				"actor_kind",
				"actor_id",
				"recorded_at_unix_ms",
				"conversation_id",
				"run_id",
				"kind",
				"payload_version",
				"payload",
				"class",
			],
		),
		(
			"settings",
			&["key", "scope", "scope_id", "value", "updated_at_unix_ms"],
		),
		(
			"scheduled_tasks",
			&[
				"schedule_id",
				"conversation_id",
				"next_due_unix_ms",
				"state",
			],
		),
		("artifact_references", &["run_id", "sha256", "size"]),
	];
	for (table, expected) in COLUMNS {
		let actual: Vec<String> =
			sqlx::query_scalar("SELECT name FROM pragma_table_info(?1)")
				.bind(table)
				.fetch_all(&mut *connection)
				.await?;
		if actual != *expected {
			return Err(invalid());
		}
	}
	Ok(())
}
