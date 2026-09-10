//! The owner-only Security audit: an integrity-chained record of the
//! decisions that widen trust, change policy, or destroy state (ADR-0105).
//!
//! It is not the Event journal. The journal is Conversation history that
//! clients subscribe to; this is a separate, narrower record that answers
//! who decided what about which target, how risky it was, and how it turned
//! out. No column here can hold a credential, a prompt, terminal output, or
//! file content, and the core has no way to put one in.
//!
//! Each record commits inside the same transaction as the decision it
//! describes, and carries the chain link that binds it to every record
//! before it. The newest link is published outside the database as the
//! audit head once that transaction commits (see [`crate::audit::head`]).

pub(crate) mod actor;
pub(crate) mod chain;
pub(crate) mod epoch;
pub(crate) mod head;
pub(crate) mod integrity;
pub(crate) mod read;
pub(crate) mod retention;

use crate::{
	AuditActorRecord, StoreError,
	audit::{
		chain::{
			AuditEntryHash, AuditTargetRef, ChainedFields, entry_hash,
			target_reference,
		},
		epoch::{counter_column, parse_counter},
		head::AuditHead,
	},
	transaction::WriteTransaction,
};
use uuid::Uuid;

/// Most records one audit page returns. The audit records decisions rather
/// than activity, so a page this size covers a long stretch of a Plane.
pub const AUDIT_PAGE_LIMIT: usize = 256;

/// How much a decision could cost if it was not the one the owner intended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditRisk {
	/// Recorded so it can be reviewed; it widens nothing and destroys
	/// nothing.
	Routine,
	/// Widens trust, changes policy, or exposes state.
	Elevated,
	/// May destroy state that cannot be brought back from within Jet.
	Destructive,
}

/// What became of the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
	/// It was carried out.
	Succeeded,
	/// It was refused before anything changed.
	Denied,
	/// It was allowed but did not complete.
	Failed,
}

/// A Security audit record to append in the transaction that carries out
/// the decision it describes. The core owns the `target_kind` and
/// `decision` vocabularies; the store keeps them as bounded text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAuditRecord {
	/// Globally unique identity chosen by the caller.
	pub record_id: Uuid,
	/// When the decision was made.
	pub recorded_at_unix_ms: i64,
	/// The responsible origin, which grants no Command authority.
	pub actor: AuditActorRecord,
	/// The durable kind spelling of what the decision was about, such as
	/// `account_binding`.
	pub target_kind: String,
	/// The target's own identity, when it has one.
	pub target_id: Option<String>,
	/// The durable spelling of the decision, such as `account.bound`.
	pub decision: String,
	/// How much the decision could cost.
	pub risk: AuditRisk,
	/// What became of it.
	pub outcome: AuditOutcome,
}

/// One recorded decision, with the chain link that binds it to the record
/// before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
	/// Plane-local position; never reused, even after retention removes the
	/// record that held it.
	pub sequence: u64,
	/// The authority epoch this record belongs to.
	pub epoch: u64,
	/// Globally unique identity.
	pub record_id: Uuid,
	/// When the decision was made.
	pub recorded_at_unix_ms: i64,
	/// The Plane that made it.
	pub plane_id: Uuid,
	/// The responsible origin, which grants no Command authority.
	pub actor: AuditActorRecord,
	/// The durable kind spelling of what it was about.
	pub target_kind: String,
	/// The opaque identifier of that target, which the chain covers and
	/// which outlives the target itself.
	pub target_reference: AuditTargetRef,
	/// The target's own identity, while the Plane still keeps it.
	pub target_id: Option<String>,
	/// The durable spelling of the decision.
	pub decision: String,
	/// How much it could cost.
	pub risk: AuditRisk,
	/// What became of it.
	pub outcome: AuditOutcome,
	/// The chain link this record folded to.
	pub entry_hash: AuditEntryHash,
}

/// Where the audit chain has reached inside the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditTip {
	/// The authority epoch the newest record belongs to.
	pub epoch: u64,
	/// Its position.
	pub sequence: u64,
	/// The chain link it folded to.
	pub entry_hash: AuditEntryHash,
}

/// The record retention last removed, whose link the remaining chain
/// continues from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetentionAnchor {
	pub(crate) epoch: u64,
	pub(crate) sequence: u64,
	pub(crate) entry_hash: AuditEntryHash,
}

impl WriteTransaction {
	/// Appends `record` to the Security audit and returns it as stored.
	///
	/// The record chains onto the newest one in its epoch, and the head it
	/// produces is published outside the database once this transaction
	/// commits. A caller therefore records a decision by appending it in
	/// the same transaction as the change it describes; there is no way to
	/// commit one without the other.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the chain tip cannot be read or the
	/// row cannot be written, including when a value exceeds its bound.
	pub async fn append_audit_record(
		&mut self,
		record: NewAuditRecord,
	) -> Result<AuditRecord, StoreError> {
		let plane_id = self.plane().await?.plane_id;
		let epoch =
			self.current_audit_epoch(record.recorded_at_unix_ms).await?;
		let previous = match self.audit_tip().await? {
			Some(tip) if tip.epoch == epoch.epoch => tip.entry_hash,
			// The first record of an epoch follows the epoch's own genesis,
			// which covers the gap that epoch recorded.
			Some(_) | None => epoch.genesis(plane_id),
		};
		let sequence = self.take_audit_sequence().await?;
		let mut stored = AuditRecord {
			sequence,
			epoch: epoch.epoch,
			record_id: record.record_id,
			recorded_at_unix_ms: record.recorded_at_unix_ms,
			plane_id,
			actor: record.actor,
			target_reference: target_reference(
				plane_id,
				&record.target_kind,
				record.target_id.as_deref(),
			),
			target_kind: record.target_kind,
			target_id: record.target_id,
			decision: record.decision,
			risk: record.risk,
			outcome: record.outcome,
			// Replaced immediately below; the link covers every field
			// beside it, so it cannot be computed before they are all here.
			entry_hash: AuditEntryHash([0; 32]),
		};
		stored.entry_hash = chain_link(previous, &stored);
		self.insert_audit_row(&stored).await?;
		self.publish_audit_head(AuditHead {
			epoch: stored.epoch,
			sequence: stored.sequence,
			entry_hash: stored.entry_hash,
		});
		Ok(stored)
	}

	async fn insert_audit_row(
		&mut self,
		record: &AuditRecord,
	) -> Result<(), StoreError> {
		let (actor_kind, actor_id) = record.actor.columns();
		let actor_id = actor_id.to_string();
		let sequence = counter_column(record.sequence)?;
		let epoch = counter_column(record.epoch)?;
		let record_id = record.record_id.to_string();
		let plane_id = record.plane_id.to_string();
		let reference = record.target_reference.0.to_vec();
		let hash = record.entry_hash.0.to_vec();
		let risk = record.risk.as_str();
		let outcome = record.outcome.as_str();
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		sqlx::query!(
			"INSERT INTO security_audit (sequence, epoch, record_id,
				recorded_at_unix_ms, plane_id, actor_kind, actor_id,
				target_kind, target_reference, target_id, decision, risk,
				outcome, entry_hash)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
				?14)",
			sequence,
			epoch,
			record_id,
			record.recorded_at_unix_ms,
			plane_id,
			actor_kind,
			actor_id,
			record.target_kind,
			reference,
			record.target_id,
			record.decision,
			risk,
			outcome,
			hash
		)
		.execute(self.connection())
		.await?;
		Ok(())
	}

	/// Claims the next audit position. Positions come from the audit's own
	/// counter rather than from the rows, so retention removing the oldest
	/// records can never hand a position out twice.
	async fn take_audit_sequence(&mut self) -> Result<u64, StoreError> {
		let assigned = sqlx::query_scalar!(
			r#"UPDATE audit_state SET next_sequence = next_sequence + 1
			 WHERE singleton = 1
			 RETURNING next_sequence - 1 AS "assigned!: i64""#
		)
		.fetch_one(self.connection())
		.await?;
		parse_counter(assigned)
	}
}

/// The chain link a record folds to when it follows `previous`.
///
/// The record's own `entry_hash` takes no part: this is what that field is
/// supposed to hold, computed from everything beside it.
pub(crate) fn chain_link(
	previous: AuditEntryHash,
	record: &AuditRecord,
) -> AuditEntryHash {
	let (actor_kind, actor_id) = record.actor.columns();
	let actor_id = actor_id.to_string();
	entry_hash(
		previous,
		&ChainedFields {
			sequence: record.sequence,
			epoch: record.epoch,
			record_id: record.record_id,
			recorded_at_unix_ms: record.recorded_at_unix_ms,
			plane_id: record.plane_id,
			actor_kind,
			actor_id: &actor_id,
			target_kind: &record.target_kind,
			target_reference: record.target_reference,
			decision: &record.decision,
			risk: record.risk.as_str(),
			outcome: record.outcome.as_str(),
		},
	)
}

/// Whether `record` still carries the identity its opaque target reference
/// was derived from. A record whose identity has been cleared by deletion
/// has nothing left to disagree with (ADR-0105).
pub(crate) fn target_matches_reference(record: &AuditRecord) -> bool {
	match &record.target_id {
		None => true,
		Some(id) => {
			target_reference(record.plane_id, &record.target_kind, Some(id))
				== record.target_reference
		}
	}
}

impl AuditRisk {
	/// The durable spelling, also used in JSON.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Routine => "routine",
			Self::Elevated => "elevated",
			Self::Destructive => "destructive",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		[Self::Routine, Self::Elevated, Self::Destructive]
			.into_iter()
			.find(|risk| risk.as_str() == text)
	}
}

impl AuditOutcome {
	/// The durable spelling, also used in JSON.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Succeeded => "succeeded",
			Self::Denied => "denied",
			Self::Failed => "failed",
		}
	}

	pub(crate) fn parse(text: &str) -> Option<Self> {
		[Self::Succeeded, Self::Denied, Self::Failed]
			.into_iter()
			.find(|outcome| outcome.as_str() == text)
	}
}

#[cfg(test)]
mod tests {
	use std::path::Path;

	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use crate::{
		AuditActorRecord, AuditBreach, AuditGap, AuditHead, AuditIntegrity,
		AuditIntegrityFailure, AuditOutcome, AuditRecord, AuditRisk,
		NewAuditRecord, Store, StoreError, audit_head_path,
	};

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;
	const DAY_MS: i64 = 24 * 60 * 60 * 1000;

	fn decision() -> NewAuditRecord {
		decision_about(&Uuid::now_v7().to_string(), NOW_UNIX_MS)
	}

	fn decision_about(
		target_id: &str,
		recorded_at_unix_ms: i64,
	) -> NewAuditRecord {
		NewAuditRecord {
			record_id: Uuid::now_v7(),
			recorded_at_unix_ms,
			actor: AuditActorRecord::InteractiveClient {
				client_id: Uuid::nil(),
			},
			target_kind: "account_binding".into(),
			target_id: Some(target_id.into()),
			decision: "account.bound".into(),
			risk: AuditRisk::Elevated,
			outcome: AuditOutcome::Succeeded,
		}
	}

	async fn append(store: &Store) -> AuditRecord {
		append_record(store, decision()).await
	}

	async fn append_record(
		store: &Store,
		record: NewAuditRecord,
	) -> AuditRecord {
		store
			.write(async |tx| tx.append_audit_record(record).await)
			.await
			.unwrap()
	}

	async fn page(store: &Store) -> (u64, Vec<AuditRecord>) {
		store
			.read(async |tx| tx.audit_page(0, 16).await)
			.await
			.unwrap()
	}

	fn head_of(record: &AuditRecord) -> AuditHead {
		AuditHead {
			epoch: record.epoch,
			sequence: record.sequence,
			entry_hash: record.entry_hash,
		}
	}

	/// Copies a closed store the way a Recovery snapshot does, taking the
	/// write-ahead log with it when one is still there.
	fn copy_store(from: &Path, to: &Path) {
		std::fs::copy(from, to).unwrap();
		for suffix in ["-wal", "-shm"] {
			let mut source = from.as_os_str().to_owned();
			source.push(suffix);
			let mut target = to.as_os_str().to_owned();
			target.push(suffix);
			let (source, target) = (Path::new(&source), Path::new(&target));
			if source.exists() {
				std::fs::copy(source, target).unwrap();
			} else if target.exists() {
				std::fs::remove_file(target).unwrap();
			}
		}
	}

	#[tokio::test]
	async fn a_recorded_decision_is_read_back_whole() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();

		let appended = append(&store).await;

		assert_eq!(page(&store).await, (1, vec![appended]));
	}

	#[tokio::test]
	async fn an_audit_nothing_has_been_recorded_in_validates() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();

		assert_eq!(
			store.validate_audit().await.unwrap(),
			AuditIntegrity::Verified { head: None }
		);
	}

	#[tokio::test]
	async fn the_head_beside_the_store_names_the_newest_decision() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();

		append(&store).await;
		let newest = append(&store).await;

		let plane_id = store.plane().await.unwrap().plane_id;
		let sequence = newest.sequence;
		let entry_hash = newest.entry_hash;
		assert_eq!(
			(
				store.validate_audit().await.unwrap(),
				std::fs::read_to_string(audit_head_path(&path)).unwrap()
			),
			(
				AuditIntegrity::Verified {
					head: Some(head_of(&newest))
				},
				format!(
					"jet-security-audit-head 1\nplane {plane_id}\nepoch 1\n\
				 sequence {sequence}\nhash {entry_hash}\n"
				)
			)
		);
	}

	#[tokio::test]
	async fn a_decision_that_rolls_back_publishes_no_head() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();

		let refused = store
			.write(async |tx| {
				tx.append_audit_record(decision()).await?;
				Err::<(), StoreError>(StoreError::Integrity(
					"the Command failed after recording its decision".into(),
				))
			})
			.await
			.unwrap_err();

		assert_eq!(
			(
				refused.to_string(),
				audit_head_path(&path).exists(),
				store.validate_audit().await.unwrap()
			),
			(
				"store integrity failure: the Command failed after recording \
			 its decision"
					.into(),
				false,
				AuditIntegrity::Verified { head: None }
			)
		);
	}

	#[tokio::test]
	async fn a_store_restored_behind_its_head_fails_validation() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let snapshot = dir.path().join("snapshot.sqlite3");

		let store = Store::open(&path).await.unwrap();
		append(&store).await;
		store.close().await;
		copy_store(&path, &snapshot);

		let store = Store::open(&path).await.unwrap();
		let lost = append(&store).await;
		store.close().await;

		// The database goes back to the snapshot. The head, which lives outside
		// it, does not.
		copy_store(&snapshot, &path);
		let store = Store::open(&path).await.unwrap();

		assert_eq!(
			store.validate_audit().await.unwrap(),
			AuditIntegrity::Failed(AuditIntegrityFailure {
				breach: AuditBreach::HeadNotInStore,
				epoch: 1,
				head: Some(head_of(&lost)),
				store_sequence: 1,
			})
		);
	}

	#[tokio::test]
	async fn a_head_lost_after_its_commit_is_repaired_at_the_next_start() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();

		append(&store).await;
		let stale = std::fs::read(audit_head_path(&path)).unwrap();
		let newest = append(&store).await;
		// The head write that the crash after the commit swallowed.
		std::fs::write(audit_head_path(&path), &stale).unwrap();

		let repaired = store.validate_audit().await.unwrap();

		assert_eq!(
			(
				repaired,
				std::fs::read(audit_head_path(&path)).unwrap() == stale
			),
			(
				AuditIntegrity::Verified {
					head: Some(head_of(&newest))
				},
				false
			)
		);
	}

	#[tokio::test]
	async fn an_audit_whose_head_is_gone_fails_validation() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();

		append(&store).await;
		std::fs::remove_file(audit_head_path(&path)).unwrap();

		assert_eq!(
			store.validate_audit().await.unwrap(),
			AuditIntegrity::Failed(AuditIntegrityFailure {
				breach: AuditBreach::HeadMissing,
				epoch: 1,
				head: None,
				store_sequence: 1,
			})
		);
	}

	#[tokio::test]
	async fn a_decision_edited_after_the_fact_no_longer_folds_to_its_link() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();

		let edited = append(&store).await;
		let newest = append(&store).await;
		store
			.write(async |tx| {
				sqlx::query!(
					"UPDATE security_audit SET outcome = 'denied'
				 WHERE sequence = ?1",
					1
				)
				.execute(tx.connection())
				.await?;
				Ok::<_, StoreError>(())
			})
			.await
			.unwrap();

		assert_eq!(
			store.validate_audit().await.unwrap(),
			AuditIntegrity::Failed(AuditIntegrityFailure {
				breach: AuditBreach::RecordAltered {
					sequence: edited.sequence
				},
				epoch: 1,
				head: Some(head_of(&newest)),
				store_sequence: 2,
			})
		);
	}

	#[tokio::test]
	async fn a_target_swapped_under_its_reference_fails_validation() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();

		let swapped = append(&store).await;
		let elsewhere = Uuid::now_v7().to_string();
		store
			.write(async |tx| {
				sqlx::query!(
					"UPDATE security_audit SET target_id = ?1 WHERE sequence = ?2",
					elsewhere,
					1
				)
				.execute(tx.connection())
				.await?;
				Ok::<_, StoreError>(())
			})
			.await
			.unwrap();

		assert_eq!(
			store.validate_audit().await.unwrap(),
			AuditIntegrity::Failed(AuditIntegrityFailure {
				breach: AuditBreach::TargetAltered {
					sequence: swapped.sequence
				},
				epoch: 1,
				head: Some(head_of(&swapped)),
				store_sequence: 1,
			})
		);
	}

	#[tokio::test]
	async fn retention_removes_expired_records_and_leaves_a_whole_chain() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();

		append_record(
			&store,
			decision_about("first", NOW_UNIX_MS - 9 * DAY_MS),
		)
		.await;
		append_record(
			&store,
			decision_about("second", NOW_UNIX_MS - 8 * DAY_MS),
		)
		.await;
		let kept =
			append_record(&store, decision_about("third", NOW_UNIX_MS)).await;

		let removed = store
			.prune_audit_before(NOW_UNIX_MS - DAY_MS)
			.await
			.unwrap();

		assert_eq!(
			(
				removed,
				page(&store).await,
				store.validate_audit().await.unwrap()
			),
			(
				2,
				(kept.sequence, vec![kept.clone()]),
				AuditIntegrity::Verified {
					head: Some(head_of(&kept))
				}
			)
		);
	}

	#[tokio::test]
	async fn retention_keeps_the_record_the_head_names() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();

		append_record(
			&store,
			decision_about("first", NOW_UNIX_MS - 9 * DAY_MS),
		)
		.await;
		let newest = append_record(
			&store,
			decision_about("second", NOW_UNIX_MS - 8 * DAY_MS),
		)
		.await;

		// Every record has expired, and the one the head names still cannot go.
		let removed = store.prune_audit_before(NOW_UNIX_MS).await.unwrap();

		assert_eq!(
			(
				removed,
				page(&store).await,
				store.validate_audit().await.unwrap()
			),
			(
				1,
				(newest.sequence, vec![newest.clone()]),
				AuditIntegrity::Verified {
					head: Some(head_of(&newest))
				}
			)
		);
	}

	#[tokio::test]
	async fn retention_stops_at_the_first_record_it_may_not_remove() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();

		append_record(
			&store,
			decision_about("expired", NOW_UNIX_MS - 9 * DAY_MS),
		)
		.await;
		// A clock that moved backwards leaves a fresh record among expired
		// ones. Removing around it would leave a hole the chain cannot be
		// folded across.
		let recent =
			append_record(&store, decision_about("recent", NOW_UNIX_MS)).await;
		let behind = append_record(
			&store,
			decision_about("behind", NOW_UNIX_MS - 9 * DAY_MS),
		)
		.await;

		let removed = store
			.prune_audit_before(NOW_UNIX_MS - DAY_MS)
			.await
			.unwrap();

		assert_eq!(
			(
				removed,
				page(&store).await,
				store.validate_audit().await.unwrap()
			),
			(
				1,
				(behind.sequence, vec![recent, behind.clone()]),
				AuditIntegrity::Verified {
					head: Some(head_of(&behind))
				}
			)
		);
	}

	#[tokio::test]
	async fn anonymizing_a_target_forgets_its_name_and_keeps_the_chain() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let deleted = Uuid::now_v7().to_string();

		let first =
			append_record(&store, decision_about(&deleted, NOW_UNIX_MS)).await;
		let other = append_record(&store, decision()).await;
		let second =
			append_record(&store, decision_about(&deleted, NOW_UNIX_MS)).await;

		let anonymized = store
			.write(async |tx| {
				tx.anonymize_audit_target("account_binding", &deleted).await
			})
			.await
			.unwrap();

		let expected = vec![
			AuditRecord {
				target_id: None,
				..first.clone()
			},
			other,
			AuditRecord {
				target_id: None,
				..second.clone()
			},
		];
		assert_eq!(
			(
				anonymized,
				page(&store).await,
				first.target_reference == second.target_reference,
				store.validate_audit().await.unwrap()
			),
			(
				2,
				(second.sequence, expected),
				true,
				AuditIntegrity::Verified {
					head: Some(head_of(&second))
				}
			)
		);
	}

	#[tokio::test]
	async fn a_new_epoch_records_its_gap_and_validates_from_its_own_genesis() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let store = Store::open(&path).await.unwrap();
		let abandoned = append(&store).await;

		let epoch = store
			.write(async |tx| {
				tx.begin_audit_epoch(
					AuditGap {
						sequence: abandoned.sequence,
						entry_hash: abandoned.entry_hash,
						reason: AuditBreach::HeadDiverged.as_str().into(),
					},
					NOW_UNIX_MS,
				)
				.await
			})
			.await
			.unwrap();
		let first = append(&store).await;

		assert_eq!(
			(epoch, first.epoch, store.validate_audit().await.unwrap()),
			(
				2,
				2,
				AuditIntegrity::Verified {
					head: Some(head_of(&first))
				}
			)
		);
	}

	/// A rollback puts the position counter back with the database while the
	/// head, which lives outside it, stays where it was. The epoch that carries
	/// on from that head has to carry on past it, or its records land on
	/// positions the abandoned chain already used and the fold never sees them.
	#[tokio::test]
	async fn a_new_epoch_continues_past_the_head_a_rollback_left_behind() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("plane.sqlite3");
		let snapshot = dir.path().join("snapshot.sqlite3");

		let store = Store::open(&path).await.unwrap();
		append(&store).await;
		store.close().await;
		copy_store(&path, &snapshot);

		let store = Store::open(&path).await.unwrap();
		let lost = append(&store).await;
		store.close().await;
		copy_store(&snapshot, &path);

		let store = Store::open(&path).await.unwrap();
		store
			.write(async |tx| {
				tx.begin_audit_epoch(
					AuditGap {
						sequence: lost.sequence,
						entry_hash: lost.entry_hash,
						reason: AuditBreach::HeadNotInStore.as_str().into(),
					},
					NOW_UNIX_MS,
				)
				.await
			})
			.await
			.unwrap();
		let first = append(&store).await;

		assert_eq!(
			(
				first.sequence > lost.sequence,
				store.validate_audit().await.unwrap()
			),
			(
				true,
				AuditIntegrity::Verified {
					head: Some(head_of(&first))
				}
			)
		);
	}
}
