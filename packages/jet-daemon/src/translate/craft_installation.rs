//! Third-party Craft installation translation seam (ADR-0049, ADR-0013).

use jet_core::{
	BrokerPermission, CraftHostAccess, CraftInstallationConfirmation,
	CraftInstallationPreview, CraftSource, CraftTrust,
};
use jet_protocol as wire;
use std::path::PathBuf;

pub(super) fn source_from_wire(source: &wire::CraftSource) -> CraftSource {
	match source {
		wire::CraftSource::GitHubRelease { repository, tag } => {
			CraftSource::GitHubRelease {
				repository: repository.clone(),
				tag: tag.clone(),
			}
		}
		wire::CraftSource::Local {
			specification,
			artifact,
		} => CraftSource::Local {
			specification: PathBuf::from(specification),
			artifact: PathBuf::from(artifact),
		},
	}
}

pub(super) fn preview(
	value: CraftInstallationPreview,
) -> wire::CraftInstallationPreview {
	let confirmation = confirmation(value.confirmation());
	wire::CraftInstallationPreview {
		craft_id: value.craft_id,
		version: value.version,
		enabled_features: value.enabled_features,
		confirmation,
	}
}

pub(super) fn confirmation_from_wire(
	value: &wire::CraftInstallationConfirmation,
) -> CraftInstallationConfirmation {
	CraftInstallationConfirmation {
		source: source_from_wire(&value.source),
		repository: value.repository.clone(),
		publisher_claim: value.publisher_claim.clone(),
		commit: value.commit.clone(),
		artifact_sha256: value.artifact_sha256.clone(),
		broker_permissions: value
			.broker_permissions
			.iter()
			.copied()
			.map(permission_from_wire)
			.collect(),
		host_access: value
			.host_access
			.iter()
			.cloned()
			.map(access_from_wire)
			.collect(),
		trust: match value.trust {
			wire::CraftTrust::SameUserExecutable => {
				CraftTrust::SameUserExecutable
			}
			wire::CraftTrust::DeveloperSource => CraftTrust::DeveloperSource,
		},
	}
}

fn confirmation(
	value: CraftInstallationConfirmation,
) -> wire::CraftInstallationConfirmation {
	wire::CraftInstallationConfirmation {
		source: source(value.source),
		repository: value.repository,
		publisher_claim: value.publisher_claim,
		commit: value.commit,
		artifact_sha256: value.artifact_sha256,
		broker_permissions: value
			.broker_permissions
			.into_iter()
			.map(permission)
			.collect(),
		host_access: value.host_access.into_iter().map(access).collect(),
		trust: match value.trust {
			CraftTrust::SameUserExecutable => {
				wire::CraftTrust::SameUserExecutable
			}
			CraftTrust::DeveloperSource => wire::CraftTrust::DeveloperSource,
		},
	}
}

fn source(source: CraftSource) -> wire::CraftSource {
	match source {
		CraftSource::GitHubRelease { repository, tag } => {
			wire::CraftSource::GitHubRelease { repository, tag }
		}
		CraftSource::Local {
			specification,
			artifact,
		} => wire::CraftSource::Local {
			specification: specification.to_string_lossy().into_owned(),
			artifact: artifact.to_string_lossy().into_owned(),
		},
	}
}

fn permission(permission: BrokerPermission) -> wire::BrokerPermission {
	match permission {
		BrokerPermission::ArtifactRead => wire::BrokerPermission::ArtifactRead,
		BrokerPermission::ArtifactWrite => {
			wire::BrokerPermission::ArtifactWrite
		}
		BrokerPermission::RemoteTools => wire::BrokerPermission::RemoteTools,
	}
}

fn permission_from_wire(
	permission: wire::BrokerPermission,
) -> BrokerPermission {
	match permission {
		wire::BrokerPermission::ArtifactRead => BrokerPermission::ArtifactRead,
		wire::BrokerPermission::ArtifactWrite => {
			BrokerPermission::ArtifactWrite
		}
		wire::BrokerPermission::RemoteTools => BrokerPermission::RemoteTools,
	}
}

fn access(access: CraftHostAccess) -> wire::CraftHostAccess {
	match access {
		CraftHostAccess::Executable { name } => {
			wire::CraftHostAccess::Executable { name }
		}
		CraftHostAccess::Filesystem { path } => {
			wire::CraftHostAccess::Filesystem { path }
		}
		CraftHostAccess::Environment { name } => {
			wire::CraftHostAccess::Environment { name }
		}
		CraftHostAccess::Network { destination } => {
			wire::CraftHostAccess::Network { destination }
		}
	}
}

fn access_from_wire(access: wire::CraftHostAccess) -> CraftHostAccess {
	match access {
		wire::CraftHostAccess::Executable { name } => {
			CraftHostAccess::Executable { name }
		}
		wire::CraftHostAccess::Filesystem { path } => {
			CraftHostAccess::Filesystem { path }
		}
		wire::CraftHostAccess::Environment { name } => {
			CraftHostAccess::Environment { name }
		}
		wire::CraftHostAccess::Network { destination } => {
			CraftHostAccess::Network { destination }
		}
	}
}
