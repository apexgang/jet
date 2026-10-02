//! A connection that only executes No-Visa destination operations.

use super::{Client, ClientError, Reply, expect_reply_to, remote_error};
use crate::{ClientIdentity, SshEndpoint};
use jet_protocol::{
	ClientMessage, NO_VISA_MINOR, RemoteToolRequest, RemoteToolResult,
	RequestId, WireError,
};

/// The [`jet_protocol::ServerMessage`] kinds a destination operation can be
/// answered with, decoded exactly as that message decodes them. Any other
/// reply fails to decode, which ends the connection as a malformed reply
/// does.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RemoteToolReply {
	/// Outcome of one No-Visa operation.
	RemoteToolResult {
		/// Request correlation.
		id: RequestId,
		/// Bounded destination result.
		result: RemoteToolResult,
	},
	/// A request failed, or the connection is being refused.
	Error {
		/// Identifier of the request that failed, if any.
		id: Option<RequestId>,
		/// Stable error body.
		error: WireError,
	},
}

impl Reply for RemoteToolReply {
	fn connection_error(&self) -> Option<&WireError> {
		match self {
			Self::Error { id: None, error } => Some(error),
			Self::Error { id: Some(_), .. } | Self::RemoteToolResult { .. } => {
				None
			}
		}
	}

	fn disconnected(error: WireError) -> Self {
		Self::Error { id: None, error }
	}
}

/// A signed SSH connection that only executes No-Visa destination
/// operations.
///
/// It behaves as [`Client::remote_tool`] on a [`Client::connect_ssh`]
/// connection, except that it reads only the replies such an operation can
/// receive: a destination that answers with anything else ends the
/// connection, so the operation fails with [`ClientError::Closed`] rather
/// than [`ClientError::Unexpected`].
#[derive(Debug)]
pub struct RemoteToolClient(Client<RemoteToolReply>);

impl RemoteToolClient {
	/// Launches the system SSH client and proves the Paired Client identity.
	/// The subprocess remains owned by this client and exits on drop.
	///
	/// # Errors
	/// Returns endpoint/SSH failures separately from Jet handshake refusals.
	pub async fn connect_ssh(
		endpoint: &SshEndpoint,
		identity: &impl ClientIdentity,
	) -> Result<Self, ClientError> {
		Client::launch_ssh(endpoint, identity).await.map(Self)
	}

	/// Executes a bounded destination operation under this signed connection.
	/// Preserve the operation identity on retry; a lost mutation reply is uncertain.
	/// # Errors
	/// Returns a destination refusal, incompatible minor, or transport failure.
	pub async fn remote_tool(
		&self,
		request: RemoteToolRequest,
	) -> Result<RemoteToolResult, ClientError> {
		let client = &self.0;
		client.require_minor(NO_VISA_MINOR)?;
		let id = client.next_id();
		let reply = client
			.exchange(
				client.request_stream(),
				&ClientMessage::RemoteTool { id, request },
			)
			.await?;
		match reply {
			RemoteToolReply::RemoteToolResult {
				id: reply_id,
				result,
			} => expect_reply_to(id, reply_id, result),
			RemoteToolReply::Error {
				id: reply_id,
				error,
			} => Err(remote_error(id, reply_id, error)),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{RemoteToolClient, RemoteToolReply};
	use crate::{Client, ClientError, connection::Reply};
	use jet_protocol::{
		CODEC_JSON_V1, CONNECTION_STREAM, ErrorCategory, Frame, FrameReader,
		FrameWriter, NoVisaOrigin, PROTOCOL_MINOR, PROTOCOL_VERSION,
		PlaneStatus, QueryResponse, RemoteToolAction, RemoteToolRequest,
		RemoteToolResult, ServerHello, ServerMessage, WireError,
		decode_control, encode_control,
	};
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	fn error() -> WireError {
		WireError {
			category: ErrorCategory::Conflict,
			code: "test.refused".into(),
			retryable: false,
			message: "refused".into(),
			revision_conflict: None,
			restart: None,
			recovery_actions: vec![],
		}
	}

	/// Every reply kind, and payloads that are not quite one. The first
	/// request a client sends has the identity 1.
	fn payloads() -> Vec<Vec<u8>> {
		let error_json =
			String::from_utf8(encode_control(&error()).unwrap()).unwrap();
		let mut payloads: Vec<_> = [
			ServerMessage::RemoteToolResult {
				id: 1,
				result: RemoteToolResult::Written,
			},
			ServerMessage::RemoteToolResult {
				id: 2,
				result: RemoteToolResult::Written,
			},
			ServerMessage::Error {
				id: None,
				error: error(),
			},
			ServerMessage::Error {
				id: Some(1),
				error: error(),
			},
			ServerMessage::Error {
				id: Some(2),
				error: error(),
			},
			ServerMessage::TerminalAttached { id: 1 },
			ServerMessage::TerminalResized { id: 1 },
			ServerMessage::QueryResult {
				id: 1,
				result: QueryResponse::Status(PlaneStatus {
					cursor: Some(0),
					plane_id: Uuid::nil(),
					daemon_starts: 1,
					started_at_unix_ms: 0,
					core_version: "0.2.0".into(),
					security: None,
					recovery: None,
				}),
			},
		]
		.iter()
		.map(|message| encode_control(message).unwrap())
		.collect();
		payloads.extend(
			[
				r#"{"kind":"remote_tool_result","id":1,"result":{"type":"written"},"extra":true}"#.to_owned(),
				r#"{"result":{"type":"written"},"id":1,"kind":"remote_tool_result"}"#.to_owned(),
				r#"["remote_tool_result",1,{"type":"written"}]"#.to_owned(),
				r#"{"kind":"remote_tool_result","id":1}"#.to_owned(),
				r#"{"kind":"remote_tool_result","kind":"remote_tool_result","id":1,"result":{"type":"written"}}"#.to_owned(),
				r#"{"kind":0,"id":1,"result":{"type":"written"}}"#.to_owned(),
				r#"{"kind":"Remote_Tool_Result","id":1,"result":{"type":"written"}}"#.to_owned(),
				format!(r#"{{"kind":"error","id":1,"error":{error_json},"extra":[]}}"#),
				format!(r#"{{"kind":"error","error":{error_json}}}"#),
				format!(r#"["error",1,{error_json}]"#),
				format!(r#"["error",null,{error_json}]"#),
				format!(r#"{{"kind":5,"id":1,"error":{error_json}}}"#),
				r#"{"kind":"error","id":"1","error":{}}"#.to_owned(),
				r#"{"kind":"error","error":{"category":"conflict","code":"c","retryable":false,"message":"m"}}"#.to_owned(),
				r#"{"kind":"query_result","id":1,"result":{}}"#.to_owned(),
				r#"["terminal_attached",1]"#.to_owned(),
				r#"{"kind":"unknown","id":1}"#.to_owned(),
				r#"{"id":1}"#.to_owned(),
				"[]".to_owned(),
				"{}".to_owned(),
				"null".to_owned(),
				r#""error""#.to_owned(),
			]
			.map(String::into_bytes),
		);
		payloads
	}

	#[test]
	fn remote_tool_replies_decode_exactly_as_server_messages_do() {
		let narrow: Vec<_> = payloads()
			.iter()
			.map(|payload| {
				decode_control::<RemoteToolReply>(payload)
					.ok()
					.map(|reply| match reply {
						RemoteToolReply::RemoteToolResult { id, result } => {
							ServerMessage::RemoteToolResult { id, result }
						}
						RemoteToolReply::Error { id, error } => {
							ServerMessage::Error { id, error }
						}
					})
			})
			.collect();
		let full: Vec<_> = payloads()
			.iter()
			.map(|payload| {
				decode_control::<ServerMessage>(payload).ok().filter(
					|message| {
						matches!(
							message,
							ServerMessage::RemoteToolResult { .. }
								| ServerMessage::Error { .. }
						)
					},
				)
			})
			.collect();

		assert_eq!(narrow, full);
	}

	#[test]
	fn error_frames_are_recognized_exactly_as_server_messages_are() {
		let narrow: Vec<_> = payloads()
			.iter()
			.map(|payload| super::super::is_error(payload))
			.collect();
		let full: Vec<_> = payloads()
			.iter()
			.map(|payload| {
				matches!(
					decode_control::<ServerMessage>(payload),
					Ok(ServerMessage::Error { .. })
				)
			})
			.collect();

		assert_eq!(narrow, full);
	}

	/// What the No-Visa broker reports for an operation. It maps every
	/// client error other than a refusal to one unconfirmed outcome.
	#[derive(Debug, PartialEq)]
	enum Brokered {
		Completed(RemoteToolResult),
		Failed(WireError),
		Unconfirmed,
	}

	fn brokered(result: Result<RemoteToolResult, ClientError>) -> Brokered {
		match result {
			Ok(result) => Brokered::Completed(result),
			Err(ClientError::Remote(error) | ClientError::Rejected(error)) => {
				Brokered::Failed(error)
			}
			Err(_) => Brokered::Unconfirmed,
		}
	}

	/// A client that reads its replies as `M`, connected to a peer that
	/// answers the first request with `reply` on `stream` (the request's own
	/// stream when `None`) and then holds the connection open.
	fn connect<M: Reply>(
		reply: Vec<u8>,
		stream: Option<jet_protocol::StreamId>,
	) -> Client<M> {
		let (client, peer) = tokio::io::duplex(1 << 20);
		let (read, write) = tokio::io::split(client);
		let (peer_read, peer_write) = tokio::io::split(peer);
		tokio::spawn(async move {
			let mut reader = FrameReader::new(peer_read);
			let mut writer = FrameWriter::new(peer_write);
			reader.enable_multiplexing();
			writer.enable_multiplexing();
			let request = reader.read().await.unwrap();
			let stream = stream.unwrap_or_else(|| request.stream_id());
			writer
				.write(&Frame::stream_control(stream, reply))
				.await
				.unwrap();
			while reader.read().await.is_ok() {}
		});
		Client::from_handshake(
			FrameReader::new(read),
			FrameWriter::new(write),
			ServerHello::Welcome {
				protocol: PROTOCOL_VERSION,
				minor: PROTOCOL_MINOR,
				codec: CODEC_JSON_V1.into(),
				max_control_frame: 1_048_576,
				max_data_frame: 262_144,
				capabilities: vec![],
			},
		)
		.unwrap()
	}

	fn request() -> RemoteToolRequest {
		RemoteToolRequest {
			operation_id: Uuid::nil(),
			origin: NoVisaOrigin {
				plane_id: Uuid::nil(),
				conversation_id: Uuid::nil(),
				run_id: Uuid::nil(),
			},
			destination_plane_id: Uuid::nil(),
			workspace_id: Uuid::nil(),
			permissions: vec![],
			action: RemoteToolAction::ReadFile { path: "a".into() },
		}
	}

	/// A reply this connection does not read ends it, where a full client
	/// would return the reply as unexpected; the broker reports both alike.
	#[tokio::test]
	async fn the_broker_reports_every_reply_as_a_full_client_would() {
		let mut narrow = vec![];
		let mut full = vec![];
		for payload in payloads() {
			for stream in [None, Some(CONNECTION_STREAM)] {
				let client = RemoteToolClient(connect(payload.clone(), stream));
				narrow.push(brokered(client.remote_tool(request()).await));
				let client = connect::<ServerMessage>(payload.clone(), stream);
				full.push(brokered(client.remote_tool(request()).await));
			}
		}

		assert_eq!(narrow, full);
		assert!(
			narrow.contains(&Brokered::Completed(RemoteToolResult::Written))
		);
		assert!(narrow.contains(&Brokered::Failed(error())));
	}

	#[tokio::test]
	async fn a_connection_error_disconnects_the_pending_operation() {
		let payload = encode_control(&ServerMessage::Error {
			id: None,
			error: error(),
		})
		.unwrap();

		let narrow =
			RemoteToolClient(connect(payload.clone(), Some(CONNECTION_STREAM)))
				.remote_tool(request())
				.await;
		let full = connect::<ServerMessage>(payload, Some(CONNECTION_STREAM))
			.remote_tool(request())
			.await;

		assert!(
			matches!(&narrow, Err(ClientError::Disconnected(disconnected)) if *disconnected == error()),
			"{narrow:?}"
		);
		assert!(
			matches!(&full, Err(ClientError::Disconnected(disconnected)) if *disconnected == error()),
			"{full:?}"
		);
	}
}
