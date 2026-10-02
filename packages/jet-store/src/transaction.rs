//! Units of work over the store. A [`ReadTransaction`] sees one consistent
//! snapshot; a [`WriteTransaction`] commits every change or none of them,
//! so current state and its journal Events always land together
//! (ADR-0020, ADR-0071).

use crate::{
	Store, StoreError,
	audit::head::{self as audit_head, AuditHead},
	authority::{self, PendingFence},
	deletion::{self, PendingDeletion},
	plane,
};
use sqlx::{SqliteConnection, SqliteTransaction};
use std::{
	future::Future,
	ops::{Deref, DerefMut},
	pin::Pin,
};

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
	/// The permanent deletions this transaction has made. They reach the
	/// Deletion ledger just before the commit, so no deletion is
	/// acknowledged that a restoration could undo (ADR-0102).
	deletions: Vec<PendingDeletion>,
	/// The Conversation authorities this transaction has retired. They
	/// reach the Authority fences just before the commit, so no
	/// relinquishment is acknowledged that a restoration could undo
	/// (ADR-0070).
	fences: Vec<PendingFence>,
}

impl WriteTransaction {
	/// Records the head to publish outside the store after this transaction
	/// commits.
	pub(crate) fn publish_audit_head(&mut self, head: AuditHead) {
		self.audit_head = Some(head);
	}

	/// Records a permanent deletion for the ledger. Every method that
	/// removes an identity a restoration could bring back calls this.
	pub(crate) fn record_deletion(&mut self, deletion: PendingDeletion) {
		self.deletions.push(deletion);
	}

	/// Records a retired authority for the fences.
	pub(crate) fn record_fence(&mut self, fence: PendingFence) {
		self.fences.push(fence);
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
		// Only `work` is generic here: opening and releasing the snapshot
		// are compiled once rather than again for every caller.
		let mut transaction = self.begin_read().await?;
		let result = work(&mut transaction).await;
		transaction.release().await;
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
		// Only `work` is generic here: opening, committing and abandoning
		// the transaction are compiled once rather than again for every
		// caller.
		let mut transaction = self.begin_write().await?;
		match work(&mut transaction).await {
			Ok(value) => {
				self.commit(transaction).await?;
				Ok(value)
			}
			Err(error) => {
				transaction.abandon().await;
				Err(error)
			}
		}
	}

	async fn begin_read(&self) -> Result<ReadTransaction, StoreError> {
		let transaction = self.pool().begin_with("BEGIN DEFERRED").await?;
		Ok(ReadTransaction { transaction })
	}

	async fn begin_write(&self) -> Result<WriteTransaction, StoreError> {
		self.require_writable()?;
		// A write takes its lock up front. A deferred transaction that reads
		// before it writes cannot upgrade, and SQLite refuses it outright
		// rather than waiting on the busy handler.
		let transaction = self.pool().begin_with("BEGIN IMMEDIATE").await?;
		Ok(WriteTransaction {
			read: ReadTransaction { transaction },
			audit_head: None,
			deletions: vec![],
			fences: vec![],
		})
	}

	/// Commits what `work` built in `transaction`, with the ledgers that
	/// must precede the commit and the head that must follow it.
	///
	/// Boxed, so its future is compiled once here rather than again in every
	/// crate whose futures await a write.
	fn commit(
		&self,
		transaction: WriteTransaction,
	) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + '_>> {
		Box::pin(self.commit_unboxed(transaction))
	}

	async fn commit_unboxed(
		&self,
		mut transaction: WriteTransaction,
	) -> Result<(), StoreError> {
		let head = transaction.audit_head;
		// The ledger precedes the commit it describes. A crash in between
		// leaves a deletion the ledger holds and the store does not, which
		// the next open finishes; a commit first would leave a deletion a
		// restoration could undo (ADR-0102).
		let mut ledgered = None;
		if !transaction.deletions.is_empty() {
			let applied =
				match plane::deletions_applied(transaction.read.connection())
					.await
					.and_then(|applied| {
						deletion::append(
							&self.database,
							self.plane_id(),
							applied,
							&transaction.deletions,
						)
					}) {
					Ok(applied) => applied,
					Err(error) => {
						transaction.abandon().await;
						return Err(error);
					}
				};
			// The store commits how far it has applied the ledger with the
			// deletion itself, which is what tells a copy of it which
			// deletions it predates.
			if let Err(error) = plane::record_deletions_applied(
				transaction.read.connection(),
				applied,
			)
			.await
			{
				transaction.abandon().await;
				return Err(error);
			}
			ledgered = Some(applied);
		}
		// The fences precede the commit for the same reason (ADR-0070).
		let mut fenced = None;
		if !transaction.fences.is_empty() {
			let applied =
				match plane::fences_applied(transaction.read.connection())
					.await
					.and_then(|applied| {
						authority::append(
							&self.database,
							self.plane_id(),
							applied,
							&transaction.fences,
						)
					}) {
					Ok(applied) => applied,
					Err(error) => {
						transaction.abandon().await;
						return Err(error);
					}
				};
			if let Err(error) = plane::record_fences_applied(
				transaction.read.connection(),
				applied,
			)
			.await
			{
				transaction.abandon().await;
				return Err(error);
			}
			fenced = Some(applied);
		}
		transaction.read.transaction.commit().await?;
		// The head follows the commit it describes. A crash in between
		// leaves the store one or more records ahead of the head, which the
		// next start folds through and repairs; a head written first would
		// name a record no commit ever made (ADR-0105).
		if let Some(head) = head {
			audit_head::write(&self.database, self.plane_id(), head)?;
		}
		if let Some(applied) = ledgered {
			self.opened
				.write()
				.expect("store state is not poisoned")
				.deletions_applied = Some(applied);
		}
		if let Some(applied) = fenced {
			self.opened
				.write()
				.expect("store state is not poisoned")
				.fences_applied = Some(applied);
		}
		self.snapshots.mark_dirty();
		self.deep_checks.mark_dirty();
		Ok(())
	}
}

impl ReadTransaction {
	/// Ends a read snapshot by releasing its read mark. Awaiting that
	/// rollback keeps it from outliving the call; a rollback that fails
	/// must not mask what the caller produced.
	async fn release(self) {
		let _ = self.transaction.rollback().await;
	}
}

impl WriteTransaction {
	/// Rolls back every change. Dropping the transaction only enqueues its
	/// rollback; awaiting it releases the write lock before the caller sees
	/// the error.
	async fn abandon(self) {
		let _ = self.read.transaction.rollback().await;
	}
}
