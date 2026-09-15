//! Whether a staged release may take over the Plane (ADR-0026, ADR-0073,
//! ADR-0088).

use super::manifest::ReleaseManifest;
use serde::Serialize;

/// Everything the rule weighs about one proposed activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Activation<'a> {
	/// The staged release that would become `current`.
	pub(crate) candidate: &'a ReleaseManifest,
	/// The release `current` names now, when any.
	pub(crate) current: Option<&'a ReleaseManifest>,
	/// The protocol major of the executable applying the rule, which stands
	/// in for `current` on a Plane that has no version layout yet.
	pub(crate) invoker_major: u32,
	/// The version `previous` names now, when any.
	pub(crate) previous: Option<&'a str>,
	/// The newest migration the Plane store has applied, when it has one.
	pub(crate) store_schema: Option<i64>,
	/// How many `jetfueld` helpers are alive under the Plane.
	pub(crate) live_helpers: usize,
}

/// Why an activation is refused, spelled for the caller's output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub(crate) enum Refusal {
	/// The payload was built for another target than the current release.
	TargetMismatch {
		/// The candidate's target.
		candidate: String,
		/// The current release's target.
		current: String,
	},
	/// Live helpers pin the negotiated protocol major until they exit.
	ProtocolMajorPinned {
		/// The candidate's protocol major.
		candidate: u32,
		/// The protocol major live helpers were started under.
		current: u32,
		/// How many helpers are alive.
		live_helpers: usize,
	},
	/// The candidate is older than the release pair that can open the
	/// store the current release migrated.
	SchemaOutsidePair {
		/// The newest migration the candidate embeds.
		candidate: i64,
		/// The newest migration the store has applied.
		store: i64,
		/// The one older release that may still open the store.
		previous: Option<String>,
	},
}

/// Applies the rule: same target; no protocol-major change while helpers
/// are alive, measured against `current` or, before any version exists,
/// the executable applying the rule; and a candidate that knows every
/// migration the store has applied, or is exactly the previous release of
/// the pair (ADR-0073).
pub(crate) fn check(activation: &Activation<'_>) -> Result<(), Refusal> {
	let Activation {
		candidate,
		current,
		invoker_major,
		previous,
		store_schema,
		live_helpers,
	} = activation;
	if let Some(current) = current
		&& candidate.target != current.target
	{
		return Err(Refusal::TargetMismatch {
			candidate: candidate.target.clone(),
			current: current.target.clone(),
		});
	}
	let pinned_major =
		current.map_or(*invoker_major, |current| current.protocol_major);
	if candidate.protocol_major != pinned_major && *live_helpers > 0 {
		return Err(Refusal::ProtocolMajorPinned {
			candidate: candidate.protocol_major,
			current: pinned_major,
			live_helpers: *live_helpers,
		});
	}
	if let Some(store) = *store_schema
		&& candidate.schema_version < store
		&& *previous != Some(candidate.version.as_str())
	{
		return Err(Refusal::SchemaOutsidePair {
			candidate: candidate.schema_version,
			store,
			previous: previous.map(str::to_owned),
		});
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use pretty_assertions::assert_eq;

	fn release(
		version: &str,
		protocol_major: u32,
		schema: i64,
	) -> ReleaseManifest {
		ReleaseManifest {
			version: version.into(),
			target: "aarch64-apple-darwin".into(),
			protocol_major,
			protocol_minor: 42,
			schema_version: schema,
			executables: Default::default(),
		}
	}

	#[test]
	fn the_rule_admits_upgrades_and_the_previous_release_only() {
		let current = release("0.3.0", 1, 30);
		let newer = release("0.4.0", 1, 40);
		let previous = release("0.2.0", 1, 20);
		let older = release("0.1.0", 1, 10);
		let major = release("0.4.0", 2, 40);
		let mut foreign = release("0.4.0", 1, 40);
		foreign.target = "x86_64-apple-darwin".into();
		let activation = |candidate, live_helpers| Activation {
			candidate,
			current: Some(&current),
			invoker_major: 1,
			previous: Some("0.2.0"),
			store_schema: Some(30),
			live_helpers,
		};

		let outcomes = [
			check(&activation(&newer, 3)),
			check(&activation(&previous, 3)),
			check(&activation(&older, 0)),
			check(&activation(&major, 1)),
			check(&activation(&major, 0)),
			check(&activation(&foreign, 0)),
			check(&Activation {
				candidate: &older,
				current: None,
				invoker_major: 1,
				previous: None,
				store_schema: None,
				live_helpers: 0,
			}),
			check(&Activation {
				candidate: &major,
				current: None,
				invoker_major: 1,
				previous: None,
				store_schema: None,
				live_helpers: 2,
			}),
		];

		assert_eq!(
			outcomes,
			[
				Ok(()),
				Ok(()),
				Err(Refusal::SchemaOutsidePair {
					candidate: 10,
					store: 30,
					previous: Some("0.2.0".into()),
				}),
				Err(Refusal::ProtocolMajorPinned {
					candidate: 2,
					current: 1,
					live_helpers: 1,
				}),
				Ok(()),
				Err(Refusal::TargetMismatch {
					candidate: "x86_64-apple-darwin".into(),
					current: "aarch64-apple-darwin".into(),
				}),
				Ok(()),
				Err(Refusal::ProtocolMajorPinned {
					candidate: 2,
					current: 1,
					live_helpers: 2,
				}),
			]
		);
	}
}
