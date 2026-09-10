//! Capability snapshots: what one Plane can do, reported explicitly rather
//! than polled (ADR-0086).
//!
//! A snapshot covers the operating system, the running core, the external
//! command-line tools the core invokes (ADR-0056), whether credentials can
//! be resolved at all (ADR-0076), the installed Crafts and the Harnesses
//! they adapt, and the conditions under which the Plane is degraded.
//!
//! Nothing here is authoritative Plane state: it describes the machine, so
//! it is observed, never stored.

pub(crate) mod probe;

use crate::{CORE_VERSION, Core, command::Command, error::CoreError};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, time::SystemTime};

/// Longest tool version line the core keeps, so a talkative tool cannot
/// grow a snapshot without bound (ADR-0061).
pub(crate) const MAX_VERSION_CHARS: usize = 120;

/// Observes what one Plane can do right now.
///
/// `jetd` observes once at startup, again when a client asks for a fresh
/// snapshot, and again before a Command commits work that depends on a
/// Capability. An implementation answers from the machine it runs on, must
/// not poll on its own, and must not change Plane state.
///
/// The returned future is boxed because the core chooses its probe at run
/// time: a Plane observes its own machine, while a test answers with a
/// fixed observation.
pub trait CapabilityProbe: std::fmt::Debug + Send + Sync {
	/// Reads power alone for admission and active-work maintenance. Implementations
	/// should avoid unrelated tool discovery on this frequent observation path.
	fn power(
		&self,
	) -> Pin<Box<dyn Future<Output = jet_runtime::PowerState> + Send + '_>> {
		Box::pin(async { self.observe().await.power })
	}

	/// Observes the Plane once.
	fn observe(
		&self,
	) -> Pin<Box<dyn Future<Output = ObservedCapabilities> + Send + '_>>;
}

/// What a [`CapabilityProbe`] saw. The core adds its own version, the time
/// of the observation, and the degraded conditions that follow from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedCapabilities {
	/// Most recent operating-system power observation.
	pub power: jet_runtime::PowerState,
	/// The operating system the Plane runs.
	pub platform: Platform,
	/// Every external tool the core knows how to invoke.
	pub external_tools: Vec<ExternalToolStatus>,
	/// Whether credentials can be resolved without storing secrets.
	pub credential_store: CredentialStoreStatus,
	/// The Crafts installed on the Plane.
	pub crafts: Vec<InstalledCraft>,
}

/// A point-in-time report of what a Plane can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitySnapshot {
	/// Most recent operating-system power observation.
	pub power: jet_runtime::PowerState,
	/// When the Plane was observed.
	pub observed_at: SystemTime,
	/// Version of the core that observed it.
	pub core_version: &'static str,
	/// The operating system the Plane runs.
	pub platform: Platform,
	/// Every external tool the core knows how to invoke.
	pub external_tools: Vec<ExternalToolStatus>,
	/// Whether credentials can be resolved without storing secrets.
	pub credential_store: CredentialStoreStatus,
	/// The Crafts installed on the Plane.
	pub crafts: Vec<InstalledCraft>,
	/// Every Harness an installed Craft adapts.
	pub harnesses: Vec<HarnessId>,
	/// What the Plane cannot do in this state.
	pub degraded: Vec<DegradedCondition>,
}

/// The operating system and processor a Plane runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
	/// Operating system name, such as `linux` or `macos`.
	pub operating_system: &'static str,
	/// Processor architecture, such as `aarch64`.
	pub architecture: &'static str,
}

/// A command-line tool the core invokes but never bundles or installs
/// (ADR-0056).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalTool {
	/// Git, which every Workspace and Change checkpoint rests on.
	Git,
	/// Git LFS, which a Project's large files pass through only when it is
	/// installed. Jet reports it and never bundles it (ADR-0103).
	GitLfs,
	/// OpenSSH, which No-Visa Runs reach paired Planes through.
	Ssh,
	/// Tailscale, which discovers and reaches Planes across networks.
	Tailscale,
}

/// Whether ordinary local work needs one external tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolNeed {
	/// Every Plane needs it, so its absence degrades this one.
	Always,
	/// Only some features need it, so its absence is reported plainly.
	SomeFeatures,
}

/// Whether one external tool answered, and with which version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolStatus {
	/// The tool that was looked for.
	pub tool: ExternalTool,
	/// What looking for it found.
	pub availability: ToolAvailability,
}

/// The result of looking for one external tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolAvailability {
	/// The tool answered with the version line it reports.
	Present {
		/// The tool's own version line, bounded and unparsed.
		version: String,
	},
	/// The tool could not be run at all.
	Missing,
}

/// The platform facility that resolves Credential references (ADR-0076).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStoreKind {
	/// The macOS Keychain.
	AppleKeychain,
	/// The freedesktop Secret Service, reached over the session bus.
	SecretService,
}

/// Whether the platform credential store can be reached, and whether it
/// will answer. Jet never falls back to plaintext, so each of these is
/// reported rather than worked around (ADR-0076).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStoreStatus {
	/// The store is present and can be asked for credentials.
	Available {
		/// Which store the Plane resolves through.
		kind: CredentialStoreKind,
	},
	/// The store is present but locked: it answers nothing until the user
	/// unlocks it through the operating system. `jetd` never asks for that
	/// itself, so work that needs a Credential waits instead.
	Locked {
		/// Which store is locked.
		kind: CredentialStoreKind,
	},
	/// The store cannot be reached on this Plane right now.
	Unavailable {
		/// Which store was expected.
		kind: CredentialStoreKind,
	},
}

/// Durable identity of one installed Jet Craft.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CraftId(pub String);

/// Identity of one Harness a Craft adapts, such as `codex`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HarnessId(pub String);

/// One Craft installed on the Plane and the Harnesses it adapts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledCraft {
	/// Whether the accepted Craft declares admission-only native child controls.
	pub limits_subagents: bool,
	/// The Craft's identity.
	pub craft: CraftId,
	/// The version its specification declares.
	pub version: String,
	/// The Harnesses it teaches this Plane to orchestrate.
	pub harnesses: Vec<HarnessId>,
}

/// Something a Plane cannot do in its current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DegradedCondition {
	/// An external tool ordinary work needs is not installed.
	MissingExternalTool {
		/// The tool the core could not run.
		tool: ExternalTool,
	},
	/// No Craft is installed, so the Plane can orchestrate no Harness.
	NoHarnessAvailable,
	/// Credentials cannot be resolved, so no Account binding can be used.
	CredentialStoreUnavailable {
		/// The store that was expected.
		kind: CredentialStoreKind,
	},
	/// The credential store is locked, so Credentials resolve only after
	/// the user unlocks it through the operating system.
	CredentialStoreLocked {
		/// The store that is locked.
		kind: CredentialStoreKind,
	},
}

/// Which Capability snapshot a Query answers with. `jetd` never polls, so
/// a Plane is observed at startup and whenever a caller asks for a new look
/// (ADR-0086).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityObservation {
	/// The snapshot the Plane observed most recently.
	LastObserved,
	/// A new observation, taken now and kept as the Plane's latest.
	Fresh,
}

/// One thing a Command may depend on the Plane being able to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capability {
	/// One of the external command-line tools the core invokes.
	ExternalTool(ExternalTool),
	/// A platform credential store that can hold a Credential durably.
	CredentialStore,
}

impl ExternalTool {
	/// Every tool the core looks for, in the order a snapshot reports them.
	pub(crate) const ALL: [Self; 4] =
		[Self::Git, Self::GitLfs, Self::Ssh, Self::Tailscale];

	/// The stable name of the tool, also its program name. Git LFS is
	/// looked for as the `git-lfs` program that `git lfs` dispatches to,
	/// because `git` answers `git lfs` with an error line of its own when
	/// the program is missing, and an error line is not a version.
	#[must_use]
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Git => "git",
			Self::GitLfs => "git-lfs",
			Self::Ssh => "ssh",
			Self::Tailscale => "tailscale",
		}
	}

	/// Whether ordinary local work needs the tool.
	pub(crate) fn need(self) -> ToolNeed {
		match self {
			// Workspaces are worktrees and checkpoints are commits.
			Self::Git => ToolNeed::Always,
			// Only a Project that tracks large files through LFS needs it,
			// and both SSH and Tailscale serve Planes the user has paired,
			// not this one alone.
			Self::GitLfs | Self::Ssh | Self::Tailscale => {
				ToolNeed::SomeFeatures
			}
		}
	}
}

impl Capability {
	/// How the Capability reads in an error a client sees.
	pub(crate) fn describe(self) -> String {
		match self {
			Self::ExternalTool(tool) => {
				format!("the {} command-line tool", tool.as_str())
			}
			Self::CredentialStore => "the platform credential store".into(),
		}
	}
}

impl CapabilitySnapshot {
	/// Completes one observation into the snapshot the Plane reports,
	/// deriving the Harnesses its Crafts adapt and the conditions that
	/// leave it degraded.
	pub(crate) fn from_observation(
		observed: ObservedCapabilities,
		observed_at: SystemTime,
	) -> Self {
		let ObservedCapabilities {
			power,
			platform,
			external_tools,
			credential_store,
			crafts,
		} = observed;
		let harnesses: Vec<HarnessId> = crafts
			.iter()
			.flat_map(|craft| craft.harnesses.iter().cloned())
			.collect();
		let mut degraded: Vec<DegradedCondition> = external_tools
			.iter()
			.filter(|status| {
				status.availability == ToolAvailability::Missing
					&& status.tool.need() == ToolNeed::Always
			})
			.map(|status| DegradedCondition::MissingExternalTool {
				tool: status.tool,
			})
			.collect();
		if harnesses.is_empty() {
			degraded.push(DegradedCondition::NoHarnessAvailable);
		}
		match credential_store {
			CredentialStoreStatus::Available { .. } => {}
			CredentialStoreStatus::Locked { kind } => {
				degraded
					.push(DegradedCondition::CredentialStoreLocked { kind });
			}
			CredentialStoreStatus::Unavailable { kind } => {
				degraded.push(DegradedCondition::CredentialStoreUnavailable {
					kind,
				});
			}
		}
		Self {
			power,
			observed_at,
			core_version: CORE_VERSION,
			platform,
			external_tools,
			credential_store,
			crafts,
			harnesses,
			degraded,
		}
	}

	/// What the Plane found when it looked for `tool`. A snapshot names
	/// every tool the core looks for, so one that does not name it was
	/// taken by a core that did not look, and that is a Plane without it.
	pub(crate) fn availability(&self, tool: ExternalTool) -> ToolAvailability {
		self.external_tools
			.iter()
			.find(|status| status.tool == tool)
			.map_or(ToolAvailability::Missing, |status| {
				status.availability.clone()
			})
	}

	/// Whether the Plane could do `capability` when it was observed.
	///
	/// A locked credential store still counts as one: locking hides the
	/// secrets it holds, not the store itself, so a binding may be recorded
	/// against it and waits for the unlock the user performs (ADR-0076).
	pub(crate) fn supports(&self, capability: Capability) -> bool {
		match capability {
			Capability::ExternalTool(tool) => {
				self.external_tools.iter().any(|status| {
					status.tool == tool
						&& status.availability != ToolAvailability::Missing
				})
			}
			Capability::CredentialStore => match self.credential_store {
				CredentialStoreStatus::Available { .. }
				| CredentialStoreStatus::Locked { .. } => true,
				CredentialStoreStatus::Unavailable { .. } => false,
			},
		}
	}
}

impl Core {
	/// Observes the Plane again for every Capability `command` depends on,
	/// before it commits anything (ADR-0086).
	pub(crate) async fn revalidate_capabilities(
		&self,
		command: &Command,
	) -> Result<(), CoreError> {
		let required = command.required_capabilities();
		if required.is_empty() {
			return Ok(());
		}
		let capabilities = self.observe_capabilities().await;
		for &capability in required {
			if !capabilities.supports(capability) {
				return Err(CoreError::capability_unavailable(capability));
			}
		}
		Ok(())
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use std::sync::Arc;
	use std::time::{Duration, SystemTime, UNIX_EPOCH};

	use pretty_assertions::assert_eq;
	use tempfile::TempDir;

	use crate::capability::{
		CredentialStoreKind, DegradedCondition, ExternalTool, HarnessId,
	};
	use crate::test_support::{
		FixedProbe, ManualClock, actor, equipped, register_repository, request,
		start_core_with, stripped,
	};
	use crate::{
		CORE_VERSION, CapabilityObservation, CapabilitySnapshot, Command, Core,
		CoreError, ErrorCategory, Query, QueryResult, ResolvedSetting,
		SettingKey, SettingScope, SettingSelection, SettingSource,
		SettingValue,
	};

	/// The one instant every observation in these tests is taken at.
	fn observed_at() -> SystemTime {
		UNIX_EPOCH + Duration::from_secs(1_700_000_000)
	}

	/// What the core reports for a Plane that has everything.
	fn equipped_snapshot() -> CapabilitySnapshot {
		let observed = equipped();
		CapabilitySnapshot {
			power: jet_runtime::PowerState::Normal,
			observed_at: observed_at(),
			core_version: CORE_VERSION,
			platform: observed.platform,
			external_tools: observed.external_tools,
			credential_store: observed.credential_store,
			crafts: observed.crafts,
			harnesses: vec![HarnessId("codex".into())],
			degraded: vec![],
		}
	}

	/// What it reports once Git, the Craft, and the session bus are gone.
	/// Tailscale is missing too, but only some features need it.
	fn stripped_snapshot() -> CapabilitySnapshot {
		let observed = stripped();
		CapabilitySnapshot {
			external_tools: observed.external_tools,
			credential_store: observed.credential_store,
			crafts: observed.crafts,
			harnesses: vec![],
			degraded: vec![
				DegradedCondition::MissingExternalTool {
					tool: ExternalTool::Git,
				},
				DegradedCondition::NoHarnessAvailable,
				DegradedCondition::CredentialStoreUnavailable {
					kind: CredentialStoreKind::SecretService,
				},
			],
			..equipped_snapshot()
		}
	}

	async fn start(dir: &TempDir, probe: Arc<FixedProbe>) -> Core {
		start_core_with(
			&dir.path().join("plane.sqlite3"),
			ManualClock::at(observed_at()),
			probe,
		)
		.await
	}

	async fn capabilities(
		core: &Core,
		observation: CapabilityObservation,
	) -> CapabilitySnapshot {
		let result = core
			.query(&actor(), Query::Capabilities { observation })
			.await
			.unwrap();
		let QueryResult::Capabilities(snapshot) = result else {
			panic!("expected QueryResult::Capabilities");
		};
		snapshot
	}

	async fn auto_commit(core: &Core, scope: SettingScope) -> ResolvedSetting {
		let result = core
			.query(
				&actor(),
				Query::Settings {
					scope,
					selection: SettingSelection::Key(SettingKey::GitAutoCommit),
				},
			)
			.await
			.unwrap();
		let QueryResult::Settings(snapshot) = result else {
			panic!("expected QueryResult::Settings");
		};
		snapshot
			.settings
			.into_iter()
			.next()
			.expect("auto-commit setting")
	}

	async fn enable_auto_commit(
		core: &Core,
		scope: SettingScope,
	) -> Result<(), CoreError> {
		core.execute(
			&actor(),
			request(Command::SetSetting {
				key: SettingKey::GitAutoCommit,
				scope,
				value: SettingValue::Flag(true),
			}),
		)
		.await
		.map(|_| ())
	}

	#[tokio::test]
	async fn a_snapshot_reports_the_plane_and_what_leaves_it_degraded() {
		let dir = tempfile::tempdir().unwrap();
		let probe = FixedProbe::new(equipped());
		let core = start(&dir, Arc::clone(&probe)).await;

		let at_startup =
			capabilities(&core, CapabilityObservation::LastObserved).await;
		probe.answer_with(stripped());
		let after_losing_them =
			capabilities(&core, CapabilityObservation::Fresh).await;

		assert_eq!(
			(at_startup, after_losing_them),
			(equipped_snapshot(), stripped_snapshot())
		);
	}

	/// ADR-0086 keeps `jetd` from polling, so a client reads the last
	/// observation until something asks the Plane again.
	#[tokio::test]
	async fn the_last_observation_stands_until_the_plane_is_observed_again() {
		let dir = tempfile::tempdir().unwrap();
		let probe = FixedProbe::new(equipped());
		let core = start(&dir, Arc::clone(&probe)).await;
		probe.answer_with(stripped());

		let stale =
			capabilities(&core, CapabilityObservation::LastObserved).await;
		let fresh = capabilities(&core, CapabilityObservation::Fresh).await;
		let kept =
			capabilities(&core, CapabilityObservation::LastObserved).await;

		assert_eq!(
			(stale, fresh, kept),
			(
				equipped_snapshot(),
				stripped_snapshot(),
				stripped_snapshot()
			)
		);
	}

	/// A client prepares a Command against the Capabilities it just read. The
	/// Plane may change before the Command runs, so the Command is answered by
	/// what the Plane can do now, and it changes nothing when the answer is no
	/// (ADR-0086).
	#[tokio::test]
	async fn a_command_whose_capability_disappeared_commits_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let probe = FixedProbe::new(equipped());
		let core = start(&dir, Arc::clone(&probe)).await;
		let scope = SettingScope::Project {
			project_id: register_repository(&core, &dir.path().join("repo"))
				.await,
		};
		let observed = capabilities(&core, CapabilityObservation::Fresh).await;
		probe.answer_with(stripped());

		let refused = enable_auto_commit(&core, scope).await.unwrap_err();
		let unchanged = auto_commit(&core, scope).await;
		probe.answer_with(equipped());
		let accepted = enable_auto_commit(&core, scope).await;
		let stored = auto_commit(&core, scope).await;

		assert_eq!(
			(observed, refused, unchanged, accepted, stored),
			(
				equipped_snapshot(),
				CoreError {
					category: ErrorCategory::Unavailable,
					code: "capability.unavailable".into(),
					retryable: false,
					message: "this Plane cannot use the git command-line tool \
				          right now"
						.into(),
					detail: None,
					revision_conflict: None,
					recovery_actions: vec![],
				},
				ResolvedSetting {
					key: SettingKey::GitAutoCommit,
					value: SettingValue::Flag(false),
					source: SettingSource::BuiltIn,
				},
				Ok(()),
				ResolvedSetting {
					key: SettingKey::GitAutoCommit,
					value: SettingValue::Flag(true),
					source: SettingSource::Scope(scope),
				},
			)
		);
	}
}
