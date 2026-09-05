//! The Account binding half of the translation seam (ADR-0049, ADR-0016).

use jet_core::{
	AccountBinding, AccountBindingList, AccountBindingStatus, CredentialItem,
	CredentialReference, CredentialSource, CredentialState, ProviderAccount,
};
use jet_protocol as wire;

use super::unix_ms;

pub(super) fn list(list: AccountBindingList) -> wire::AccountBindingList {
	wire::AccountBindingList {
		cursor: list.cursor.0,
		bindings: list.bindings.into_iter().map(status).collect(),
	}
}

fn status(status: AccountBindingStatus) -> wire::AccountBindingStatus {
	wire::AccountBindingStatus {
		binding: binding(status.binding),
		credential_state: credential_state(status.credential_state),
	}
}

fn credential_state(state: CredentialState) -> wire::CredentialState {
	match state {
		CredentialState::Resolvable => wire::CredentialState::Resolvable,
		CredentialState::ResolvedAtUse => wire::CredentialState::ResolvedAtUse,
		CredentialState::WaitingForUnlock { kind } => {
			wire::CredentialState::WaitingForUnlock {
				kind: super::capability::credential_store_kind(kind),
			}
		}
		CredentialState::Unavailable { kind } => {
			wire::CredentialState::Unavailable {
				kind: super::capability::credential_store_kind(kind),
			}
		}
		CredentialState::InvalidatedByRestart => {
			wire::CredentialState::InvalidatedByRestart
		}
	}
}

pub(super) fn binding(binding: AccountBinding) -> wire::AccountBinding {
	wire::AccountBinding {
		binding_id: binding.binding_id.0,
		provider: binding.provider.0,
		label: binding.label,
		provider_account: binding
			.provider_account
			.map(|ProviderAccount(identity)| identity),
		credential_reference: reference(binding.credential_reference),
		created_at_unix_ms: unix_ms(binding.created_at),
	}
}

pub(super) fn provider_account(
	provider_account: Option<&String>,
) -> Option<ProviderAccount> {
	provider_account.map(|identity| ProviderAccount(identity.clone()))
}

pub(super) fn reference(
	reference: CredentialReference,
) -> wire::CredentialReference {
	match reference {
		CredentialReference::PlatformStore {
			item: CredentialItem { service, account },
		} => wire::CredentialReference::PlatformStore {
			item: wire::CredentialItem { service, account },
		},
		CredentialReference::ExternalHelper { helper } => {
			wire::CredentialReference::ExternalHelper { helper }
		}
		CredentialReference::HarnessNative => {
			wire::CredentialReference::HarnessNative
		}
		CredentialReference::SessionOnly {
			established_at_daemon_start,
		} => wire::CredentialReference::SessionOnly {
			established_at_daemon_start,
		},
	}
}

pub(super) fn source_from_wire(
	source: &wire::CredentialSource,
) -> CredentialSource {
	match source {
		wire::CredentialSource::PlatformStore => {
			CredentialSource::PlatformStore
		}
		wire::CredentialSource::ExternalHelper { helper } => {
			CredentialSource::ExternalHelper {
				helper: helper.clone(),
			}
		}
		wire::CredentialSource::HarnessNative => {
			CredentialSource::HarnessNative
		}
		wire::CredentialSource::SessionOnly => CredentialSource::SessionOnly,
	}
}
