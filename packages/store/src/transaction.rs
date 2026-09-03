//! Units of work over the store. A [`ReadTransaction`] sees one consistent
//! snapshot; a [`WriteTransaction`] commits every change or none of them,
//! so current state and its journal Events always land together
//! (ADR-0020, ADR-0071).

use std::ops::Deref;

use rusqlite::{Transaction, TransactionBehavior};

use crate::{Store, StoreError};

/// One consistent read snapshot of the store.
pub struct ReadTransaction<'a> {
	pub(crate) transaction: Transaction<'a>,
}

/// One atomic set of changes, readable while it is being built.
pub struct WriteTransaction<'a>(ReadTransaction<'a>);

impl<'a> Deref for WriteTransaction<'a> {
	type Target = ReadTransaction<'a>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl Store {
	/// Runs `work` against one consistent snapshot.
	///
	/// # Errors
	///
	/// Returns the error `work` produced, or a [`StoreError`] converted into
	/// `E` when the snapshot cannot be opened.
	pub fn read<T, E: From<StoreError>>(
		&self,
		work: impl FnOnce(&ReadTransaction<'_>) -> Result<T, E>,
	) -> Result<T, E> {
		let mut connection = self.lock();
		let transaction = connection
			.transaction_with_behavior(TransactionBehavior::Deferred)
			.map_err(StoreError::from)?;
		work(&ReadTransaction { transaction })
	}

	/// Runs `work` as one durable transaction: every change commits when
	/// `work` returns `Ok`, and none of them persist when it returns `Err`.
	///
	/// # Errors
	///
	/// Returns the error `work` produced, or a [`StoreError`] converted into
	/// `E` when the transaction cannot be opened or committed.
	pub fn write<T, E: From<StoreError>>(
		&self,
		work: impl FnOnce(&WriteTransaction<'_>) -> Result<T, E>,
	) -> Result<T, E> {
		let mut connection = self.lock();
		let transaction = connection
			.transaction_with_behavior(TransactionBehavior::Immediate)
			.map_err(StoreError::from)?;
		let transaction = WriteTransaction(ReadTransaction { transaction });
		let result = work(&transaction)?;
		transaction
			.0
			.transaction
			.commit()
			.map_err(StoreError::from)?;
		Ok(result)
	}
}
