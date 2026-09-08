//! Destination-selected native execution over local or authenticated remote Jet.
use crate::{Client, ClientError};
use jet_protocol::{CommandRequest, CommandResponse, Run, VisaRunRequest};
use uuid::Uuid;

impl Client {
	/// Admits a Visa Run on the selected Conversation Home Plane. Its lifetime
	/// belongs to that Plane, independently from this client connection.
	///
	/// # Errors
	/// Returns an older-protocol refusal, a destination validation error, or
	/// a transport error. An uncertain response must be retried with the same
	/// Command identity and selections.
	pub async fn start_visa_run(
		&self,
		command_id: Uuid,
		request: VisaRunRequest,
	) -> Result<Run, ClientError> {
		self.require_minor(jet_protocol::VISA_RUNS_MINOR)?;
		match self
			.execute_command(command_id, CommandRequest::StartVisaRun(request))
			.await?
		{
			CommandResponse::RunCreated(run) => Ok(run),
			other => Err(crate::requests::unexpected(&other)),
		}
	}
}
