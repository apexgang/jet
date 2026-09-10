//! Direct user edits and structured review submissions (ADR-0041).

use crate::{
	connection::{Client, ClientError},
	requests::unexpected,
};
use jet_protocol::{
	CommandRequest, CommandResponse, EditableFile, FileRevision, FileTarget,
	QueryRequest, QueryResponse, ReviewComment, Turn,
};
use uuid::Uuid;

impl Client {
	/// Reads bounded UTF-8 content and the exact Revision required to edit it.
	///
	/// # Errors
	///
	/// Returns a stable target, path, content, or transport error.
	pub async fn editable_file(
		&self,
		target: FileTarget,
		path: &str,
	) -> Result<EditableFile, ClientError> {
		self.require_minor(jet_protocol::USER_INPUT_MINOR)?;
		match self
			.query(QueryRequest::EditableFile {
				target,
				path: path.into(),
			})
			.await?
		{
			QueryResponse::EditableFile(file) => Ok(file),
			other => Err(unexpected(&other)),
		}
	}

	/// Replaces one file only if its exact Revision is still current.
	///
	/// # Errors
	///
	/// Returns `user_edit.stale_revision` with a refresh action when the file
	/// moved on, or another stable path, content, or transport error.
	pub async fn apply_user_edit(
		&self,
		command_id: Uuid,
		target: FileTarget,
		path: &str,
		expected_revision: FileRevision,
		content: &str,
	) -> Result<FileRevision, ClientError> {
		self.require_minor(jet_protocol::USER_INPUT_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::ApplyUserEdit {
					target,
					path: path.into(),
					expected_revision,
					content: content.into(),
				},
			)
			.await?
		{
			CommandResponse::UserEditApplied { revision, .. } => Ok(revision),
			other => Err(unexpected(&other)),
		}
	}

	/// Preserves a review batch as one structured queued user Turn.
	///
	/// # Errors
	///
	/// Returns a stable validation, queue, or transport error.
	pub async fn submit_review(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
		comments: Vec<ReviewComment>,
	) -> Result<Turn, ClientError> {
		self.require_minor(jet_protocol::USER_INPUT_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::SubmitReview {
					conversation_id,
					comments,
				},
			)
			.await?
		{
			CommandResponse::TurnAdmitted { turn } => Ok(turn),
			other => Err(unexpected(&other)),
		}
	}
}
