//! Child policy and turn delivery share one transport lock, in that order.
use crate::run_host::{RunConnection, send};
use jet_core::{ChildWork, CoreError, RunFuture};
use jet_protocol::{CraftCommand, FrameWriter};
use tokio::net::unix::OwnedWriteHalf;
use uuid::Uuid;

#[expect(
	clippy::await_holding_invalid_type,
	reason = "serialize each child policy update with its following turn and transport write"
)]
impl RunConnection {
	pub(crate) fn apply_child_policy(
		&self,
		work: ChildWork,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async move {
			let mut applied = self.child_work.lock().await;
			let mut writer = self.writer.lock().await;
			self.write_policy(work, &mut applied, &mut writer).await
		})
	}
	pub(crate) fn deliver_turn(
		&self,
		id: Uuid,
		text: String,
		work: ChildWork,
	) -> RunFuture<'_, Result<(), CoreError>> {
		Box::pin(async move {
			let mut applied = self.child_work.lock().await;
			let mut writer = self.writer.lock().await;
			self.write_policy(work, &mut applied, &mut writer).await?;
			send(
				&mut writer,
				&CraftCommand::Turn {
					id: id.to_string(),
					text,
				},
			)
			.await
		})
	}
	async fn write_policy(
		&self,
		work: ChildWork,
		applied: &mut Option<ChildWork>,
		writer: &mut FrameWriter<OwnedWriteHalf>,
	) -> Result<(), CoreError> {
		if !self.limits_subagents || *applied == Some(work) {
			return Ok(());
		}
		let max_children = match work {
			ChildWork::Native => None,
			ChildWork::Paused => Some(0),
		};
		send(writer, &CraftCommand::ConstrainSubagents { max_children })
			.await?;
		*applied = Some(work);
		Ok(())
	}
}
