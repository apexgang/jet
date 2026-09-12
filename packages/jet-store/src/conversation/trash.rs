//! Jet Trash: Conversations staged for permanent deletion, the reason they
//! were staged, and the grace period before they go (ADR-0011, ADR-0015).
//!
//! Staging writes one row; restoring removes it; expiry removes the
//! Conversation itself through [`super::purge`], and records the identity
//! in the Deletion ledger before the transaction commits (ADR-0102).

use crate::{
	StoreError,
	deletion::{DeletedIdentityKind, PendingDeletion},
	records::parse_uuid,
	transaction::{ReadTransaction, WriteTransaction},
};
use uuid::Uuid;

/// Why a Conversation is in Jet Trash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashReasonRecord {
	/// Its owner asked Jet to forget it.
	Manual,
	/// Its retention policy forgets it after its final Run, and nothing
	/// protected it any longer.
	Automatic,
	/// Its owner asked for it to be deleted everywhere, native history
	/// included where a Harness supports that.
	Everywhere,
	/// An approved Autodelete rule matched it while nothing protected it
	/// (ADR-0015).
	Autodelete,
	/// An approved Autodelete rule separately authorized to delete
	/// everywhere matched it.
	AutodeleteEverywhere,
}

impl TrashReasonRecord {
	/// The durable spelling.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Manual => "manual",
			Self::Automatic => "automatic",
			Self::Everywhere => "everywhere",
			Self::Autodelete => "autodelete",
			Self::AutodeleteEverywhere => "autodelete_everywhere",
		}
	}

	fn parse(text: &str) -> Result<Self, StoreError> {
		[
			Self::Manual,
			Self::Automatic,
			Self::Everywhere,
			Self::Autodelete,
			Self::AutodeleteEverywhere,
		]
		.into_iter()
		.find(|reason| reason.as_str() == text)
		.ok_or_else(|| {
			StoreError::Integrity(format!("unknown Trash reason {text:?}"))
		})
	}
}

/// One Conversation in Jet Trash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrashRecord {
	/// The Conversation staged for deletion.
	pub conversation_id: Uuid,
	/// Why it was staged.
	pub reason: TrashReasonRecord,
	/// When it was staged.
	pub trashed_at_unix_ms: i64,
	/// When its grace period ends and it may be deleted for good.
	pub expires_at_unix_ms: i64,
}

/// Most Trash entries one read returns.
const TRASH_PAGE_LIMIT: i64 = 256;

/// Most expired entries one sweep takes on.
const EXPIRY_BATCH_LIMIT: i64 = 16;

struct Row {
	conversation_id: String,
	reason: String,
	trashed_at_unix_ms: i64,
	expires_at_unix_ms: i64,
}

fn read_row(row: Row) -> Result<TrashRecord, StoreError> {
	Ok(TrashRecord {
		conversation_id: parse_uuid("conversation_id", &row.conversation_id)?,
		reason: TrashReasonRecord::parse(&row.reason)?,
		trashed_at_unix_ms: row.trashed_at_unix_ms,
		expires_at_unix_ms: row.expires_at_unix_ms,
	})
}

impl ReadTransaction {
	/// The Trash entry of `conversation_id`, if it is staged.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn trash_entry(
		&mut self,
		conversation_id: Uuid,
	) -> Result<Option<TrashRecord>, StoreError> {
		let id = conversation_id.to_string();
		sqlx::query_as!(
			Row,
			r#"SELECT conversation_id AS "conversation_id!", reason,
				trashed_at_unix_ms, expires_at_unix_ms
			 FROM conversation_trash WHERE conversation_id = ?1"#,
			id
		)
		.fetch_optional(self.connection())
		.await?
		.map(read_row)
		.transpose()
	}

	/// Every staged Conversation, soonest expiry first, bounded to 256.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn trash_entries(
		&mut self,
	) -> Result<Vec<TrashRecord>, StoreError> {
		sqlx::query_as!(
			Row,
			r#"SELECT conversation_id AS "conversation_id!", reason,
				trashed_at_unix_ms, expires_at_unix_ms
			 FROM conversation_trash
			 ORDER BY expires_at_unix_ms, conversation_id
			 LIMIT ?1"#,
			TRASH_PAGE_LIMIT
		)
		.fetch_all(self.connection())
		.await?
		.into_iter()
		.map(read_row)
		.collect()
	}

	/// The staged Conversations whose grace period ended at or before
	/// `now_unix_ms`, soonest expired first, bounded to one sweep's batch.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn expired_trash(
		&mut self,
		now_unix_ms: i64,
	) -> Result<Vec<TrashRecord>, StoreError> {
		sqlx::query_as!(
			Row,
			r#"SELECT conversation_id AS "conversation_id!", reason,
				trashed_at_unix_ms, expires_at_unix_ms
			 FROM conversation_trash
			 WHERE expires_at_unix_ms <= ?1
			 ORDER BY expires_at_unix_ms, conversation_id
			 LIMIT ?2"#,
			now_unix_ms,
			EXPIRY_BATCH_LIMIT
		)
		.fetch_all(self.connection())
		.await?
		.into_iter()
		.map(read_row)
		.collect()
	}

	/// When the soonest grace period ends, if anything is staged.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be read.
	pub async fn next_trash_expiry(
		&mut self,
	) -> Result<Option<i64>, StoreError> {
		Ok(sqlx::query_scalar!(
			r#"SELECT MIN(expires_at_unix_ms) AS "expires_at_unix_ms?: i64"
			 FROM conversation_trash"#
		)
		.fetch_one(self.connection())
		.await?)
	}

	/// A page of Conversations whose retention policy forgets them after
	/// their final Run, that have had at least one Run, and that are not
	/// staged yet, in identity order after `after`. Whether their last Run
	/// has ended and whether anything else protects them is the caller's
	/// judgement.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn forgettable_conversations(
		&mut self,
		after: &str,
	) -> Result<Vec<Uuid>, StoreError> {
		// 'forget_after_final_run' is the spelling `RetentionPolicy::as_str`
		// gives that policy; the macro takes a literal and no other form.
		let rows = sqlx::query_scalar!(
			r#"SELECT c.conversation_id AS "conversation_id!"
			 FROM conversations c
			 WHERE c.retention = 'forget_after_final_run'
				AND c.conversation_id > ?1
				AND EXISTS (SELECT 1 FROM runs r
					WHERE r.conversation_id = c.conversation_id)
				AND NOT EXISTS (SELECT 1 FROM conversation_trash t
					WHERE t.conversation_id = c.conversation_id)
			 ORDER BY c.conversation_id LIMIT 100"#,
			after
		)
		.fetch_all(self.connection())
		.await?;
		rows.iter()
			.map(|id| parse_uuid("conversation_id", id))
			.collect()
	}

	/// Whether any Effect of any Run of `conversation_id` is still pending
	/// or in flight.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the outbox cannot be read.
	pub async fn conversation_has_unresolved_effects(
		&mut self,
		conversation_id: Uuid,
	) -> Result<bool, StoreError> {
		let id = conversation_id.to_string();
		Ok(sqlx::query_scalar!(
			r#"SELECT EXISTS (
				SELECT 1 FROM effects e JOIN runs r ON r.run_id = e.run_id
				WHERE r.conversation_id = ?1
					AND e.state IN ('pending', 'in_flight')
			) AS "found!: bool""#,
			id
		)
		.fetch_one(self.connection())
		.await?)
	}
}

impl WriteTransaction {
	/// Stages `entry`. A Conversation already staged keeps its first entry.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be written, including
	/// when the Conversation does not exist.
	pub async fn insert_trash(
		&mut self,
		entry: TrashRecord,
	) -> Result<(), StoreError> {
		let id = entry.conversation_id.to_string();
		let reason = entry.reason.as_str();
		sqlx::query!(
			"INSERT INTO conversation_trash
				(conversation_id, reason, trashed_at_unix_ms, expires_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4)
			 ON CONFLICT (conversation_id) DO NOTHING",
			id,
			reason,
			entry.trashed_at_unix_ms,
			entry.expires_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Takes `conversation_id` out of Jet Trash, and says whether it was
	/// there.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the row cannot be removed.
	pub async fn delete_trash(
		&mut self,
		conversation_id: Uuid,
	) -> Result<bool, StoreError> {
		let id = conversation_id.to_string();
		let removed = sqlx::query!(
			"DELETE FROM conversation_trash WHERE conversation_id = ?1",
			id
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		Ok(removed > 0)
	}

	/// Removes `conversation_id` and every Jet-owned row it reaches, and
	/// records the deletion for the ledger the transaction writes before
	/// it commits. Artifact payloads are not touched here: with their
	/// references gone they are unreferenced, and collection removes them.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when a row cannot be removed.
	pub async fn delete_conversation(
		&mut self,
		conversation_id: Uuid,
		deleted_at_unix_ms: i64,
	) -> Result<(), StoreError> {
		let id = conversation_id.to_string();
		super::purge::purge_rows(self.connection(), &id).await?;
		self.record_deletion(PendingDeletion {
			kind: DeletedIdentityKind::Conversation,
			identity: conversation_id,
			deleted_at_unix_ms,
		});
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		ConversationOriginRecord, DeletionLedger, EventClass, NewConversation,
		NewEvent, NewProject, NewRun, NewWorkspace, RetentionPolicy, Store,
		WorkingTreeRecord, records::ActorRecord,
	};
	use pretty_assertions::assert_eq;

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	fn conversation(retention: RetentionPolicy) -> NewConversation {
		NewConversation {
			conversation_id: Uuid::now_v7(),
			retention,
			working_tree: WorkingTreeRecord::NoProject,
			origin: ConversationOriginRecord::New,
			created_at_unix_ms: NOW_UNIX_MS,
		}
	}

	fn entry(conversation_id: Uuid, expires_at_unix_ms: i64) -> TrashRecord {
		TrashRecord {
			conversation_id,
			reason: TrashReasonRecord::Manual,
			trashed_at_unix_ms: NOW_UNIX_MS,
			expires_at_unix_ms,
		}
	}

	#[tokio::test]
	async fn staging_lists_expires_and_restores() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let soon = conversation(RetentionPolicy::Retain);
		let later = conversation(RetentionPolicy::Retain);
		let kept = conversation(RetentionPolicy::Retain);
		let (soon_id, later_id) = (soon.conversation_id, later.conversation_id);
		store
			.write(async |tx| {
				for new in [soon, later, kept] {
					tx.insert_conversation(new).await?;
				}
				tx.insert_trash(entry(later_id, NOW_UNIX_MS + 20)).await?;
				tx.insert_trash(entry(soon_id, NOW_UNIX_MS + 10)).await?;
				// A second staging keeps the first entry.
				tx.insert_trash(entry(soon_id, NOW_UNIX_MS + 99)).await
			})
			.await
			.unwrap();
		let (entries, expired, next, staged) = store
			.read(async |tx| {
				Ok::<_, StoreError>((
					tx.trash_entries().await?,
					tx.expired_trash(NOW_UNIX_MS + 10).await?,
					tx.next_trash_expiry().await?,
					tx.trash_entry(soon_id).await?,
				))
			})
			.await
			.unwrap();
		let (restored, again, after) = store
			.write(async |tx| {
				let restored = tx.delete_trash(soon_id).await?;
				let again = tx.delete_trash(soon_id).await?;
				Ok::<_, StoreError>((
					restored,
					again,
					tx.trash_entries().await?,
				))
			})
			.await
			.unwrap();
		assert_eq!(
			(entries, expired, next, staged, restored, again, after),
			(
				vec![
					entry(soon_id, NOW_UNIX_MS + 10),
					entry(later_id, NOW_UNIX_MS + 20)
				],
				vec![entry(soon_id, NOW_UNIX_MS + 10)],
				Some(NOW_UNIX_MS + 10),
				Some(entry(soon_id, NOW_UNIX_MS + 10)),
				true,
				false,
				vec![entry(later_id, NOW_UNIX_MS + 20)],
			)
		);
	}

	#[tokio::test]
	async fn forgettable_conversations_need_the_policy_a_run_and_no_entry() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let retained = conversation(RetentionPolicy::Retain);
		let never_ran = conversation(RetentionPolicy::ForgetAfterFinalRun);
		let ran = conversation(RetentionPolicy::ForgetAfterFinalRun);
		let staged = conversation(RetentionPolicy::ForgetAfterFinalRun);
		let (retained_id, ran_id, staged_id) = (
			retained.conversation_id,
			ran.conversation_id,
			staged.conversation_id,
		);
		store
			.write(async |tx| {
				for new in [retained, never_ran, ran, staged] {
					tx.insert_conversation(new).await?;
				}
				for conversation_id in [retained_id, ran_id, staged_id] {
					tx.insert_run(NewRun {
						run_id: Uuid::now_v7(),
						conversation_id,
						created_at_unix_ms: NOW_UNIX_MS,
					})
					.await?;
				}
				tx.insert_trash(entry(staged_id, NOW_UNIX_MS + 1)).await
			})
			.await
			.unwrap();
		let found = store
			.read(async |tx| tx.forgettable_conversations("").await)
			.await
			.unwrap();
		assert_eq!(found, vec![ran_id]);
	}

	#[tokio::test]
	async fn deleting_a_conversation_takes_its_rows_and_reaches_the_ledger() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let gone = conversation(RetentionPolicy::Retain);
		let kept = conversation(RetentionPolicy::Retain);
		let (gone_id, kept_id) = (gone.conversation_id, kept.conversation_id);
		let run_id = Uuid::now_v7();
		let workspace_id = Uuid::now_v7();
		let project_id = Uuid::now_v7();
		store
			.write(async |tx| {
				tx.insert_conversation(gone).await?;
				tx.insert_conversation(kept).await?;
				tx.insert_project(NewProject {
					project_id,
					root: "/tmp/nowhere".into(),
					registered_by: ActorRecord::InteractiveClient {
						client_id: Uuid::nil(),
					},
					registered_at_unix_ms: NOW_UNIX_MS,
				})
				.await?;
				tx.insert_run(NewRun {
					run_id,
					conversation_id: gone_id,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await?;
				tx.insert_workspace(NewWorkspace {
					workspace_id,
					conversation_id: gone_id,
					project_id,
					root: "/tmp/nowhere".into(),
					base_selection: "main".into(),
					base_commit: "0".repeat(40),
					seed: None,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await?;
				tx.reference_artifact(run_id, &"a".repeat(64), 1).await?;
				for (conversation_id, run) in
					[(gone_id, Some(run_id)), (kept_id, None)]
				{
					tx.append_event(NewEvent {
						event_id: Uuid::now_v7(),
						actor: ActorRecord::InteractiveClient {
							client_id: Uuid::nil(),
						},
						recorded_at_unix_ms: NOW_UNIX_MS,
						conversation_id: Some(conversation_id),
						run_id: run,
						kind: "test".into(),
						payload_version: 1,
						payload: "{}".into(),
						class: EventClass::Semantic,
					})
					.await?;
				}
				tx.insert_trash(entry(gone_id, NOW_UNIX_MS)).await
			})
			.await
			.unwrap();

		store
			.write(async |tx| {
				tx.delete_conversation(gone_id, NOW_UNIX_MS + 1).await
			})
			.await
			.unwrap();

		let (gone_row, kept_row, runs, workspace, referenced, trash, position) =
			store
				.read(async |tx| {
					Ok::<_, StoreError>((
						tx.conversation(gone_id)
							.await?
							.map(|c| c.conversation_id),
						tx.conversation(kept_id)
							.await?
							.map(|c| c.conversation_id),
						tx.runs(gone_id).await?.len(),
						tx.workspace_of(gone_id).await?.map(|w| w.workspace_id),
						tx.artifact_referenced(&"a".repeat(64)).await?,
						tx.trash_entries().await?,
						tx.journal_position().await?,
					))
				})
				.await
				.unwrap();
		let ledger = store.deletion_ledger().unwrap();
		let DeletionLedger::Verified(records) = ledger else {
			panic!("ledger is corrupt");
		};
		assert_eq!(
			(
				gone_row,
				kept_row,
				runs,
				workspace,
				referenced,
				trash,
				position,
				records
					.iter()
					.map(|record| (record.kind, record.identity))
					.collect::<Vec<_>>(),
			),
			(
				None,
				Some(kept_id),
				0,
				None,
				false,
				vec![],
				(2, 1),
				vec![(DeletedIdentityKind::Conversation, gone_id)],
			)
		);
	}
}
