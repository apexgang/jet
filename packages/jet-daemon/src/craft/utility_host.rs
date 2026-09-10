//! An accepted Craft's isolated Utility entrypoint; never a Run or native session.
use jet_core::{
	AccountBinding, CoreError, PinnedCraft, RunFuture, UtilityHost,
	UtilityInput, UtilityModel, UtilityReply,
};
use jet_protocol::{
	CraftHostAccess, CraftUtilityModel, CraftUtilityReply, CraftUtilityRequest,
};
use std::{path::PathBuf, process::Stdio};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	process::Command,
};

#[derive(Debug)]
pub(crate) struct Utilities {
	pub(crate) home: PathBuf,
}
impl UtilityHost for Utilities {
	fn select<'a>(
		&'a self,
		binding: &'a AccountBinding,
	) -> RunFuture<'a, Result<UtilityModel, CoreError>> {
		Box::pin(async move {
			let (id, destination) = match binding.provider.0.as_str() {
				"anthropic" => ("claude-code", "api.anthropic.com"),
				"openai" => ("codex", "api.openai.com"),
				_ => return Err(unavailable()),
			};
			let pin = crate::run::craft::load(&self.home, id).await?;
			let contract = crate::run::craft::Contract::of(&pin)?;
			if !contract
				.specification
				.enabled_features()
				.map_err(|_| unavailable())?
				.iter()
				.any(|f| f == "utility")
				|| !contract.specification.host_access.contains(
					&CraftHostAccess::Executable {
						name: "curl".into(),
					},
				) || !contract.specification.host_access.contains(
				&CraftHostAccess::Network {
					destination: destination.into(),
				},
			) {
				return Err(unavailable());
			}
			let bytes = exchange(&pin, "--utility-model", &[], 1024).await?;
			let selection: CraftUtilityModel =
				serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
			if selection.version != 1 {
				return Err(unavailable());
			}
			Ok(UtilityModel {
				name: selection.model,
				adapter_state: serde_json::to_string(&pin)
					.map_err(|_| unavailable())?,
			})
		})
	}
	fn infer<'a>(
		&'a self,
		binding: &'a AccountBinding,
		model: &'a UtilityModel,
		input: &'a UtilityInput,
	) -> RunFuture<'a, Result<UtilityReply, CoreError>> {
		Box::pin(async move {
			let pin: PinnedCraft = serde_json::from_str(&model.adapter_state)
				.map_err(|_| unavailable())?;
			let request = CraftUtilityRequest {
				version: 1,
				model: model.name.clone(),
				binding_id: binding.binding_id.0,
				credential_reference: crate::translate::utility::reference(
					binding.credential_reference.clone(),
				),
				input: match input {
					UtilityInput::Naming {
						title,
						opening_context,
					} => jet_protocol::UtilityInput::Naming {
						title: title.clone(),
						opening_context: opening_context.clone(),
					},
					UtilityInput::GitText {
						patch,
						instructions,
					} => jet_protocol::UtilityInput::GitText {
						patch: patch.clone(),
						instructions: instructions.clone(),
					},
					UtilityInput::Autodelete { prompt } => {
						jet_protocol::UtilityInput::Autodelete {
							prompt: prompt.clone(),
						}
					}
				},
			};
			let bytes =
				serde_json::to_vec(&request).map_err(|_| unavailable())?;
			if bytes.len() > 131072 {
				return Err(unavailable());
			}
			let bytes = exchange(&pin, "--utility", &bytes, 65536).await?;
			let reply: CraftUtilityReply =
				serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
			if reply.version != 1 {
				return Err(unavailable());
			}
			Ok(UtilityReply {
				model: reply.model,
				output: reply.output.into_bytes(),
			})
		})
	}
}
/// One bounded, one-shot exchange with an accepted Craft entrypoint. The
/// process gets the request on standard input, answers on standard output,
/// and is supervised as its own group so a timeout also ends the credential
/// helpers and transport it started.
pub(crate) async fn exchange(
	pin: &PinnedCraft,
	mode: &str,
	input: &[u8],
	limit: u64,
) -> Result<Vec<u8>, CoreError> {
	pin.verify().await?;
	let mut command = Command::new(&pin.executable);
	command
		.arg(mode)
		.current_dir("/")
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null());
	// Reuse process-group supervision so timeout, cancellation, and normal
	// completion also terminate credential helpers and transport descendants.
	let (_cancel, revoked) = tokio::sync::oneshot::channel::<()>();
	let mut process =
		jet_runtime::NoVisaOperation::spawn(&mut command, async {
			let _ = revoked.await;
		})
		.map_err(|_| unavailable())?;
	let mut stdin = process.take_stdin().ok_or_else(unavailable)?;
	let stdout = process.take_stdout().ok_or_else(unavailable)?;
	let write = async {
		stdin.write_all(input).await.map_err(|_| unavailable())?;
		drop(stdin);
		Ok::<_, CoreError>(())
	};
	let read = async {
		let mut bytes = Vec::new();
		stdout
			.take(limit + 1)
			.read_to_end(&mut bytes)
			.await
			.map_err(|_| unavailable())?;
		if bytes.len() as u64 > limit {
			return Err(unavailable());
		}
		Ok(bytes)
	};
	let (_, bytes) = tokio::try_join!(write, read)?;
	if !process.wait().await.map_err(|_| unavailable())?.success() {
		return Err(unavailable());
	}
	Ok(bytes)
}
fn unavailable() -> CoreError {
	CoreError {
		category: jet_core::ErrorCategory::Unavailable,
		code: "utility.craft_unavailable".into(),
		retryable: false,
		message: "the selected Utility Craft or inference is unavailable"
			.into(),
		detail: None,
		revision_conflict: None,
		recovery_actions: vec![],
	}
}
