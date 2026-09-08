//! Translates bounded remote tools and runs isolated preauthorized workers.
use jet_core as core;
use jet_protocol as wire;

pub(crate) async fn answer(
	core: &core::Core,
	actor: &core::Actor,
	minor: u32,
	id: wire::RequestId,
	request: wire::RemoteToolRequest,
) -> wire::ServerMessage {
	if minor < wire::NO_VISA_MINOR {
		return wire::ServerMessage::Error {
			id: Some(id),
			error: crate::connection::wire_error(
				wire::ErrorCategory::Incompatible,
				"protocol.unsupported_minor",
				"No-Visa tools require a newer protocol minor".into(),
			),
		};
	}
	match core.remote_tool(actor, from_wire(request)).await {
		Ok(result) => wire::ServerMessage::RemoteToolResult {
			id,
			result: result_to_wire(result),
		},
		Err(error) => wire::ServerMessage::Error {
			id: Some(id),
			error: crate::translate::error(error, minor),
		},
	}
}

fn from_wire(request: wire::RemoteToolRequest) -> core::RemoteToolRequest {
	core::RemoteToolRequest {
		operation_id: request.operation_id,
		origin: core::NoVisaOrigin {
			plane_id: core::PlaneId(request.origin.plane_id),
			conversation_id: core::ConversationId(
				request.origin.conversation_id,
			),
			run_id: core::RunId(request.origin.run_id),
		},
		destination_plane_id: core::PlaneId(request.destination_plane_id),
		workspace_id: core::WorkspaceId(request.workspace_id),
		permissions: request
			.permissions
			.into_iter()
			.map(|permission| match permission {
				wire::BrokerPermission::RemoteTools => {
					core::BrokerPermission::RemoteTools
				}
				wire::BrokerPermission::ArtifactRead => {
					core::BrokerPermission::ArtifactRead
				}
				wire::BrokerPermission::ArtifactWrite => {
					core::BrokerPermission::ArtifactWrite
				}
			})
			.collect(),
		action: match request.action {
			wire::RemoteToolAction::Process {
				arguments,
				directory,
				environment,
			} => core::RemoteToolAction::Process {
				arguments,
				directory,
				environment: environment
					.into_iter()
					.map(|e| core::RemoteEnvironment {
						name: e.name,
						value: e.value,
					})
					.collect(),
			},
			wire::RemoteToolAction::Git { operation } => {
				core::RemoteToolAction::Git {
					operation: match operation {
						wire::RemoteGitOperation::Status => {
							core::RemoteGitOperation::Status
						}
						wire::RemoteGitOperation::Diff => {
							core::RemoteGitOperation::Diff
						}
					},
				}
			}
			wire::RemoteToolAction::Terminal {
				directory,
				input,
				rows,
				columns,
			} => core::RemoteToolAction::Terminal {
				directory,
				input,
				rows,
				columns,
			},
			wire::RemoteToolAction::ReadFile { path } => {
				core::RemoteToolAction::ReadFile { path }
			}
			wire::RemoteToolAction::WriteFile { path, content } => {
				core::RemoteToolAction::WriteFile { path, content }
			}
			wire::RemoteToolAction::Shell {
				directory,
				environment,
				script,
			} => core::RemoteToolAction::Shell {
				directory,
				script,
				environment: environment
					.into_iter()
					.map(|entry| core::RemoteEnvironment {
						name: entry.name,
						value: entry.value,
					})
					.collect(),
			},
		},
	}
}

pub(crate) fn to_wire(
	request: core::RemoteToolRequest,
) -> wire::RemoteToolRequest {
	wire::RemoteToolRequest {
		operation_id: request.operation_id,
		origin: wire::NoVisaOrigin {
			plane_id: request.origin.plane_id.0,
			conversation_id: request.origin.conversation_id.0,
			run_id: request.origin.run_id.0,
		},
		destination_plane_id: request.destination_plane_id.0,
		workspace_id: request.workspace_id.0,
		permissions: request
			.permissions
			.into_iter()
			.map(|permission| match permission {
				core::BrokerPermission::RemoteTools => {
					wire::BrokerPermission::RemoteTools
				}
				core::BrokerPermission::ArtifactRead => {
					wire::BrokerPermission::ArtifactRead
				}
				core::BrokerPermission::ArtifactWrite => {
					wire::BrokerPermission::ArtifactWrite
				}
			})
			.collect(),
		action: match request.action {
			core::RemoteToolAction::Process {
				arguments,
				directory,
				environment,
			} => wire::RemoteToolAction::Process {
				arguments,
				directory,
				environment: environment
					.into_iter()
					.map(|e| wire::RemoteEnvironment {
						name: e.name,
						value: e.value,
					})
					.collect(),
			},
			core::RemoteToolAction::Git { operation } => {
				wire::RemoteToolAction::Git {
					operation: match operation {
						core::RemoteGitOperation::Status => {
							wire::RemoteGitOperation::Status
						}
						core::RemoteGitOperation::Diff => {
							wire::RemoteGitOperation::Diff
						}
					},
				}
			}
			core::RemoteToolAction::Terminal {
				directory,
				input,
				rows,
				columns,
			} => wire::RemoteToolAction::Terminal {
				directory,
				input,
				rows,
				columns,
			},
			core::RemoteToolAction::ReadFile { path } => {
				wire::RemoteToolAction::ReadFile { path }
			}
			core::RemoteToolAction::WriteFile { path, content } => {
				wire::RemoteToolAction::WriteFile { path, content }
			}
			core::RemoteToolAction::Shell {
				directory,
				environment,
				script,
			} => wire::RemoteToolAction::Shell {
				directory,
				script,
				environment: environment
					.into_iter()
					.map(|entry| wire::RemoteEnvironment {
						name: entry.name,
						value: entry.value,
					})
					.collect(),
			},
		},
	}
}

fn result_to_wire(result: core::RemoteToolResult) -> wire::RemoteToolResult {
	match result {
		core::RemoteToolResult::Written => wire::RemoteToolResult::Written,
		core::RemoteToolResult::File { content } => {
			wire::RemoteToolResult::File { content }
		}
		core::RemoteToolResult::ApprovalRequired { operation_id } => {
			wire::RemoteToolResult::ApprovalRequired { operation_id }
		}
		core::RemoteToolResult::Process {
			exit_code,
			stdout,
			stderr,
		} => wire::RemoteToolResult::Process {
			exit_code,
			stdout,
			stderr,
		},
	}
}

pub(crate) async fn worker() -> std::process::ExitCode {
	use std::io::{Read, Write};
	let mut bytes = Vec::new();
	if std::io::stdin()
		.take(1_048_577)
		.read_to_end(&mut bytes)
		.is_err()
		|| bytes.len() > 1_048_576
	{
		return std::process::ExitCode::FAILURE;
	}
	let Ok(work) = wire::decode_control::<core::RemoteWork>(&bytes) else {
		return std::process::ExitCode::FAILURE;
	};
	let process = matches!(
		work.action,
		core::RemoteToolAction::Shell { .. }
			| core::RemoteToolAction::Process { .. }
			| core::RemoteToolAction::Git { .. }
	);
	let result = work.execute_in_worker().await;
	if process {
		// A successful process exec never returns. Keep launch failures separate
		// from the JSON envelope used for file workers.
		let _ = std::io::stderr()
			.write_all(b"Jet could not launch the admitted operation\n");
		return std::process::ExitCode::from(127);
	}
	let Ok(output) = serde_json::to_vec(&result) else {
		return std::process::ExitCode::FAILURE;
	};
	if std::io::stdout().write_all(&output).is_err() {
		return std::process::ExitCode::FAILURE;
	}
	std::process::ExitCode::SUCCESS
}
