//! Accepted-Craft extension calls run outside active Run connections.
use crate::translate::extension;
use jet_core::{
	CoreError, ExtensionCatalog, ExtensionConfirmation, ExtensionHost,
	PinnedCraft, RunFuture,
};
use jet_protocol::{CraftExtensionReply, CraftExtensionRequest};
use std::{path::PathBuf, process::Stdio};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	process::Command,
};
#[derive(Debug)]
pub(crate) struct Extensions {
	pub(crate) home: PathBuf,
}
impl ExtensionHost for Extensions {
	fn pin<'a>(
		&'a self,
		id: &'a str,
	) -> RunFuture<'a, Result<PinnedCraft, CoreError>> {
		Box::pin(async move { crate::run_craft::load(&self.home, id).await })
	}

	fn catalog<'a>(
		&'a self,
		pin: &'a PinnedCraft,
	) -> RunFuture<'a, Result<ExtensionCatalog, CoreError>> {
		Box::pin(async move {
			match self.exchange(pin, CraftExtensionRequest::Catalog).await? {
				CraftExtensionReply::Catalog { catalog } => {
					Ok(extension::catalog_from_wire(catalog))
				}
				CraftExtensionReply::Applied | CraftExtensionReply::Refused => {
					Err(unavailable())
				}
			}
		})
	}
	fn inspect<'a>(
		&'a self,
		craft: &'a PinnedCraft,
		id: &'a str,
	) -> RunFuture<'a, Result<ExtensionCatalog, CoreError>> {
		Box::pin(async move {
			match self
				.exchange(
					craft,
					CraftExtensionRequest::Inspect {
						extension_id: id.into(),
					},
				)
				.await?
			{
				CraftExtensionReply::Catalog { catalog } => {
					Ok(extension::catalog_from_wire(catalog))
				}
				CraftExtensionReply::Applied | CraftExtensionReply::Refused => {
					Err(unavailable())
				}
			}
		})
	}
	fn apply<'a>(
		&'a self,
		pin: &'a PinnedCraft,
		confirmation: &'a ExtensionConfirmation,
	) -> RunFuture<'a, Result<(), CoreError>> {
		Box::pin(async move {
			match self
				.exchange(
					pin,
					CraftExtensionRequest::Apply {
						confirmation: extension::confirmation(
							confirmation.clone(),
						),
					},
				)
				.await?
			{
				CraftExtensionReply::Applied => Ok(()),
				CraftExtensionReply::Refused => {
					Err(failure("extension.refused"))
				}
				CraftExtensionReply::Catalog { .. } => Err(unavailable()),
			}
		})
	}
}
impl Extensions {
	async fn exchange(
		&self,
		pin: &PinnedCraft,
		request: CraftExtensionRequest,
	) -> Result<CraftExtensionReply, CoreError> {
		let contract = crate::run_craft::Contract::of(pin)?;
		// ASVS 8.3.1: feature support is not broker authority; this endpoint grants none.
		if !contract
			.specification
			.enabled_features()
			.map_err(|_| unavailable())?
			.iter()
			.any(|f| f == "extensions")
		{
			return Err(unavailable());
		}
		pin.verify().await?;
		let input = jet_protocol::encode_control(&request)
			.map_err(|_| unavailable())?;
		if input.len() > 131072 {
			return Err(unavailable());
		}
		let mut command = Command::new(&pin.executable);
		command
			.arg("--extensions-v1")
			.current_dir("/")
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::null());
		// Group ownership also reaps native descendants on cancellation or timeout.
		let (_cancel, revoked) = tokio::sync::oneshot::channel::<()>();
		let mut process =
			jet_runtime::NoVisaOperation::spawn(&mut command, async {
				let _ = revoked.await;
			})
			.map_err(|_| unavailable())?;
		let mut stdin = process.take_stdin().ok_or_else(unavailable)?;
		let stdout = process.take_stdout().ok_or_else(unavailable)?;
		let write = async {
			stdin.write_all(&input).await.map_err(|_| unavailable())?;
			drop(stdin);
			Ok::<_, CoreError>(())
		};
		let read = async {
			let mut bytes = Vec::new();
			stdout
				.take(131073)
				.read_to_end(&mut bytes)
				.await
				.map_err(|_| unavailable())?;
			if bytes.len() > 131072 {
				return Err(unavailable());
			}
			Ok(bytes)
		};
		let (_, bytes) = tokio::try_join!(write, read)?;
		if !process.wait().await.map_err(|_| unavailable())?.success() {
			return Err(unavailable());
		}
		jet_protocol::decode_control(&bytes).map_err(|_| unavailable())
	}
}
fn unavailable() -> CoreError {
	failure("extension.craft_unavailable")
}

fn failure(code: &str) -> CoreError {
	CoreError {
		category: jet_core::ErrorCategory::Unavailable,
		code: code.into(),
		retryable: false,
		message: "the native extension operation is unavailable".into(),
		detail: None,
		revision_conflict: None,
		recovery_actions: vec![],
	}
}
