//! Removing a Conversation and every Jet-owned row it reaches (ADR-0011).
//!
//! The removal takes the Runs, Workspace, Git delivery, schedules, Usage,
//! search documents, and journal Events of one Conversation, and moves the
//! journal's replay floor past the Events it removed, so a cursor from
//! before the deletion is told to take a fresh snapshot rather than
//! replayed across a gap (ADR-0078). Reapplying the Deletion ledger at open
//! runs the same statements, so a restored snapshot loses exactly what the
//! deletion removed (ADR-0102).

use crate::StoreError;
use sqlx::SqliteConnection;

/// Removes every row of the Conversation `id`, dependents first. Reapplying
/// the ledger at open uses the same statements, so a restored snapshot
/// loses exactly what the deletion removed.
pub(crate) async fn purge_rows(
	connection: &mut SqliteConnection,
	id: &str,
) -> Result<u64, StoreError> {
	let mut changed = 0;
	// The Workspace and what hangs off it.
	changed += sqlx::query!(
		"DELETE FROM workspace_promotion_conflicts WHERE promotion_id IN (
			SELECT p.promotion_id FROM workspace_promotions p
			JOIN workspaces w ON w.workspace_id = p.workspace_id
			WHERE w.conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM effects WHERE promotion_id IN (
			SELECT p.promotion_id FROM workspace_promotions p
			JOIN workspaces w ON w.workspace_id = p.workspace_id
			WHERE w.conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM workspace_promotions WHERE workspace_id IN (
			SELECT workspace_id FROM workspaces WHERE conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM effects WHERE terminal_id IN (
			SELECT t.terminal_id FROM workspace_terminals t
			JOIN workspaces w ON w.workspace_id = t.workspace_id
			WHERE w.conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM workspace_terminals WHERE workspace_id IN (
			SELECT workspace_id FROM workspaces WHERE conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed +=
		sqlx::query!("DELETE FROM workspaces WHERE conversation_id = ?1", id)
			.execute(&mut *connection)
			.await?
			.rows_affected();
	// Git delivery, whose Effects are keyed by delivery rather than Run.
	changed += sqlx::query!(
		"DELETE FROM effects WHERE effect_id IN (
			SELECT delivery_id FROM git_deliveries WHERE conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM git_deliveries WHERE conversation_id = ?1",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed +=
		sqlx::query!("DELETE FROM git_drafts WHERE conversation_id = ?1", id)
			.execute(&mut *connection)
			.await?
			.rows_affected();
	changed +=
		sqlx::query!("DELETE FROM git_branches WHERE conversation_id = ?1", id)
			.execute(&mut *connection)
			.await?
			.rows_affected();
	// The Runs and what hangs off them.
	changed += sqlx::query!(
		"DELETE FROM execution_resolutions WHERE effect_id IN (
			SELECT e.effect_id FROM effects e JOIN runs r ON r.run_id = e.run_id
			WHERE r.conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM effects WHERE run_id IN (
			SELECT run_id FROM runs WHERE conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM orphaned_executions WHERE execution_id IN (
			SELECT run_id FROM runs WHERE conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM artifact_references WHERE run_id IN (
			SELECT run_id FROM runs WHERE conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM change_checkpoints WHERE run_id IN (
			SELECT run_id FROM runs WHERE conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM run_executions WHERE run_id IN (
			SELECT run_id FROM runs WHERE conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	// The hours the removed measurements were counted into are recounted
	// at the next rebuild; what has already been downsampled past the raw
	// tier stands.
	crate::usage::history::mark_conversation_usage_hours_dirty(connection, id)
		.await?;
	changed += sqlx::query!(
		"DELETE FROM usage_observations WHERE conversation_id = ?1",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM usage_quota_snapshots WHERE conversation_id = ?1
			OR run_id IN (SELECT run_id FROM runs WHERE conversation_id = ?1)",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM remote_operations WHERE origin_conversation_id = ?1",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM scheduled_tasks WHERE conversation_id = ?1",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	// The journal: its floor moves past what goes, so an older cursor is
	// refused rather than replayed across the gap (ADR-0078).
	sqlx::query!(
		"UPDATE event_journal_state
		 SET minimum_replay_cursor = MAX(minimum_replay_cursor, COALESCE(
			(SELECT MAX(sequence) FROM events WHERE conversation_id = ?1), 0))
		 WHERE singleton = 1",
		id
	)
	.execute(&mut *connection)
	.await?;
	changed +=
		sqlx::query!("DELETE FROM events WHERE conversation_id = ?1", id)
			.execute(&mut *connection)
			.await?
			.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM search_documents WHERE conversation_id = ?1",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!(
		"DELETE FROM search_name_documents WHERE conversation_id = ?1",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	changed += sqlx::query!("DELETE FROM runs WHERE conversation_id = ?1", id)
		.execute(&mut *connection)
		.await?
		.rows_affected();
	// The Conversation itself, then the import it continued, which names
	// the Harness-native Conversation. The Trash entry, the queue, and the
	// fork launch cascade.
	let import_id = sqlx::query_scalar!(
		"SELECT import_id FROM conversations WHERE conversation_id = ?1",
		id
	)
	.fetch_optional(&mut *connection)
	.await?
	.flatten();
	changed += sqlx::query!(
		"DELETE FROM conversations WHERE conversation_id = ?1",
		id
	)
	.execute(&mut *connection)
	.await?
	.rows_affected();
	if let Some(import_id) = import_id {
		changed += sqlx::query!(
			"DELETE FROM imported_conversations WHERE import_id = ?1",
			import_id
		)
		.execute(&mut *connection)
		.await?
		.rows_affected();
	}
	Ok(changed)
}
