//! Explicit Utility translation keeps model data separate from Commands.
use jet_core as core;
use jet_protocol as wire;
pub(super) fn request(request: wire::UtilityRequest) -> core::UtilityRequest {
	match request {
		wire::UtilityRequest::Naming { run_id } => {
			core::UtilityRequest::Naming {
				run_id: core::RunId(run_id),
			}
		}
		wire::UtilityRequest::GitText { run_id, turn } => {
			core::UtilityRequest::GitText {
				run_id: core::RunId(run_id),
				turn,
			}
		}
		wire::UtilityRequest::Autodelete { prompt } => {
			core::UtilityRequest::Autodelete { prompt }
		}
	}
}
pub(super) fn job(job: core::UtilityJob) -> wire::UtilityJob {
	wire::UtilityJob {
		job_id: job.job_id,
		plane_id: job.plane_id.0,
		purpose: match job.purpose {
			core::UtilityPurpose::Naming => wire::UtilityPurpose::Naming,
			core::UtilityPurpose::GitText => wire::UtilityPurpose::GitText,
			core::UtilityPurpose::Autodelete => {
				wire::UtilityPurpose::Autodelete
			}
		},
		provider: job.provider.map(|p| p.0),
		binding_id: job.binding_id.map(|b| b.0),
		model: job.model,
		policy: wire::UtilityPolicy {
			version: job.policy.version,
			enabled: job.policy.enabled,
			cross_provider_consent: job.policy.cross_provider_consent,
		},
		outcome: match job.outcome {
			core::UtilityOutcome::Pending => wire::UtilityOutcome::Pending,
			core::UtilityOutcome::Draft { inactive_days } => {
				wire::UtilityOutcome::Draft { inactive_days }
			}
			core::UtilityOutcome::Text {
				text,
				body,
				fallback_reason,
			} => wire::UtilityOutcome::Text {
				text,
				body,
				fallback_reason,
			},
			core::UtilityOutcome::Refused { reason } => {
				wire::UtilityOutcome::Refused { reason }
			}
		},
	}
}

pub(crate) fn reference(
	reference: core::CredentialReference,
) -> wire::CredentialReference {
	super::account::reference(reference)
}
