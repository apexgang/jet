//! Destination authorization for No-Visa tools; Pairing is the only trust grant.
use crate::{
	Actor, Core, CoreError, RemoteEnvironment, RemoteToolAction,
	RemoteToolRequest, RemoteToolResult,
};

impl Core {
	/// Installs the trusted daemon executable used for disposable remote workers.
	pub fn with_remote_worker(
		mut self,
		executable: std::path::PathBuf,
	) -> Self {
		self.remote_worker = Some(executable);
		self
	}

	/// Authorizes and executes one destination tool under live Pairing authority.
	///
	/// # Errors
	/// Refuses unpaired callers, undeclared permissions, wrong Planes, and paths
	/// outside a registered Workspace. Native errors never reach the peer.
	pub async fn remote_tool(
		&self,
		actor: &Actor,
		request: RemoteToolRequest,
	) -> Result<RemoteToolResult, CoreError> {
		let result = self.remote_tool_admitted(actor, &request).await;
		if result.is_err() && matches!(actor, Actor::RemoteClient { .. }) {
			// Admission refusals are attributable without recording command content.
			self.store
				.write(async |tx| {
					audit(
						tx,
						actor,
						actor.client_id(),
						&request,
						"remote.refused",
						crate::AuditOutcome::Denied,
						self.now_unix_ms(),
					)
					.await
				})
				.await?;
		}
		result
	}

	async fn remote_tool_admitted(
		&self,
		actor: &Actor,
		request: &RemoteToolRequest,
	) -> Result<RemoteToolResult, CoreError> {
		let _slot = self.remote_tool_slots.try_acquire().map_err(|_| {
			CoreError::conflict(
				"remote.busy",
				"the destination remote operation limit was reached",
			)
		})?;
		let access = self
			.remote_access
			.acquire()
			.await
			.expect("authority gate never closes");
		let Actor::RemoteClient { session } = actor else {
			return Err(crate::remote::unauthorized());
		};
		self.remote_sessions.authorize(session)?;
		// ASVS 8.3.1–8.3.3: full Pairing trust does not bypass the originating
		// Craft's declarations or the destination's own root validation.
		if !request
			.permissions
			.contains(&crate::BrokerPermission::RemoteTools)
		{
			return Err(CoreError::invalid_input(
				"remote.permission_denied",
				"the accepted Craft did not declare remote_tools",
			));
		}
		let root = self
			.store
			.read(async |tx| {
				if tx.plane().await?.plane_id != request.destination_plane_id.0
				{
					return Err(CoreError::invalid_input(
						"remote.destination_mismatch",
						"the request names another destination Plane",
					));
				}
				let workspace = tx
					.workspace(request.workspace_id.0)
					.await?
					.ok_or_else(|| {
						CoreError::not_found(
							"workspace.not_found",
							"the Workspace does not exist",
						)
					})?;
				if tx.project(workspace.project_id).await?.is_none() {
					return Err(CoreError::not_found(
						"project.not_found",
						"the Project is no longer registered",
					));
				}
				Ok::<_, CoreError>(std::path::PathBuf::from(workspace.root))
			})
			.await?;
		request.action.validate()?;
		if !matches!(
			request.action,
			RemoteToolAction::ReadFile { .. } | RemoteToolAction::Git { .. }
		) {
			self.security()
				.await
				.admit(crate::security::SecurityClass::Guarded)?;
		}
		let client_id = actor.client_id().0.to_string();
		let operation_id = request.operation_id.to_string();
		let encoded = serde_json::to_string(&request).map_err(codec)?;
		use sha2::Digest;
		let digest = sha2::Sha256::digest(encoded.as_bytes()).to_vec();
		let previous = self
			.store
			.write(async |tx| {
				tx.expire_remote_operations(
					self.now_unix_ms()
						- crate::command_receipt::COMMAND_RETENTION_MS,
				)
				.await?;
				tx.expire_remote_reviews(self.now_unix_ms() - 600_000)
					.await?;
				if let Some(record) =
					tx.remote_operation(&client_id, &operation_id).await?
				{
					if record.request_digest != digest {
						return Err(CoreError::conflict(
							"remote.identity_reused",
							"the operation identity already names different content",
						));
					}
					if record.state == "denied" {
						return Ok(Some(Err(CoreError::conflict(
							"remote.denied",
							"the remote operation was denied",
						))));
					}
					if matches!(record.state.as_str(), "pending" | "approved")
						&& self
							.now_unix_ms()
							.saturating_sub(record.recorded_at_unix_ms)
							> 600_000
					{
						return Ok(Some(Err(CoreError::conflict(
							"remote.review_expired",
							"the remote action review expired",
						))));
					}
					if record.state == "approved" {
						tx.transition_remote_operation(
							&client_id,
							&operation_id,
							"approved",
							"running",
						)
						.await?;
						return Ok(None);
					}
					if record.state == "pending" {
						return Ok(Some(Ok(
							RemoteToolResult::ApprovalRequired {
								operation_id: request.operation_id,
							},
						)));
					}
					return Ok(Some(match record.result {
						Some(result) => {
							serde_json::from_str(&result).map_err(codec)?
						}
						None => Err(CoreError::conflict(
							"remote.outcome_unknown",
							"the recorded operation cannot safely be repeated",
						)),
					}));
				}
				tx.insert_remote_operation(&jet_store::RemoteOperationRecord {
					client_id: client_id.clone(),
					operation_id: operation_id.clone(),
					request_digest: digest,
					origin_plane_id: request.origin.plane_id.0.to_string(),
					origin_conversation_id: request
						.origin
						.conversation_id
						.0
						.to_string(),
					origin_run_id: request.origin.run_id.0.to_string(),
					workspace_id: request.workspace_id.0.to_string(),
					recorded_at_unix_ms: self.now_unix_ms(),
					request: Some(encoded),
					state: if request.action.needs_review() {
						"pending"
					} else {
						"running"
					}
					.into(),
					result: None,
				})
				.await?;
				audit(
					tx,
					actor,
					actor.client_id(),
					request,
					"remote.admitted",
					crate::AuditOutcome::Succeeded,
					self.now_unix_ms(),
				)
				.await?;
				Ok::<_, CoreError>(request.action.needs_review().then_some(Ok(
					RemoteToolResult::ApprovalRequired {
						operation_id: request.operation_id,
					},
				)))
			})
			.await?;
		if let Some(previous) = previous {
			return previous;
		}
		drop(access);
		let result = self
			.perform_remote_work(
				session,
				crate::RemoteWork {
					root,
					action: request.action.clone(),
				},
			)
			.await;
		self.store
			.write(async |tx| {
				tx.finish_remote_operation(
					&client_id,
					&operation_id,
					&serde_json::to_string(&result).map_err(codec)?,
				)
				.await?;
				audit(
					tx,
					actor,
					actor.client_id(),
					request,
					"remote.finished",
					if result.is_ok() {
						crate::AuditOutcome::Succeeded
					} else {
						crate::AuditOutcome::Failed
					},
					self.now_unix_ms(),
				)
				.await
			})
			.await?;
		result
	}
}

impl RemoteToolAction {
	pub(crate) fn needs_review(&self) -> bool {
		matches!(
			self,
			Self::Shell { .. } | Self::Process { .. } | Self::Terminal { .. }
		)
	}

	pub(crate) fn validate(&self) -> Result<(), CoreError> {
		let path = match self {
			Self::Terminal {
				directory,
				input,
				rows,
				columns,
			} => {
				if input.is_empty()
					|| input.len() > 16384
					|| input.contains('\0')
					|| !(1..=1000).contains(rows)
					|| !(1..=1000).contains(columns)
				{
					return Err(CoreError::invalid_input(
						"remote.invalid_terminal",
						"terminal input and dimensions exceed their bounds",
					));
				}
				if directory.is_empty() {
					return Ok(());
				}
				directory
			}
			Self::Git { .. } => return Ok(()),
			Self::Process {
				arguments,
				directory,
				environment,
			} => {
				if arguments.is_empty()
					|| arguments.len() > 128
					|| arguments[0].is_empty()
					|| arguments.iter().map(String::len).sum::<usize>() > 16384
					|| arguments.iter().any(|a| a.contains('\0'))
				{
					return Err(CoreError::invalid_input(
						"remote.invalid_arguments",
						"process arguments must be a bounded, NUL-free array with an executable",
					));
				}
				validate_environment(environment)?;
				if directory.is_empty() {
					return Ok(());
				}
				directory
			}
			Self::Shell {
				directory,
				environment,
				script,
			} => {
				if script.is_empty()
					|| script.len() > 16384
					|| script.contains('\0')
				{
					return Err(CoreError::invalid_input(
						"remote.invalid_script",
						"shell source must contain 1 to 16384 bytes without NULs",
					));
				}
				validate_environment(environment)?;
				if directory.is_empty() {
					return Ok(());
				}
				directory
			}
			Self::ReadFile { path } => path,
			Self::WriteFile { path, content } => {
				if content.len() > 65536 {
					return Err(CoreError::invalid_input(
						"remote.too_large",
						"remote file content exceeds 64 KiB",
					));
				}
				if path
					.split('/')
					.any(|component| component.eq_ignore_ascii_case(".git"))
				{
					return Err(CoreError::invalid_input(
						"remote.protected_path",
						"remote file writes cannot alter Git metadata",
					));
				}
				path
			}
		};
		crate::RelativePath::parse(path)?;
		Ok(())
	}
}

pub(crate) async fn audit(
	tx: &mut jet_store::WriteTransaction,
	actor: &Actor,
	source_client: crate::ClientId,
	request: &RemoteToolRequest,
	decision: &str,
	outcome: crate::AuditOutcome,
	now: i64,
) -> Result<(), CoreError> {
	// ASVS 16.2.1–16.2.5: retain paired identity, origin and Workspace, never
	// file contents, arguments, environment values, or terminal output.
	tx.append_audit_record(jet_store::NewAuditRecord {
		record_id: uuid::Uuid::now_v7(),
		recorded_at_unix_ms: now,
		actor: actor.record().into(),
		target_kind: "remote_operation".into(),
		target_id: Some(format!(
			"{}/{}",
			source_client.0, request.operation_id
		)),
		decision: decision.into(),
		risk: if request.action.needs_review() {
			crate::AuditRisk::Elevated
		} else {
			crate::AuditRisk::Routine
		},
		outcome,
	})
	.await?;
	Ok(())
}

pub(crate) fn codec(error: serde_json::Error) -> CoreError {
	CoreError::internal("remote.invalid_record", error.to_string())
}

pub(crate) fn unavailable(error: std::io::Error) -> CoreError {
	CoreError::unavailable(
		"remote.io_failed",
		"the remote file operation failed",
		error.to_string(),
	)
}

pub(crate) fn validate_environment(
	environment: &[RemoteEnvironment],
) -> Result<(), CoreError> {
	if environment.len() > 32
		|| environment
			.iter()
			.map(|entry| {
				entry.name.len() + entry.value.as_ref().map_or(0, String::len)
			})
			.sum::<usize>()
			> 8192
	{
		return Err(CoreError::invalid_input(
			"remote.invalid_environment",
			"explicit environment changes exceed their bound",
		));
	}
	for (index, entry) in environment.iter().enumerate() {
		if entry.name.is_empty()
			|| entry.name.len() > 128
			|| !entry
				.name
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'_')
			|| entry.name.as_bytes()[0].is_ascii_digit()
			|| entry
				.value
				.as_ref()
				.is_some_and(|value| value.contains('\0'))
			|| environment[..index]
				.iter()
				.any(|previous| previous.name == entry.name)
		{
			return Err(CoreError::invalid_input(
				"remote.invalid_environment",
				"environment changes require unique variable names and NUL-free values",
			));
		}
	}
	Ok(())
}
