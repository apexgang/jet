//! Autodelete rules: the prompt, the Utility job that compiled it, and the
//! interpretation its owner edited or approved (ADR-0015).
//!
//! The row keeps no candidate matches: those are read when asked, from the
//! Conversations that have been inactive long enough and are not staged.

use crate::{
	StoreError, records::parse_uuid, transaction::ReadTransaction,
	transaction::WriteTransaction,
};
use uuid::Uuid;

/// How many rules one Plane lists.
pub const AUTODELETE_RULE_LIMIT: i64 = 64;

/// One Autodelete rule as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutodeleteRuleRecord {
	/// Client-chosen identity.
	pub rule_id: Uuid,
	/// The natural-language source, 1 to 4096 bytes.
	pub prompt: String,
	/// The Utility job that compiled this source.
	pub utility_job_id: Uuid,
	/// The interpretation the owner edited or approved; `None` while the
	/// draft is the job's.
	pub inactive_days: Option<u32>,
	/// When the owner approved this interpretation, if they have.
	pub approved_at_unix_ms: Option<i64>,
	/// Whether matches may have their native history requested too.
	pub everywhere: bool,
	/// When the rule was first compiled.
	pub created_at_unix_ms: i64,
	/// When it was last compiled, edited, approved, or authorized.
	pub updated_at_unix_ms: i64,
}

/// A Conversation that has been idle since `last_active_at_unix_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InactiveConversation {
	/// The idle Conversation.
	pub conversation_id: Uuid,
	/// Its creation or the end of its latest Run, whichever is later.
	pub last_active_at_unix_ms: i64,
}

struct Row {
	rule_id: String,
	prompt: String,
	utility_job_id: String,
	inactive_days: Option<i64>,
	approved_at_unix_ms: Option<i64>,
	everywhere: i64,
	created_at_unix_ms: i64,
	updated_at_unix_ms: i64,
}

fn read_row(row: Row) -> Result<AutodeleteRuleRecord, StoreError> {
	let inactive_days = match row.inactive_days {
		Some(days) => Some(u32::try_from(days).map_err(|_| {
			StoreError::Integrity(format!(
				"autodelete_rules.inactive_days out of range: {days}"
			))
		})?),
		None => None,
	};
	Ok(AutodeleteRuleRecord {
		rule_id: parse_uuid("rule_id", &row.rule_id)?,
		prompt: row.prompt,
		utility_job_id: parse_uuid("utility_job_id", &row.utility_job_id)?,
		inactive_days,
		approved_at_unix_ms: row.approved_at_unix_ms,
		everywhere: row.everywhere != 0,
		created_at_unix_ms: row.created_at_unix_ms,
		updated_at_unix_ms: row.updated_at_unix_ms,
	})
}

impl ReadTransaction {
	/// One rule, if it exists.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn autodelete_rule(
		&mut self,
		rule_id: Uuid,
	) -> Result<Option<AutodeleteRuleRecord>, StoreError> {
		let id = rule_id.to_string();
		sqlx::query_as!(
			Row,
			r#"SELECT rule_id AS "rule_id!", prompt, utility_job_id,
				inactive_days, approved_at_unix_ms, everywhere,
				created_at_unix_ms, updated_at_unix_ms
			 FROM autodelete_rules WHERE rule_id = ?1"#,
			id
		)
		.fetch_optional(self.connection())
		.await?
		.map(read_row)
		.transpose()
	}

	/// Every rule, oldest first, bounded to [`AUTODELETE_RULE_LIMIT`].
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn autodelete_rules(
		&mut self,
	) -> Result<Vec<AutodeleteRuleRecord>, StoreError> {
		sqlx::query_as!(
			Row,
			r#"SELECT rule_id AS "rule_id!", prompt, utility_job_id,
				inactive_days, approved_at_unix_ms, everywhere,
				created_at_unix_ms, updated_at_unix_ms
			 FROM autodelete_rules
			 ORDER BY created_at_unix_ms, rule_id LIMIT ?1"#,
			AUTODELETE_RULE_LIMIT
		)
		.fetch_all(self.connection())
		.await?
		.into_iter()
		.map(read_row)
		.collect()
	}

	/// A page of Conversations that are not staged, have no Run still
	/// running, and were last active at or before `cutoff_unix_ms`, in
	/// identity order after `after`. Whether anything else protects them
	/// is the caller's judgement.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn inactive_conversations(
		&mut self,
		cutoff_unix_ms: i64,
		after: &str,
		limit: i64,
	) -> Result<Vec<InactiveConversation>, StoreError> {
		let rows = sqlx::query!(
			r#"SELECT c.conversation_id AS "conversation_id!",
				MAX(c.created_at_unix_ms, COALESCE((
					SELECT MAX(COALESCE(r.ended_at_unix_ms, r.created_at_unix_ms))
					FROM runs r WHERE r.conversation_id = c.conversation_id
				), 0)) AS "last_active_at_unix_ms!: i64"
			 FROM conversations c
			 WHERE c.conversation_id > ?1
				AND NOT EXISTS (SELECT 1 FROM conversation_trash t
					WHERE t.conversation_id = c.conversation_id)
				AND NOT EXISTS (SELECT 1 FROM runs r
					WHERE r.conversation_id = c.conversation_id
						AND r.ended_at_unix_ms IS NULL)
				AND MAX(c.created_at_unix_ms, COALESCE((
					SELECT MAX(COALESCE(r.ended_at_unix_ms, r.created_at_unix_ms))
					FROM runs r WHERE r.conversation_id = c.conversation_id
				), 0)) <= ?2
			 ORDER BY c.conversation_id LIMIT ?3"#,
			after,
			cutoff_unix_ms,
			limit
		)
		.fetch_all(self.connection())
		.await?;
		rows.into_iter()
			.map(|row| {
				Ok(InactiveConversation {
					conversation_id: parse_uuid(
						"conversation_id",
						&row.conversation_id,
					)?,
					last_active_at_unix_ms: row.last_active_at_unix_ms,
				})
			})
			.collect()
	}

	/// When the first Conversation that is not yet idle past `cutoff_unix_ms`
	/// was last active, if there is one, among those not staged and without
	/// a running Run. An approved rule's next match becomes due its
	/// inactivity after that; Conversations already past the cutoff are the
	/// sweep's to judge and set no deadline.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn next_inactivity_after(
		&mut self,
		cutoff_unix_ms: i64,
	) -> Result<Option<i64>, StoreError> {
		Ok(sqlx::query_scalar!(
			r#"SELECT MIN(last_active_at_unix_ms) AS "last_active_at_unix_ms?: i64"
			 FROM (
				SELECT MAX(c.created_at_unix_ms, COALESCE((
					SELECT MAX(COALESCE(r.ended_at_unix_ms, r.created_at_unix_ms))
					FROM runs r WHERE r.conversation_id = c.conversation_id
				), 0)) AS last_active_at_unix_ms
				FROM conversations c
				WHERE NOT EXISTS (SELECT 1 FROM conversation_trash t
						WHERE t.conversation_id = c.conversation_id)
					AND NOT EXISTS (SELECT 1 FROM runs r
						WHERE r.conversation_id = c.conversation_id
							AND r.ended_at_unix_ms IS NULL)
			 )
			 WHERE last_active_at_unix_ms > ?1"#,
			cutoff_unix_ms
		)
		.fetch_one(self.connection())
		.await?)
	}

	/// When one Conversation was created or its latest Run ended, whichever
	/// is later, or `None` when it does not exist or a Run is still
	/// running.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn conversation_last_active(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Option<i64>, StoreError> {
		let id = conversation_id.to_string();
		Ok(sqlx::query_scalar!(
			r#"SELECT MAX(c.created_at_unix_ms, COALESCE((
					SELECT MAX(COALESCE(r.ended_at_unix_ms, r.created_at_unix_ms))
					FROM runs r WHERE r.conversation_id = c.conversation_id
				), 0)) AS "last_active_at_unix_ms!: i64"
			 FROM conversations c
			 WHERE c.conversation_id = ?1
				AND NOT EXISTS (SELECT 1 FROM runs r
					WHERE r.conversation_id = c.conversation_id
						AND r.ended_at_unix_ms IS NULL)"#,
			id
		)
		.fetch_optional(self.connection())
		.await?)
	}
}

impl WriteTransaction {
	/// Writes `rule`, replacing an existing row with the same identity.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when the Utility job does not exist or a column check fails.
	pub async fn save_autodelete_rule(
		&mut self,
		rule: &AutodeleteRuleRecord,
	) -> Result<(), StoreError> {
		let id = rule.rule_id.to_string();
		let job_id = rule.utility_job_id.to_string();
		let inactive_days = rule.inactive_days.map(i64::from);
		let everywhere = i64::from(rule.everywhere);
		sqlx::query!(
			"INSERT INTO autodelete_rules
				(rule_id, prompt, utility_job_id, inactive_days,
				 approved_at_unix_ms, everywhere,
				 created_at_unix_ms, updated_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
			 ON CONFLICT (rule_id) DO UPDATE SET
				prompt = excluded.prompt,
				utility_job_id = excluded.utility_job_id,
				inactive_days = excluded.inactive_days,
				approved_at_unix_ms = excluded.approved_at_unix_ms,
				everywhere = excluded.everywhere,
				updated_at_unix_ms = excluded.updated_at_unix_ms",
			id,
			rule.prompt,
			job_id,
			inactive_days,
			rule.approved_at_unix_ms,
			everywhere,
			rule.created_at_unix_ms,
			rule.updated_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Removes the rule, and says whether it existed.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be removed.
	pub async fn delete_autodelete_rule(
		&mut self,
		rule_id: Uuid,
	) -> Result<bool, StoreError> {
		let id = rule_id.to_string();
		let removed =
			sqlx::query!("DELETE FROM autodelete_rules WHERE rule_id = ?1", id)
				.execute(self.connection())
				.await?
				.rows_affected();
		Ok(removed > 0)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		ConversationOriginRecord, NewConversation, NewRun, RetentionPolicy,
		RunLifecycle, Store, TrashReasonRecord, TrashRecord, WorkingTreeRecord,
	};
	use pretty_assertions::assert_eq;

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;
	const DAY_MS: i64 = 24 * 60 * 60 * 1000;

	fn conversation(created_at_unix_ms: i64) -> NewConversation {
		NewConversation {
			conversation_id: Uuid::now_v7(),
			retention: RetentionPolicy::Retain,
			working_tree: WorkingTreeRecord::NoProject,
			origin: ConversationOriginRecord::New,
			created_at_unix_ms,
		}
	}

	fn rule(rule_id: Uuid, utility_job_id: Uuid) -> AutodeleteRuleRecord {
		AutodeleteRuleRecord {
			rule_id,
			prompt: "Forget anything idle for 90 days".into(),
			utility_job_id,
			inactive_days: None,
			approved_at_unix_ms: None,
			everywhere: false,
			created_at_unix_ms: NOW_UNIX_MS,
			updated_at_unix_ms: NOW_UNIX_MS,
		}
	}

	/// A rule is written, replaced in place, listed in creation order, and
	/// removed once; a row that claims approval without an interpretation
	/// or authorization without approval is refused by the schema.
	#[tokio::test]
	async fn rules_are_saved_replaced_listed_and_removed() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let job = Uuid::now_v7();
		let first = rule(Uuid::now_v7(), job);
		let second = AutodeleteRuleRecord {
			created_at_unix_ms: NOW_UNIX_MS + 1,
			..rule(Uuid::now_v7(), job)
		};
		let approved = AutodeleteRuleRecord {
			inactive_days: Some(90),
			approved_at_unix_ms: Some(NOW_UNIX_MS + 2),
			everywhere: true,
			updated_at_unix_ms: NOW_UNIX_MS + 2,
			..first.clone()
		};
		let (listed, unapproved_everywhere, approved_without_days) = store
			.write(async |tx| {
				tx.save_utility_job(job, "{}").await?;
				tx.save_autodelete_rule(&second).await?;
				tx.save_autodelete_rule(&first).await?;
				tx.save_autodelete_rule(&approved).await?;
				let listed = tx.autodelete_rules().await?;
				let unapproved_everywhere = tx
					.save_autodelete_rule(&AutodeleteRuleRecord {
						everywhere: true,
						..second.clone()
					})
					.await
					.is_err();
				let approved_without_days = tx
					.save_autodelete_rule(&AutodeleteRuleRecord {
						approved_at_unix_ms: Some(NOW_UNIX_MS),
						..second.clone()
					})
					.await
					.is_err();
				Ok::<_, StoreError>((
					listed,
					unapproved_everywhere,
					approved_without_days,
				))
			})
			.await
			.unwrap();
		let (removed, again, one, left) = store
			.write(async |tx| {
				let removed = tx.delete_autodelete_rule(first.rule_id).await?;
				let again = tx.delete_autodelete_rule(first.rule_id).await?;
				Ok::<_, StoreError>((
					removed,
					again,
					tx.autodelete_rule(second.rule_id).await?,
					tx.autodelete_rules().await?,
				))
			})
			.await
			.unwrap();
		assert_eq!(
			(
				listed,
				unapproved_everywhere,
				approved_without_days,
				removed,
				again,
				one,
				left
			),
			(
				vec![approved, second.clone()],
				true,
				true,
				true,
				false,
				Some(second.clone()),
				vec![second],
			)
		);
	}

	/// Inactivity is the later of creation and the latest Run's end. A
	/// Conversation with a Run still running, or one already staged, is
	/// never a candidate, has no last activity, and sets no deadline; the
	/// next deadline comes from the first Conversation not yet past the
	/// cutoff.
	#[tokio::test]
	async fn inactive_conversations_follow_their_latest_run() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let idle = conversation(NOW_UNIX_MS - 100 * DAY_MS);
		let recently_run = conversation(NOW_UNIX_MS - 100 * DAY_MS);
		let running = conversation(NOW_UNIX_MS - 100 * DAY_MS);
		let staged = conversation(NOW_UNIX_MS - 100 * DAY_MS);
		let fresh = conversation(NOW_UNIX_MS - DAY_MS);
		let ids = [
			idle.conversation_id,
			recently_run.conversation_id,
			running.conversation_id,
			staged.conversation_id,
			fresh.conversation_id,
		];
		store
			.write(async |tx| {
				for new in [idle, recently_run, running, staged, fresh] {
					tx.insert_conversation(new).await?;
				}
				let ended = Uuid::now_v7();
				tx.insert_run(NewRun {
					run_id: ended,
					conversation_id: ids[1],
					created_at_unix_ms: NOW_UNIX_MS - 50 * DAY_MS,
				})
				.await?;
				tx.update_run_lifecycle(
					ended,
					RunLifecycle::Completed,
					NOW_UNIX_MS - 40 * DAY_MS,
				)
				.await?;
				tx.insert_run(NewRun {
					run_id: Uuid::now_v7(),
					conversation_id: ids[2],
					created_at_unix_ms: NOW_UNIX_MS - 50 * DAY_MS,
				})
				.await?;
				tx.insert_trash(TrashRecord {
					conversation_id: ids[3],
					reason: TrashReasonRecord::Autodelete,
					trashed_at_unix_ms: NOW_UNIX_MS,
					expires_at_unix_ms: NOW_UNIX_MS + DAY_MS,
				})
				.await
			})
			.await
			.unwrap();
		let (ninety, thirty, next, last_active) = store
			.read(async |tx| {
				Ok::<_, StoreError>((
					tx.inactive_conversations(
						NOW_UNIX_MS - 90 * DAY_MS,
						"",
						10,
					)
					.await?,
					tx.inactive_conversations(
						NOW_UNIX_MS - 30 * DAY_MS,
						"",
						10,
					)
					.await?,
					tx.next_inactivity_after(NOW_UNIX_MS - 90 * DAY_MS).await?,
					(
						tx.conversation_last_active(ids[1]).await?,
						tx.conversation_last_active(ids[2]).await?,
						tx.conversation_last_active(Uuid::now_v7()).await?,
					),
				))
			})
			.await
			.unwrap();
		let mut expected_thirty = vec![
			InactiveConversation {
				conversation_id: ids[0],
				last_active_at_unix_ms: NOW_UNIX_MS - 100 * DAY_MS,
			},
			InactiveConversation {
				conversation_id: ids[1],
				last_active_at_unix_ms: NOW_UNIX_MS - 40 * DAY_MS,
			},
		];
		expected_thirty.sort_by_key(|c| c.conversation_id.to_string());
		assert_eq!(
			(ninety, thirty, next, last_active),
			(
				vec![InactiveConversation {
					conversation_id: ids[0],
					last_active_at_unix_ms: NOW_UNIX_MS - 100 * DAY_MS,
				}],
				expected_thirty,
				Some(NOW_UNIX_MS - 40 * DAY_MS),
				(Some(NOW_UNIX_MS - 40 * DAY_MS), None, None),
			)
		);
	}
}
