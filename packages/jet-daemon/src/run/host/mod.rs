//! Concrete out-of-process Craft and helper connections, pinned by accepted digest.
mod launch;
pub(crate) use launch::{
	ConnectionMode, connect, craft_connection, failed, helper_config, receive,
	send, start,
};

use crate::run::craft::{self as run_craft, Contract};
use jet_core::{
	CoreError, ForkLaunchSource, LaunchPlan, PinnedCraft, RunFuture, RunHost,
	RunId, RunObservation, RunStartError,
};
use jet_protocol::{CraftCommand, CraftEvent, FrameReader, FrameWriter};
use std::{path::PathBuf, time::Duration};
use tokio::{
	net::unix::{OwnedReadHalf, OwnedWriteHalf},
	sync::Mutex,
};
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) use crate::craft::processes::CraftProcesses;

pub(crate) struct RunConnection {
	pub(crate) limits_subagents: bool,
	pub(crate) child_work: Mutex<Option<jet_core::ChildWork>>,
	pub(crate) broker: Option<crate::remote::no_visa_broker::Broker>,
	pub(crate) craft_minor: u32,
	pub(crate) reader: Mutex<FrameReader<OwnedReadHalf>>,
	pub(crate) writer: Mutex<FrameWriter<OwnedWriteHalf>>,
	pub(crate) helper_pid: u32,
	pub(crate) run_id: RunId,
}
#[expect(
	clippy::await_holding_invalid_type,
	reason = "independent read and write locks preserve partial frames while serializing complete outbound Craft messages"
)]
impl jet_core::RunConnection for RunConnection {
	fn constrain_children(
		&self,
		work: jet_core::ChildWork,
	) -> RunFuture<'_, Result<(), CoreError>> {
		self.apply_child_policy(work)
	}
	fn submit_turn(
		&self,
		turn_id: Uuid,
		prompt: String,
		child_work: jet_core::ChildWork,
	) -> RunFuture<'_, Result<(), CoreError>> {
		self.deliver_turn(turn_id, prompt, child_work)
	}
	fn receive(&self) -> RunFuture<'_, Result<RunObservation, CoreError>> {
		Box::pin(async move {
			let event: CraftEvent = loop {
				let event = receive(&mut *self.reader.lock().await).await?;
				if let CraftEvent::RemoteTool { call } = event {
					let broker = self.broker.as_ref().ok_or_else(|| {
						failed("remote tools were not admitted for this Run")
					})?;
					let operation_id = call.operation_id;
					let outcome = broker.call(call).await;
					send(
						&mut *self.writer.lock().await,
						&CraftCommand::RemoteToolResult {
							operation_id,
							outcome,
						},
					)
					.await?;
				} else {
					break event;
				}
			};
			if self.craft_minor < 3
				&& matches!(
					&event,
					CraftEvent::TurnStarted
						| CraftEvent::TurnEnded { .. }
						| CraftEvent::FileChanged { .. }
				) {
				return Err(failed("change evidence requires Craft 1.3"));
			}
			if self.craft_minor < 5
				&& matches!(
					&event,
					CraftEvent::ConversationTitle { .. }
						| CraftEvent::RunTitle { .. }
						| CraftEvent::ProcessTitle { .. }
				) {
				return Err(failed("native titles require Craft 1.5"));
			}
			if self.craft_minor < 7
				&& matches!(&event, CraftEvent::Usage { .. })
			{
				return Err(failed("Usage records require Craft 1.7"));
			}
			if self.craft_minor < 8
				&& matches!(&event, CraftEvent::ApprovalRequested { .. })
			{
				return Err(failed(
					"structured approval requests require Craft 1.8",
				));
			}
			Ok(match event {
				CraftEvent::RemoteTool { .. } => unreachable!("handled above"),
				CraftEvent::Model { model } => {
					if self.craft_minor < 10 {
						return Err(failed(
							"Model selection requires Craft 1.10",
						));
					}
					RunObservation::Model(jet_core::ModelId(model))
				}
				CraftEvent::Usage { usage } => RunObservation::Usage(
					crate::translate::usage::report(usage),
				),
				CraftEvent::ConversationTitle { title } => {
					RunObservation::ConversationTitle(title)
				}
				CraftEvent::RunTitle { title } => {
					RunObservation::RunTitle(title)
				}
				CraftEvent::ProcessTitle { pid, title } => {
					RunObservation::ProcessTitle { pid, title }
				}
				CraftEvent::ApprovalRequested { request } => {
					RunObservation::ApprovalRequested(
						jet_core::ApprovalRequest {
							request_id: request.request_id,
							tool: request.tool,
							action: request.action,
						},
					)
				}
				CraftEvent::TurnStarted => RunObservation::TurnStarted,
				CraftEvent::TurnEnded { outcome } => {
					RunObservation::TurnEnded(match outcome {
						jet_protocol::TurnOutcome::Completed => {
							jet_core::TurnOutcome::Completed
						}
						jet_protocol::TurnOutcome::Interrupted => {
							jet_core::TurnOutcome::Interrupted
						}
					})
				}
				CraftEvent::FileChanged { change } => {
					RunObservation::FileChanged(jet_core::ChangeEvidence {
						activity_id: change.activity_id,
						path: change.path,
						before_object: change.before_object,
						after_object: change.after_object,
						before_mode: change.before_mode,
						after_mode: change.after_mode,
						origin: jet_core::ChangeOrigin::Harness {
							run_id: self.run_id,
						},
					})
				}
				CraftEvent::RunStarted {
					helper_pid,
					harness_pid,
				} => {
					if helper_pid != self.helper_pid {
						return Err(failed("wrong helper identity"));
					}
					RunObservation::Started {
						helper_pid,
						harness_pid,
					}
				}
				CraftEvent::RunLaunchFailed => RunObservation::LaunchFailed,
				CraftEvent::RunRecovered { .. } => {
					return Err(failed("unexpected recovery handshake"));
				}
				CraftEvent::Activity { activity } => {
					RunObservation::Activity(activity_from_wire(activity))
				}
				CraftEvent::Output {
					native_event,
					presentation,
				} => RunObservation::Output {
					native_json: native_event.get().into(),
					presentation_json: presentation
						.into_iter()
						.map(|p| p.raw().get().to_owned())
						.collect(),
				},
				CraftEvent::Completed {
					id,
					native_conversation,
				} => {
					if id == self.run_id.0.to_string() {
						RunObservation::NativeConversation(native_conversation)
					} else {
						RunObservation::TurnCompleted {
							turn_id: id.parse().map_err(failed)?,
							native_conversation,
						}
					}
				}
				CraftEvent::RunEnded { exit_code } => {
					RunObservation::Ended(exit_code)
				}
				CraftEvent::Progress {
					source_offset,
					checkpoint,
				} => RunObservation::Progress {
					offset: source_offset,
					checkpoint,
				},
			})
		})
	}
	fn supports_native_cancellation(&self) -> bool {
		self.craft_minor >= 4
	}
	fn interrupt(&self, turn_id: Uuid) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async move {
			if !self.supports_native_cancellation() {
				return Err(failed("cancellation requires Craft 1.4"));
			}
			send(
				&mut *self.writer.lock().await,
				&CraftCommand::Interrupt {
					id: turn_id.to_string(),
				},
			)
			.await
		})
	}
	fn decide_approval<'a>(
		&'a self,
		request_id: &'a str,
		decision: jet_core::ReviewDecision,
	) -> RunFuture<'a, Result<(), CoreError>> {
		Box::pin(async move {
			// ASVS 8.3.1: exactly the request Core decided on, and only
			// once. There is no blanket answer to send, because the wire
			// has none to say (ADR-0012).
			send(
				&mut *self.writer.lock().await,
				&CraftCommand::Action {
					id: Uuid::now_v7().to_string(),
					action: jet_protocol::CraftAction::Approval {
						request_id: request_id.to_owned(),
						decision: match decision {
							jet_core::ReviewDecision::Allow => {
								jet_protocol::CraftApprovalDecision::AllowOnce
							}
							jet_core::ReviewDecision::Deny => {
								jet_protocol::CraftApprovalDecision::Deny
							}
						},
					},
				},
			)
			.await
		})
	}
	fn acknowledge(
		&self,
		source_offset: u64,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async move {
			send(
				&mut *self.writer.lock().await,
				&CraftCommand::Acknowledge { source_offset },
			)
			.await
		})
	}
	fn finish(&self) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async move {
			send(&mut *self.writer.lock().await, &CraftCommand::Shutdown).await
		})
	}
}
impl RunHost for CraftProcesses {
	fn craft_retirement_delay(&self) -> RunFuture<'_, Option<Duration>> {
		Box::pin(self.retirement_delay())
	}
	fn revoked_craft_digests(
		&self,
	) -> RunFuture<'_, Result<Vec<String>, CoreError>> {
		let key = self.release_key;
		let path = self.home.join("crafts/revocations.json");
		Box::pin(async move {
			let Some(key) = key else {
				return Ok(vec![]);
			};
			filesystem::blocking(move || {
				match crate::craft::revocation::load(&path, &key) {
					Ok(digests) => digests,
					Err(error) => {
						eprintln!(
							"jetd: ignored Craft revocation metadata: {error}"
						);
						vec![]
					}
				}
			})
			.await
		})
	}

	fn craft_id(&self, pin: &PinnedCraft) -> Result<String, CoreError> {
		Ok(jet_protocol::decode_control::<Contract>(
			pin.adapter_state.as_bytes(),
		)
		.map_err(failed)?
		.specification
		.id)
	}
	fn maintain_crafts(
		&self,
		active: Vec<PinnedCraft>,
		stopped: Vec<String>,
		force_disabled: Vec<String>,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(self.maintain(active, stopped, force_disabled))
	}

	fn validate_no_visa(&self, plan: &LaunchPlan) -> Result<(), CoreError> {
		crate::remote::no_visa_broker::Broker::prepare(
			self,
			plan,
			RunId(Uuid::nil()),
		)
		.map(|_| ())
	}
	fn native_provider(
		&self,
		craft: &PinnedCraft,
	) -> Result<jet_core::ProviderId, CoreError> {
		crate::run::craft::native_provider(craft)
	}
	fn harness(&self, craft: &PinnedCraft) -> Result<String, CoreError> {
		Ok(run_craft::Contract::of(craft)?.specification.harness)
	}
	fn pin(
		&self,
		home: PathBuf,
		id: String,
	) -> RunFuture<'_, Result<PinnedCraft, CoreError>> {
		Box::pin(async move { run_craft::load(&home, &id).await })
	}
	fn prepare_retry_run(
		&self,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<LaunchPlan, CoreError>> {
		Box::pin(run_craft::prepare_retry_run(plan))
	}
	fn prepare_next_run(
		&self,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<LaunchPlan, CoreError>> {
		Box::pin(run_craft::prepare_next_run(&self.home, plan))
	}
	fn prepare_fork(
		&self,
		plan: LaunchPlan,
		source: Option<ForkLaunchSource>,
	) -> RunFuture<'_, Result<LaunchPlan, CoreError>> {
		Box::pin(run_craft::prepare_fork(plan, source))
	}
	fn start(
		&self,
		home: PathBuf,
		run_id: RunId,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<Box<dyn jet_core::RunConnection>, RunStartError>>
	{
		Box::pin(async move {
			let (mut connection, command) = start(self, home, run_id, &plan)
				.await
				.map_err(|_| RunStartError::NotStarted)?;
			jet_core::RunConnection::constrain_children(
				&connection,
				plan.child_work,
			)
			.await
			.map_err(|_| RunStartError::NotStarted)?;
			send(connection.writer.get_mut(), &command)
				.await
				.map_err(|_| RunStartError::Unknown)?;
			Ok(Box::new(connection) as Box<dyn jet_core::RunConnection>)
		})
	}
	fn recover(
		&self,
		home: PathBuf,
		run_id: RunId,
		plan: LaunchPlan,
		cursor: jet_core::RunRecoveryCursor,
	) -> RunFuture<
		'_,
		Result<Box<dyn jet_core::RunConnection>, jet_core::RunRecoveryError>,
	> {
		Box::pin(crate::run::recovery::connect(
			self, home, run_id, plan, cursor,
		))
	}
	fn discover(
		&self,
		home: PathBuf,
	) -> RunFuture<'_, Result<Vec<RunId>, CoreError>> {
		Box::pin(crate::run::recovery::discover(home))
	}

	fn probe(
		&self,
		home: PathBuf,
		id: RunId,
		accepted: Option<LaunchPlan>,
		helper_pid: Option<u32>,
	) -> RunFuture<'_, Result<(), jet_core::RunRecoveryError>> {
		Box::pin(async move {
			if let Some(plan) = accepted {
				crate::run::recovery::validate_boot(&plan).await?;
			}
			crate::run::recovery::identity(home, id, helper_pid)
				.await
				.map(|_| ())
		})
	}
	fn describe(
		&self,
		home: PathBuf,
		id: RunId,
	) -> RunFuture<'_, Result<jet_core::ExecutionMetadata, CoreError>> {
		Box::pin(crate::run::recovery::describe(home, id))
	}
	fn validate_recovery(
		&self,
		home: PathBuf,
		id: RunId,
		plan: LaunchPlan,
		cursor: jet_core::RunRecoveryCursor,
	) -> RunFuture<'_, Result<(), jet_core::RunRecoveryError>> {
		Box::pin(async move {
			crate::run::recovery::validate(home, id, &plan, &cursor)
				.await
				.map(|_| ())
		})
	}

	fn signal(
		&self,
		home: PathBuf,
		run_id: RunId,
		signal: jet_core::ExecutionSignal,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(crate::run::execution_signal::deliver(home, run_id, signal))
	}
	fn terminate(
		&self,
		home: PathBuf,
		request: jet_core::ExecutionResolution,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(crate::run::execution_termination::terminate(home, request))
	}
}

fn activity_from_wire(
	value: jet_protocol::RunActivity,
) -> jet_core::RunActivity {
	match value {
		jet_protocol::RunActivity::Working => jet_core::RunActivity::Working,
		jet_protocol::RunActivity::WaitingForUser => {
			jet_core::RunActivity::WaitingForUser
		}
		jet_protocol::RunActivity::WaitingForApproval => {
			jet_core::RunActivity::WaitingForApproval
		}
		jet_protocol::RunActivity::WaitingForAuth => {
			jet_core::RunActivity::WaitingForAuth
		}
		jet_protocol::RunActivity::WaitingForQuota => {
			jet_core::RunActivity::WaitingForQuota
		}
		jet_protocol::RunActivity::Reconnecting => {
			jet_core::RunActivity::Reconnecting
		}
	}
}

// File operations run off the daemon's async connection workers.
pub(crate) mod filesystem {
	use jet_core::CoreError;
	pub(crate) async fn blocking<T: Send + 'static>(
		work: impl FnOnce() -> T + Send + 'static,
	) -> Result<T, CoreError> {
		tokio::task::spawn_blocking(work)
			.await
			.map_err(super::failed)
	}
}
