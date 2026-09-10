//! Native extension discovery and deferred lifecycle progress.
use crate::{Client, ClientError, requests::unexpected};
use jet_protocol::{
	CommandRequest, CommandResponse, ExtensionCatalog, ExtensionChange,
	ExtensionConfirmation, QueryRequest, QueryResponse,
};
use uuid::Uuid;

impl Client {
	/// Read native source, component and permission metadata through its Craft.
	/// # Errors
	/// Returns compatibility, native discovery or transport errors.
	pub async fn extension_catalog(
		&self,
		craft_id: String,
	) -> Result<ExtensionCatalog, ClientError> {
		self.require_minor(jet_protocol::EXTENSIONS_MINOR)?;
		let reply = self
			.query(QueryRequest::ExtensionCatalog { craft_id })
			.await?;
		if let QueryResponse::ExtensionCatalog(catalog) = reply {
			Ok(catalog)
		} else {
			Err(unexpected(&reply))
		}
	}
	/// Inspect the selected native package's metadata before granting access.
	/// # Errors
	/// Returns compatibility, inspection or transport errors.
	pub async fn inspect_extension(
		&self,
		craft_id: String,
		extension_id: String,
	) -> Result<ExtensionCatalog, ClientError> {
		self.require_minor(jet_protocol::EXTENSIONS_MINOR)?;
		let reply = self
			.query(QueryRequest::InspectExtension {
				craft_id,
				extension_id,
			})
			.await?;
		if let QueryResponse::ExtensionCatalog(catalog) = reply {
			Ok(catalog)
		} else {
			Err(unexpected(&reply))
		}
	}
	/// Stage a native mutation after reviewing the exact catalog and same-user access.
	/// # Errors
	/// Returns compatibility, stale consent, policy or transport errors.
	pub async fn change_extension(
		&self,
		command_id: Uuid,
		confirmation: ExtensionConfirmation,
	) -> Result<Uuid, ClientError> {
		self.require_minor(jet_protocol::EXTENSIONS_MINOR)?;
		let reply = self
			.execute_command(
				command_id,
				CommandRequest::ChangeExtension { confirmation },
			)
			.await?;
		if let CommandResponse::ExtensionChangeQueued { change_id } = reply {
			Ok(change_id)
		} else {
			Err(unexpected(&reply))
		}
	}
	/// Read whether a native mutation is staged, applied, refused or uncertain.
	/// # Errors
	/// Returns compatibility, missing change or transport errors.
	pub async fn extension_change(
		&self,
		change_id: Uuid,
	) -> Result<ExtensionChange, ClientError> {
		self.require_minor(jet_protocol::EXTENSIONS_MINOR)?;
		let reply = self
			.query(QueryRequest::ExtensionChange { change_id })
			.await?;
		if let QueryResponse::ExtensionChange(change) = reply {
			Ok(change)
		} else {
			Err(unexpected(&reply))
		}
	}
}
