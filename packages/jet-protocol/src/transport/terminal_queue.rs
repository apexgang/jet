//! Ordered terminal delivery below ordinary control priority.
use super::*;
impl OutboundQueue {
	/// Queues terminal bytes whose credit has already been reserved at the source,
	/// or the gap/finish/attach control that must retain its order beside them.
	/// Ordinary replies remain prioritized; terminal control never overtakes bytes.
	///
	/// # Errors
	/// Rejects invalid, oversized, or backpressured frames without dropping bytes.
	pub fn queue_terminal_frame(
		&mut self,
		frame: Frame,
	) -> Result<(), StreamQueueError> {
		if frame.stream_id().is_connection() {
			return Err(StreamQueueError::ConnectionStream);
		}
		match &frame {
			Frame::Data { stream_id, payload } => {
				if payload.is_empty() {
					return Err(StreamQueueError::EmptyData);
				}
				if payload.len() > MAX_DATA_FRAME {
					return Err(StreamQueueError::OversizedData {
						declared: payload.len(),
						limit: MAX_DATA_FRAME,
					});
				}
				let next = self.binary_bytes.saturating_add(payload.len());
				if next > self.limits.binary_bytes {
					return Err(StreamQueueError::Backpressured {
						stream_id: *stream_id,
						queued_bytes: self.binary_bytes,
						limit: self.limits.binary_bytes,
					});
				}
				self.binary_bytes = next;
			}
			Frame::Control { payload, .. } => {
				if payload.is_empty() {
					return Err(StreamQueueError::EmptyControl);
				}
				let next = self.control_bytes.saturating_add(payload.len());
				if payload.len() > MAX_CONTROL_FRAME
					|| next > self.limits.control_bytes
				{
					return Err(StreamQueueError::ControlBackpressured {
						limit: self.limits.control_bytes,
					});
				}
				self.control_bytes = next;
			}
		}
		self.ordered.push_back(frame);
		Ok(())
	}
}
