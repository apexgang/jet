//! Daily schedules on retained Conversations (ADR-0024).
use super::unexpected;
use crate::{Client, ClientError};
use jet_protocol::{
	CommandRequest, CommandResponse, QueryRequest, QueryResponse,
	ScheduledTask, ScheduledTasks,
};
use uuid::Uuid;

impl Client {
	/// Reads enabled schedules for one Conversation.
	///
	/// # Errors
	/// Returns protocol feature, remote refusal, or transport failures.
	pub async fn scheduled_tasks(
		&self,
		conversation_id: Uuid,
	) -> Result<ScheduledTasks, ClientError> {
		self.require_minor(jet_protocol::SCHEDULES_MINOR)?;
		match self
			.query(QueryRequest::ScheduledTasks { conversation_id })
			.await?
		{
			QueryResponse::ScheduledTasks(snapshot) => Ok(snapshot),
			other => Err(unexpected(&other)),
		}
	}

	/// Creates a daily schedule under a caller-kept durable command identity.
	/// The service validates the zone, time, prompt and Conversation retention.
	///
	/// # Errors
	/// Returns protocol feature, remote refusal, or transport failures.
	pub async fn create_schedule(
		&self,
		command_id: Uuid,
		conversation_id: Uuid,
		time_zone: String,
		local_time: String,
		prompt: String,
	) -> Result<ScheduledTask, ClientError> {
		self.require_minor(jet_protocol::SCHEDULES_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::CreateSchedule {
					conversation_id,
					time_zone,
					local_time,
					prompt,
				},
			)
			.await?
		{
			CommandResponse::ScheduleCreated { task } => Ok(task),
			other => Err(unexpected(&other)),
		}
	}

	/// Cancels future firings and withdraws this schedule's pending input.
	///
	/// # Errors
	/// Returns protocol feature, remote refusal, or transport failures.
	pub async fn cancel_schedule(
		&self,
		command_id: Uuid,
		schedule_id: Uuid,
	) -> Result<(), ClientError> {
		self.require_minor(jet_protocol::SCHEDULES_MINOR)?;
		match self
			.execute_command(
				command_id,
				CommandRequest::CancelSchedule { schedule_id },
			)
			.await?
		{
			CommandResponse::ScheduleCanceled {
				schedule_id: returned,
			} if returned == schedule_id => Ok(()),
			other => Err(unexpected(&other)),
		}
	}
}
