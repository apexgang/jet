//! Resolve exactly the selected reference, only at the transport boundary.
use crate::{
	CraftError,
	utility::{UtilityProvider, invalid},
	utility_http::run,
};
use jet_protocol::CredentialReference;
use tokio::process::Command;

/// Resolve the one Credential the host selected for this request. The
/// binding names the item, so no request can reach another account's key.
pub(crate) async fn resolve(
	provider: UtilityProvider,
	reference: &CredentialReference,
	binding_id: &str,
) -> Result<String, CraftError> {
	let bytes = match reference {
		CredentialReference::HarnessNative => std::env::var(match provider {
			UtilityProvider::OpenAi => "OPENAI_API_KEY",
			UtilityProvider::Anthropic => "ANTHROPIC_API_KEY",
		})
		.map_err(|_| invalid())?
		.into_bytes(),
		CredentialReference::ExternalHelper { helper } => {
			let mut command = Command::new(helper);
			command.args(["jet-utility-api-key", provider.name(), binding_id]);
			run(&mut command, &[], 4096).await?
		}
		CredentialReference::PlatformStore { item } => {
			// The binding owns its item; clients cannot choose another account's key.
			if item.service != "me.heeka.jet.credential"
				|| item.account != binding_id
			{
				return Err(invalid());
			}
			#[cfg(target_os = "macos")]
			let mut command = {
				let mut c = Command::new("security");
				c.args([
					"find-generic-password",
					"-s",
					&item.service,
					"-a",
					&item.account,
					"-w",
				]);
				c
			};
			#[cfg(not(target_os = "macos"))]
			let mut command = {
				let mut c = Command::new("secret-tool");
				c.args([
					"lookup",
					"service",
					&item.service,
					"account",
					&item.account,
				]);
				c
			};
			run(&mut command, &[], 4096).await?
		}
		CredentialReference::SessionOnly { .. } => return Err(invalid()),
	};
	let text = std::str::from_utf8(&bytes)
		.map_err(|_| invalid())?
		.trim_end_matches(['\n', '\r']);
	if text.is_empty()
		|| text.len() > 4096
		|| !text.bytes().all(|b| b.is_ascii_graphic())
	{
		return Err(invalid());
	}
	Ok(text.into())
}
