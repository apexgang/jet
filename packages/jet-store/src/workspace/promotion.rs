//! Workspace promotions: applying a Workspace's changes to a permanent
//! checkout or branch of its Project (ADR-0025).
//!
//! A row keeps what the preview bound and where the promotion stands. A
//! conflicted promotion is settled the moment it is recorded and keeps
//! its conflicting paths beside it; an applying one is settled by the
//! outcome its Effect records.

use crate::{
	StoreError,
	records::{ActorRecord, column_error, parse_uuid},
	transaction::{ReadTransaction, WriteTransaction},
};
use uuid::Uuid;

/// Where a promotion applies a Workspace's changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionDestinationRecord {
	/// The Project's own Local checkout.
	LocalCheckout,
	/// A branch of the Project, by name.
	Branch(String),
}

/// Where a promotion stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionStateRecord {
	/// Recorded, with its Effect not yet settled.
	Applying,
	/// Applied, and the destination verified to hold the result.
	Promoted,
	/// Never applied: the preview could not settle every path.
	Conflicted,
	/// Its Effect reported a definite failure; the destination is as it
	/// was.
	Failed,
	/// Its Effect's outcome could not be established.
	OutcomeUnknown,
}

/// Why one path of a conflicted promotion could not be settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionConflictKindRecord {
	/// Both sides changed the path in ways Git cannot combine.
	Diverged,
	/// The Workspace adds the path and the destination holds an ignored
	/// file there.
	Untracked,
	/// The destination's index holds a version of the path that differs
	/// from its file.
	Staged,
}

/// One path a conflicted promotion could not settle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionConflictRecord {
	/// The path, as Git spells it.
	pub path: String,
	/// Why it could not be settled.
	pub kind: PromotionConflictKindRecord,
}

/// A promotion to record, in the state it starts in: applying, or
/// conflicted with its paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkspacePromotion {
	/// Globally unique identity chosen by the caller.
	pub promotion_id: Uuid,
	/// The Workspace being promoted.
	pub workspace_id: Uuid,
	/// The authenticated Actor that confirmed the preview.
	pub promoted_by: ActorRecord,
	/// Where the changes go.
	pub destination: PromotionDestinationRecord,
	/// The commit the Workspace started from.
	pub base_commit: String,
	/// The Workspace's working tree as captured.
	pub workspace_tree: String,
	/// The commit the destination was at.
	pub destination_commit: String,
	/// The destination's content as captured.
	pub destination_tree: String,
	/// The tree the three-way merge produced.
	pub result_tree: String,
	/// Whether the destination held uncommitted changes of its own.
	pub destination_dirty: bool,
	/// How many paths the result changes in the destination.
	pub changed_paths: u32,
	/// Whether the promotion is applying or was conflicted on arrival.
	pub state: PromotionStateRecord,
	/// The paths that could not be settled; empty unless conflicted.
	pub conflicts: Vec<PromotionConflictRecord>,
	/// When the caller recorded the promotion.
	pub recorded_at_unix_ms: i64,
}

/// One recorded promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePromotionRecord {
	/// Globally unique identity.
	pub promotion_id: Uuid,
	/// The Workspace being promoted.
	pub workspace_id: Uuid,
	/// The authenticated Actor that confirmed the preview.
	pub promoted_by: ActorRecord,
	/// Where the changes go.
	pub destination: PromotionDestinationRecord,
	/// The commit the Workspace started from.
	pub base_commit: String,
	/// The Workspace's working tree as captured.
	pub workspace_tree: String,
	/// The commit the destination was at.
	pub destination_commit: String,
	/// The destination's content as captured.
	pub destination_tree: String,
	/// The tree the three-way merge produced.
	pub result_tree: String,
	/// Whether the destination held uncommitted changes of its own.
	pub destination_dirty: bool,
	/// How many paths the result changes in the destination.
	pub changed_paths: u32,
	/// Where the promotion stands.
	pub state: PromotionStateRecord,
	/// The paths that could not be settled; empty unless conflicted.
	pub conflicts: Vec<PromotionConflictRecord>,
	/// When the promotion was recorded.
	pub recorded_at_unix_ms: i64,
	/// When it reached a settled state, if it has.
	pub settled_at_unix_ms: Option<i64>,
}

/// One `workspace_promotions` row as SQLite stores it.
struct Row {
	promotion_id: String,
	workspace_id: String,
	actor_kind: String,
	actor_id: String,
	destination_kind: String,
	destination_branch: Option<String>,
	base_commit: String,
	workspace_tree: String,
	destination_commit: String,
	destination_tree: String,
	result_tree: String,
	destination_dirty: i64,
	changed_paths: i64,
	state: String,
	recorded_at_unix_ms: i64,
	settled_at_unix_ms: Option<i64>,
}

/// One `workspace_promotion_conflicts` row as SQLite stores it.
struct ConflictRow {
	path: String,
	kind: String,
}

impl PromotionStateRecord {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Applying => "applying",
			Self::Promoted => "promoted",
			Self::Conflicted => "conflicted",
			Self::Failed => "failed",
			Self::OutcomeUnknown => "outcome_unknown",
		}
	}

	fn parse(text: &str) -> Option<Self> {
		[
			Self::Applying,
			Self::Promoted,
			Self::Conflicted,
			Self::Failed,
			Self::OutcomeUnknown,
		]
		.into_iter()
		.find(|state| state.as_str() == text)
	}
}

impl PromotionConflictKindRecord {
	fn as_str(self) -> &'static str {
		match self {
			Self::Diverged => "diverged",
			Self::Untracked => "untracked",
			Self::Staged => "staged",
		}
	}

	fn parse(text: &str) -> Option<Self> {
		[Self::Diverged, Self::Untracked, Self::Staged]
			.into_iter()
			.find(|kind| kind.as_str() == text)
	}
}

impl ReadTransaction {
	/// The promotion identified by `promotion_id`, if recorded.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn promotion(
		&mut self,
		promotion_id: Uuid,
	) -> Result<Option<WorkspacePromotionRecord>, StoreError> {
		let promotion_id = promotion_id.to_string();
		// ASVS 1.2.4: SQL structure is static; every dynamic value in this
		// module is passed through SQLite parameters.
		let row = sqlx::query_as!(
			Row,
			r#"SELECT promotion_id AS "promotion_id!", workspace_id, actor_kind,
				actor_id, destination_kind, destination_branch, base_commit,
				workspace_tree, destination_commit, destination_tree,
				result_tree, destination_dirty, changed_paths, state,
				recorded_at_unix_ms, settled_at_unix_ms
			 FROM workspace_promotions
			 WHERE promotion_id = ?1"#,
			promotion_id
		)
		.fetch_optional(self.connection())
		.await?;
		match row {
			Some(row) => Ok(Some(self.read_row(row).await?)),
			None => Ok(None),
		}
	}

	/// The most recently recorded promotion of `workspace_id`, if any.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be read.
	pub async fn latest_promotion(
		&mut self,
		workspace_id: Uuid,
	) -> Result<Option<WorkspacePromotionRecord>, StoreError> {
		let workspace_id = workspace_id.to_string();
		let row = sqlx::query_as!(
			Row,
			r#"SELECT promotion_id AS "promotion_id!", workspace_id, actor_kind,
				actor_id, destination_kind, destination_branch, base_commit,
				workspace_tree, destination_commit, destination_tree,
				result_tree, destination_dirty, changed_paths, state,
				recorded_at_unix_ms, settled_at_unix_ms
			 FROM workspace_promotions
			 WHERE workspace_id = ?1
			 ORDER BY rowid DESC
			 LIMIT 1"#,
			workspace_id
		)
		.fetch_optional(self.connection())
		.await?;
		match row {
			Some(row) => Ok(Some(self.read_row(row).await?)),
			None => Ok(None),
		}
	}

	async fn read_row(
		&mut self,
		row: Row,
	) -> Result<WorkspacePromotionRecord, StoreError> {
		let conflicts = sqlx::query_as!(
			ConflictRow,
			"SELECT path, kind FROM workspace_promotion_conflicts
			 WHERE promotion_id = ?1
			 ORDER BY position",
			row.promotion_id
		)
		.fetch_all(self.connection())
		.await?
		.into_iter()
		.map(read_conflict)
		.collect::<Result<_, _>>()?;
		let destination =
			match (row.destination_kind.as_str(), row.destination_branch) {
				("local_checkout", None) => {
					PromotionDestinationRecord::LocalCheckout
				}
				("branch", Some(name)) => {
					PromotionDestinationRecord::Branch(name)
				}
				(kind, branch) => {
					return Err(column_error(
						"destination_kind",
						format!(
							"{kind:?} with branch {branch:?} is not a destination"
						),
					));
				}
			};
		Ok(WorkspacePromotionRecord {
			promotion_id: parse_uuid("promotion_id", &row.promotion_id)?,
			workspace_id: parse_uuid("workspace_id", &row.workspace_id)?,
			promoted_by: ActorRecord::parse(&row.actor_kind, &row.actor_id)?,
			destination,
			base_commit: row.base_commit,
			workspace_tree: row.workspace_tree,
			destination_commit: row.destination_commit,
			destination_tree: row.destination_tree,
			result_tree: row.result_tree,
			destination_dirty: match row.destination_dirty {
				0 => false,
				1 => true,
				other => {
					return Err(column_error(
						"destination_dirty",
						format!("{other} is not a flag"),
					));
				}
			},
			changed_paths: u32::try_from(row.changed_paths).map_err(|_| {
				column_error(
					"changed_paths",
					format!("{} is not a path count", row.changed_paths),
				)
			})?,
			state: PromotionStateRecord::parse(&row.state).ok_or_else(
				|| {
					column_error(
						"state",
						format!("unknown promotion state {:?}", row.state),
					)
				},
			)?,
			conflicts,
			recorded_at_unix_ms: row.recorded_at_unix_ms,
			settled_at_unix_ms: row.settled_at_unix_ms,
		})
	}
}

impl WriteTransaction {
	/// Records a new promotion and returns it as stored. A conflicted one
	/// is settled as it is recorded.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the rows cannot be written, including
	/// when the state is neither applying nor conflicted.
	pub async fn insert_promotion(
		&mut self,
		promotion: NewWorkspacePromotion,
	) -> Result<WorkspacePromotionRecord, StoreError> {
		let settled_at_unix_ms = match promotion.state {
			PromotionStateRecord::Applying => None,
			PromotionStateRecord::Conflicted => {
				Some(promotion.recorded_at_unix_ms)
			}
			PromotionStateRecord::Promoted
			| PromotionStateRecord::Failed
			| PromotionStateRecord::OutcomeUnknown => {
				return Err(StoreError::Integrity(
					"a promotion is recorded applying or conflicted".into(),
				));
			}
		};
		let id = promotion.promotion_id.to_string();
		let workspace = promotion.workspace_id.to_string();
		let (actor_kind, actor_id) = promotion.promoted_by.columns();
		let actor_id = actor_id.to_string();
		let (destination_kind, destination_branch) =
			match &promotion.destination {
				PromotionDestinationRecord::LocalCheckout => {
					("local_checkout", None)
				}
				PromotionDestinationRecord::Branch(name) => {
					("branch", Some(name.as_str()))
				}
			};
		let destination_dirty = i64::from(promotion.destination_dirty);
		let changed_paths = i64::from(promotion.changed_paths);
		let state = promotion.state.as_str();
		sqlx::query!(
			"INSERT INTO workspace_promotions
				(promotion_id, workspace_id, actor_kind, actor_id,
				destination_kind, destination_branch, base_commit,
				workspace_tree, destination_commit, destination_tree,
				result_tree, destination_dirty, changed_paths, state,
				recorded_at_unix_ms, settled_at_unix_ms)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
				?14, ?15, ?16)",
			id,
			workspace,
			actor_kind,
			actor_id,
			destination_kind,
			destination_branch,
			promotion.base_commit,
			promotion.workspace_tree,
			promotion.destination_commit,
			promotion.destination_tree,
			promotion.result_tree,
			destination_dirty,
			changed_paths,
			state,
			promotion.recorded_at_unix_ms,
			settled_at_unix_ms
		)
		.execute(self.connection())
		.await?;
		for (position, conflict) in promotion.conflicts.iter().enumerate() {
			let position = i64::try_from(position).unwrap_or(i64::MAX);
			let kind = conflict.kind.as_str();
			sqlx::query!(
				"INSERT INTO workspace_promotion_conflicts
					(promotion_id, position, path, kind)
				 VALUES (?1, ?2, ?3, ?4)",
				id,
				position,
				conflict.path,
				kind
			)
			.execute(self.connection())
			.await?;
		}
		Ok(WorkspacePromotionRecord {
			promotion_id: promotion.promotion_id,
			workspace_id: promotion.workspace_id,
			promoted_by: promotion.promoted_by,
			destination: promotion.destination,
			base_commit: promotion.base_commit,
			workspace_tree: promotion.workspace_tree,
			destination_commit: promotion.destination_commit,
			destination_tree: promotion.destination_tree,
			result_tree: promotion.result_tree,
			destination_dirty: promotion.destination_dirty,
			changed_paths: promotion.changed_paths,
			state: promotion.state,
			conflicts: promotion.conflicts,
			recorded_at_unix_ms: promotion.recorded_at_unix_ms,
			settled_at_unix_ms,
		})
	}

	/// Settles an applying promotion with the outcome its Effect recorded.
	///
	/// # Errors
	///
	/// Returns a [`StoreError`] when the promotion is not applying, the
	/// state is not a settled one, or the row cannot be updated.
	pub async fn settle_promotion(
		&mut self,
		promotion_id: Uuid,
		state: PromotionStateRecord,
		settled_at_unix_ms: i64,
	) -> Result<WorkspacePromotionRecord, StoreError> {
		match state {
			PromotionStateRecord::Promoted
			| PromotionStateRecord::Failed
			| PromotionStateRecord::OutcomeUnknown => {}
			PromotionStateRecord::Applying
			| PromotionStateRecord::Conflicted => {
				return Err(StoreError::Integrity(
					"a promotion settles as promoted, failed, or outcome unknown"
						.into(),
				));
			}
		}
		let id = promotion_id.to_string();
		let state = state.as_str();
		let changed = sqlx::query!(
			"UPDATE workspace_promotions
			 SET state = ?2, settled_at_unix_ms = ?3
			 WHERE promotion_id = ?1 AND state = 'applying'",
			id,
			state,
			settled_at_unix_ms
		)
		.execute(self.connection())
		.await?
		.rows_affected();
		if changed != 1 {
			return Err(StoreError::Integrity(format!(
				"promotion {promotion_id} is not applying"
			)));
		}
		self.promotion(promotion_id).await?.ok_or_else(|| {
			StoreError::Integrity(format!(
				"promotion {promotion_id} disappeared"
			))
		})
	}
}

fn read_conflict(
	row: ConflictRow,
) -> Result<PromotionConflictRecord, StoreError> {
	Ok(PromotionConflictRecord {
		path: row.path,
		kind: PromotionConflictKindRecord::parse(&row.kind).ok_or_else(
			|| {
				column_error(
					"kind",
					format!("unknown conflict kind {:?}", row.kind),
				)
			},
		)?,
	})
}

#[cfg(test)]
mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use super::{
		NewWorkspacePromotion, PromotionConflictKindRecord,
		PromotionConflictRecord, PromotionDestinationRecord,
		PromotionStateRecord, WorkspacePromotionRecord,
	};
	use crate::{
		ActorRecord, ConversationOriginRecord, EffectKindRecord,
		EffectSafetyRecord, NewConversation, NewEffect, NewProject,
		NewWorkspace, RetentionPolicy, Store, StoreError, WorkingTreeRecord,
	};

	const NOW_UNIX_MS: i64 = 1_700_000_000_000;

	fn actor() -> ActorRecord {
		ActorRecord::InteractiveClient {
			client_id: Uuid::nil(),
		}
	}

	/// A Project with one Workspace, recorded so promotions have something
	/// to reference.
	async fn workspace(store: &Store) -> Uuid {
		let project_id = Uuid::now_v7();
		let conversation_id = Uuid::now_v7();
		let workspace_id = Uuid::now_v7();
		store
			.write(async |tx| {
				tx.insert_project(NewProject {
					project_id,
					root: "/home/jet/repo".into(),
					registered_by: actor(),
					registered_at_unix_ms: NOW_UNIX_MS,
				})
				.await?;
				tx.insert_conversation(NewConversation {
					conversation_id,
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRecord::Workspace { project_id },
					origin: ConversationOriginRecord::New,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await?;
				tx.insert_workspace(NewWorkspace {
					workspace_id,
					conversation_id,
					project_id,
					root: "/home/jet/.jet/ws/a".into(),
					base_selection: "HEAD".into(),
					base_commit: "0".repeat(40),
					seed: None,
					created_at_unix_ms: NOW_UNIX_MS,
				})
				.await?;
				Ok::<_, StoreError>(())
			})
			.await
			.unwrap();
		workspace_id
	}

	fn promotion(
		workspace_id: Uuid,
		destination: PromotionDestinationRecord,
		state: PromotionStateRecord,
		conflicts: Vec<PromotionConflictRecord>,
	) -> NewWorkspacePromotion {
		NewWorkspacePromotion {
			promotion_id: Uuid::now_v7(),
			workspace_id,
			promoted_by: actor(),
			destination,
			base_commit: "0".repeat(40),
			workspace_tree: "1".repeat(40),
			destination_commit: "2".repeat(40),
			destination_tree: "3".repeat(40),
			result_tree: "4".repeat(40),
			destination_dirty: true,
			changed_paths: 2,
			state,
			conflicts,
			recorded_at_unix_ms: NOW_UNIX_MS,
		}
	}

	fn recorded(
		promotion: &NewWorkspacePromotion,
		state: PromotionStateRecord,
		settled_at_unix_ms: Option<i64>,
	) -> WorkspacePromotionRecord {
		WorkspacePromotionRecord {
			promotion_id: promotion.promotion_id,
			workspace_id: promotion.workspace_id,
			promoted_by: promotion.promoted_by,
			destination: promotion.destination.clone(),
			base_commit: promotion.base_commit.clone(),
			workspace_tree: promotion.workspace_tree.clone(),
			destination_commit: promotion.destination_commit.clone(),
			destination_tree: promotion.destination_tree.clone(),
			result_tree: promotion.result_tree.clone(),
			destination_dirty: promotion.destination_dirty,
			changed_paths: promotion.changed_paths,
			state,
			conflicts: promotion.conflicts.clone(),
			recorded_at_unix_ms: promotion.recorded_at_unix_ms,
			settled_at_unix_ms,
		}
	}

	/// A conflicted promotion is settled as it is recorded and keeps its
	/// paths in order; an applying one settles once, with the outcome its
	/// Effect records, and the newest promotion of a Workspace is the one
	/// read back with it. The Effect that applies a promotion names it
	/// (ADR-0025, ADR-0064).
	#[tokio::test]
	async fn a_promotion_is_recorded_with_its_conflicts_and_settled_once() {
		let dir = tempfile::tempdir().unwrap();
		let store = Store::open(&dir.path().join("plane.sqlite3"))
			.await
			.unwrap();
		let workspace_id = workspace(&store).await;
		let conflicted = promotion(
			workspace_id,
			PromotionDestinationRecord::LocalCheckout,
			PromotionStateRecord::Conflicted,
			vec![
				PromotionConflictRecord {
					path: "src/lib.rs".into(),
					kind: PromotionConflictKindRecord::Diverged,
				},
				PromotionConflictRecord {
					path: "local.txt".into(),
					kind: PromotionConflictKindRecord::Untracked,
				},
			],
		);
		let applying = promotion(
			workspace_id,
			PromotionDestinationRecord::Branch("release".into()),
			PromotionStateRecord::Applying,
			vec![],
		);
		let effect_id = Uuid::now_v7();

		let (first, latest_after_first, second, settled, settled_twice, latest) =
			store
				.write(async |tx| {
					let first = tx.insert_promotion(conflicted.clone()).await?;
					let latest_after_first =
						tx.latest_promotion(workspace_id).await?;
					let second = tx.insert_promotion(applying.clone()).await?;
					tx.insert_effect(&NewEffect {
						effect_id,
						command_id: Uuid::now_v7(),
						run_id: None,
						promotion_id: Some(applying.promotion_id),
						terminal_id: None,
						kind: EffectKindRecord::PromoteWorkspace,
						safety: EffectSafetyRecord::Ambiguous,
					})
					.await?;
					let settled = tx
						.settle_promotion(
							applying.promotion_id,
							PromotionStateRecord::Promoted,
							NOW_UNIX_MS + 1,
						)
						.await?;
					let settled_twice = tx
						.settle_promotion(
							applying.promotion_id,
							PromotionStateRecord::Failed,
							NOW_UNIX_MS + 2,
						)
						.await
						.is_err();
					let latest = tx.latest_promotion(workspace_id).await?;
					Ok::<_, StoreError>((
						first,
						latest_after_first,
						second,
						settled,
						settled_twice,
						latest,
					))
				})
				.await
				.unwrap();
		let effects = store
			.read(async |tx| {
				tx.unresolved_effects_of(EffectKindRecord::PromoteWorkspace)
					.await
			})
			.await
			.unwrap();

		assert_eq!(
			(
				&first,
				latest_after_first.as_ref(),
				&second,
				&settled,
				settled_twice,
				latest.as_ref(),
				effects
					.iter()
					.map(|effect| (effect.effect_id, effect.promotion_id))
					.collect::<Vec<_>>(),
			),
			(
				&recorded(
					&conflicted,
					PromotionStateRecord::Conflicted,
					Some(NOW_UNIX_MS)
				),
				Some(&recorded(
					&conflicted,
					PromotionStateRecord::Conflicted,
					Some(NOW_UNIX_MS)
				)),
				&recorded(&applying, PromotionStateRecord::Applying, None),
				&recorded(
					&applying,
					PromotionStateRecord::Promoted,
					Some(NOW_UNIX_MS + 1)
				),
				true,
				Some(&recorded(
					&applying,
					PromotionStateRecord::Promoted,
					Some(NOW_UNIX_MS + 1)
				)),
				vec![(effect_id, Some(applying.promotion_id))],
			)
		);
	}
}
