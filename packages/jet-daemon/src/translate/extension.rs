//! Explicit domain/wire translations keep native metadata unchanged (ADR-0049).
use jet_core as core;
use jet_protocol as wire;

pub(crate) fn catalog(value: core::ExtensionCatalog) -> wire::ExtensionCatalog {
	wire::ExtensionCatalog {
		craft_id: value.craft_id,
		harness: value.harness,
		native_metadata: value.native_metadata,
	}
}
pub(crate) fn catalog_from_wire(
	value: wire::ExtensionCatalog,
) -> core::ExtensionCatalog {
	core::ExtensionCatalog {
		craft_id: value.craft_id,
		harness: value.harness,
		native_metadata: value.native_metadata,
	}
}
fn action(value: core::ExtensionAction) -> wire::ExtensionAction {
	match value {
		core::ExtensionAction::Install => wire::ExtensionAction::Install,
		core::ExtensionAction::Update => wire::ExtensionAction::Update,
		core::ExtensionAction::Disable => wire::ExtensionAction::Disable,
		core::ExtensionAction::Remove => wire::ExtensionAction::Remove,
	}
}
fn action_from_wire(value: wire::ExtensionAction) -> core::ExtensionAction {
	match value {
		wire::ExtensionAction::Install => core::ExtensionAction::Install,
		wire::ExtensionAction::Update => core::ExtensionAction::Update,
		wire::ExtensionAction::Disable => core::ExtensionAction::Disable,
		wire::ExtensionAction::Remove => core::ExtensionAction::Remove,
	}
}
pub(crate) fn confirmation(
	value: core::ExtensionConfirmation,
) -> wire::ExtensionConfirmation {
	wire::ExtensionConfirmation {
		catalog: catalog(value.catalog),
		extension_id: value.extension_id,
		action: action(value.action),
		scope: match value.scope {
			core::ExtensionScope::User => wire::ExtensionScope::User,
		},
		trust: match value.trust {
			core::ExtensionTrust::SameUserExecutable => {
				wire::ExtensionTrust::SameUserExecutable
			}
		},
	}
}
pub(crate) fn confirmation_from_wire(
	value: wire::ExtensionConfirmation,
) -> core::ExtensionConfirmation {
	core::ExtensionConfirmation {
		catalog: catalog_from_wire(value.catalog),
		extension_id: value.extension_id,
		action: action_from_wire(value.action),
		scope: match value.scope {
			wire::ExtensionScope::User => core::ExtensionScope::User,
		},
		trust: match value.trust {
			wire::ExtensionTrust::SameUserExecutable => {
				core::ExtensionTrust::SameUserExecutable
			}
		},
	}
}
pub(crate) fn change(value: core::ExtensionChange) -> wire::ExtensionChange {
	wire::ExtensionChange {
		change_id: value.change_id,
		craft_id: value.craft_id,
		extension_id: value.extension_id,
		action: action(value.action),
		state: match value.state {
			core::ExtensionChangeState::Staged => {
				wire::ExtensionChangeState::Staged
			}
			core::ExtensionChangeState::Applied => {
				wire::ExtensionChangeState::Applied
			}
			core::ExtensionChangeState::Refused => {
				wire::ExtensionChangeState::Refused
			}
			core::ExtensionChangeState::OutcomeUnknown => {
				wire::ExtensionChangeState::OutcomeUnknown
			}
		},
	}
}
