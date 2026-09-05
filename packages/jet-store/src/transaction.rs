//! Units of work over the store. A [`ReadTransaction`] sees one consistent
//! snapshot; a [`WriteTransaction`] commits every change or none of them,
//! so current state and its journal Events always land together
//! (ADR-0020, ADR-0071).

use std::ops::{Deref, DerefMut};

use sqlx::{SqliteConnection, SqliteTransaction};

use crate::audit_head::{self, AuditHead};
use crate::{Store, StoreError};

/// One consistent read snapshot of the store.
pub struct ReadTransaction {
	transaction: SqliteTransaction<'static>,
}

/// One atomic set of changes, readable while it is being built.
pub struct WriteTransaction {
	read: ReadTransaction,
	/// Where the Security audit chain will have reached once this
	/// transaction commits. Appending an audit record sets it; nothing else
	/// can, and it is published only after the commit that earned it
	/// (ADR-0105).
	audit_head: Option<AuditHead>,
}

impl WriteTransaction {
	/// Records the head to publish outside the store after this transaction
	/// commits.
	pub(crate) fn publish_audit_head(&mut self, head: AuditHead) {
		self.audit_head = Some(head);
	}
}

impl ReadTransaction {
	/// The connection every statement in this unit of work runs on. SQLite
	/// executes statements on one connection in order, and borrowing the
	/// transaction exclusively is what makes the compiler agree.
	pub(crate) fn connection(&mut self) -> &mut SqliteConnection {
		&mut self.transaction
	}
}

impl Deref for WriteTransaction {
	type Target = ReadTransaction;

	fn deref(&self) -> &Self::Target {
		&self.read
	}
}

impl DerefMut for WriteTransaction {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.read
	}
}

impl Store {
	/// Runs `work` against one consistent snapshot.
	///
	/// `work` is an async closure, so the call reads
	/// `store.read(async |tx| { .. }).await`. A plain closure returning an
	/// async block does not satisfy the bound.
	///
	/// # Errors
	///
	/// Returns the error `work` produced, or a [`StoreError`] converted into
	/// `E` when the snapshot cannot be opened.
	pub async fn read<T, E: From<StoreError>>(
		&self,
		work: impl AsyncFnOnce(&mut ReadTransaction) -> Result<T, E>,
	) -> Result<T, E> {
		let transaction = self
			.pool
			.begin_with("BEGIN DEFERRED")
			.await
			.map_err(StoreError::from)?;
		let mut transaction = ReadTransaction { transaction };
		let result = work(&mut transaction).await;
		// A read snapshot ends by releasing its read mark. Awaiting that
		// rollback keeps it from outliving the call; a rollback that fails
		// must not mask what the caller produced.
		let _ = transaction.transaction.rollback().await;
		result
	}

	/// Runs `work` as one durable transaction: every change commits when
	/// `work` returns `Ok`, and none of them persist when it returns `Err`.
	///
	/// `work` is an async closure, so the call reads
	/// `store.write(async |tx| { .. }).await`.
	///
	/// # Errors
	///
	/// Returns the error `work` produced, or a [`StoreError`] converted into
	/// `E` when the transaction cannot be opened or committed.
	pub async fn write<T, E: From<StoreError>>(
		&self,
		work: impl AsyncFnOnce(&mut WriteTransaction) -> Result<T, E>,
	) -> Result<T, E> {
		// A write takes its lock up front. A deferred transaction that reads
		// before it writes cannot upgrade, and SQLite refuses it outright
		// rather than waiting on the busy handler.
		let transaction = self
			.pool
			.begin_with("BEGIN IMMEDIATE")
			.await
			.map_err(StoreError::from)?;
		let mut transaction = WriteTransaction {
			read: ReadTransaction { transaction },
			audit_head: None,
		};
		match work(&mut transaction).await {
			Ok(value) => {
				let head = transaction.audit_head;
				transaction
					.read
					.transaction
					.commit()
					.await
					.map_err(|error| E::from(StoreError::from(error)))?;
				// The head follows the commit it describes. A crash in
				// between leaves the store one or more records ahead of the
				// head, which the next start folds through and repairs; a
				// head written first would name a record no commit ever
				// made (ADR-0105).
				if let Some(head) = head {
					audit_head::write(&self.database, self.plane_id, head)
						.map_err(E::from)?;
				}
				Ok(value)
			}
			Err(error) => {
				// Dropping the transaction only enqueues its rollback;
				// awaiting it releases the write lock before the caller sees
				// the error.
				let _ = transaction.read.transaction.rollback().await;
				Err(error)
			}
		}
	}
}
