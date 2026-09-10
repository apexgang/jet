//! Mutable Settings: the catalog the core understands, the scopes that may
//! store each one, and how a Query resolves them (ADR-0085).
//!
//! `~/.jet/config.toml` keeps only the bootstrap values `jetd` needs before
//! its store opens. Everything here is mutable Plane state that changes
//! through authenticated Commands and resolves from built-in defaults
//! through the Plane, Project, and Conversation scopes, except where a key
//! is restricted to narrower ones.
//!
//! A restriction says where a value may be *stored*, so a Command that
//! names an unsupported scope is refused. It never narrows what applies: a
//! Plane-wide value still resolves for a Conversation that cannot override
//! it.

mod resolution;
pub(crate) use resolution::{
	event_subject, prepare_clear, prepare_write, require_subject, resolve,
	resolve_plane,
};

mod key;
pub use key::SettingKey;

use crate::{
	ProjectId, conversation::ConversationId, error::CoreError,
	event::EventSequence,
};
use jet_store::SettingScopeRecord;
use serde::{Deserialize, Serialize};

/// Largest text value one Setting may carry. The store bounds the encoded
/// row as well, with room for JSON escaping above this limit.
const MAX_SETTING_TEXT_BYTES: usize = 2048;

/// Where a Setting value lives. A Command writes exactly the scope it
/// names; a Query resolves the Plane's values and then the values of the
/// scope it names (ADR-0085).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum SettingScope {
	/// Everything on the Plane.
	Plane,
	/// One registered Project.
	Project {
		/// The Project the values apply to.
		project_id: ProjectId,
	},
	/// One Conversation.
	Conversation {
		/// The Conversation the values apply to.
		conversation_id: ConversationId,
	},
}

/// A scope without its subject identity, used to declare where a key may
/// be stored and to order one scope against another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SettingScopeKind {
	/// Everything on the Plane; the widest scope above the built-in default.
	Plane,
	/// One registered Project.
	Project,
	/// One Conversation; the narrowest scope, resolved last.
	Conversation,
}

impl SettingScope {
	fn kind(self) -> SettingScopeKind {
		match self {
			Self::Plane => SettingScopeKind::Plane,
			Self::Project { .. } => SettingScopeKind::Project,
			Self::Conversation { .. } => SettingScopeKind::Conversation,
		}
	}

	pub(crate) fn record(self) -> SettingScopeRecord {
		match self {
			Self::Plane => SettingScopeRecord::Plane,
			Self::Project { project_id } => SettingScopeRecord::Project {
				project_id: project_id.0,
			},
			Self::Conversation { conversation_id } => {
				SettingScopeRecord::Conversation {
					conversation_id: conversation_id.0,
				}
			}
		}
	}

	fn from_record(record: SettingScopeRecord) -> Self {
		match record {
			SettingScopeRecord::Plane => Self::Plane,
			SettingScopeRecord::Project { project_id } => Self::Project {
				project_id: ProjectId(project_id),
			},
			SettingScopeRecord::Conversation { conversation_id } => {
				Self::Conversation {
					conversation_id: ConversationId(conversation_id),
				}
			}
		}
	}
}

impl SettingScopeKind {
	fn as_str(self) -> &'static str {
		match self {
			Self::Plane => "Plane",
			Self::Project => "Project",
			Self::Conversation => "Conversation",
		}
	}
}

/// One Setting's value. Each key holds exactly one of these shapes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SettingValue {
	/// A yes-or-no choice.
	Flag(bool),
	/// Bounded free text.
	Text(String),
	/// A whole number of something, such as days.
	Count(u32),
}

impl SettingValue {
	fn encode(&self) -> Result<String, CoreError> {
		serde_json::to_string(self).map_err(|error| {
			CoreError::internal("setting.value_unencodable", error.to_string())
		})
	}

	fn decode(text: &str) -> Option<Self> {
		serde_json::from_str(text).ok()
	}
}

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
	/// The core's built-in default; no scope stores a value.
	BuiltIn,
	/// The value one scope stores.
	Scope(SettingScope),
}

/// One Setting as it applies to the scope a Query addressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSetting {
	/// The Setting.
	pub key: SettingKey,
	/// Its value after precedence.
	pub value: SettingValue,
	/// The scope the value came from.
	pub source: SettingSource,
}

/// Which Settings one Query resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSelection {
	/// Every Setting the core understands.
	All,
	/// One named Setting.
	Key(SettingKey),
}

/// Settings resolved for one scope, fenced by the journal position the
/// snapshot was read at (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingSnapshot {
	/// Newest Event sequence visible when the snapshot was read.
	pub cursor: EventSequence,
	/// The scope the Settings were resolved for.
	pub scope: SettingScope,
	/// The resolved Settings in catalog order.
	pub settings: Vec<ResolvedSetting>,
}

impl SettingSelection {
	/// The keys this selection resolves.
	pub(crate) fn keys(self) -> Vec<SettingKey> {
		match self {
			Self::All => SettingKey::all(),
			Self::Key(key) => vec![key],
		}
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use pretty_assertions::assert_eq;
	use uuid::Uuid;

	use crate::test_support::{
		actor, register_repository, request, start_core,
	};
	use crate::{
		Command, CommandOutcome, ConversationId, Core, CoreError,
		ErrorCategory, EventKind, ProjectId, Query, QueryResult,
		ResolvedSetting, RetentionPolicy, SettingKey, SettingScope,
		SettingSelection, SettingSource, SettingValue, WorkingTreeRequest,
	};

	async fn start(path: &tempfile::TempDir) -> Core {
		start_core(&path.path().join("plane.sqlite3")).await
	}

	async fn conversation(core: &Core) -> SettingScope {
		let outcome = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::NoProject,
				}),
			)
			.await
			.unwrap();
		let CommandOutcome::ConversationCreated(conversation) = outcome else {
			panic!("expected CommandOutcome::ConversationCreated");
		};
		SettingScope::Conversation {
			conversation_id: conversation.conversation_id,
		}
	}

	async fn set(
		core: &Core,
		key: SettingKey,
		scope: SettingScope,
		value: SettingValue,
	) -> Result<CommandOutcome, CoreError> {
		core.execute(
			&actor(),
			request(Command::SetSetting { key, scope, value }),
		)
		.await
	}

	async fn clear(
		core: &Core,
		key: SettingKey,
		scope: SettingScope,
	) -> Result<CommandOutcome, CoreError> {
		core.execute(&actor(), request(Command::ClearSetting { key, scope }))
			.await
	}

	async fn resolve(
		core: &Core,
		scope: SettingScope,
		selection: SettingSelection,
	) -> Result<Vec<ResolvedSetting>, CoreError> {
		let result = core
			.query(&actor(), Query::Settings { scope, selection })
			.await?;
		let QueryResult::Settings(snapshot) = result else {
			panic!("expected QueryResult::Settings");
		};
		assert_eq!(snapshot.scope, scope);
		Ok(snapshot.settings)
	}

	async fn resolve_one(
		core: &Core,
		scope: SettingScope,
		key: SettingKey,
	) -> ResolvedSetting {
		let settings = resolve(core, scope, SettingSelection::Key(key))
			.await
			.unwrap();
		assert_eq!(settings.len(), 1);
		settings.into_iter().next().expect("one resolved setting")
	}

	fn resolved(
		key: SettingKey,
		value: SettingValue,
		source: SettingSource,
	) -> ResolvedSetting {
		ResolvedSetting { key, value, source }
	}

	#[tokio::test]
	async fn a_setting_resolves_from_the_narrowest_scope_that_stores_it() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let scope = conversation(&core).await;
		let key = SettingKey::UtilityAutomaticNaming;

		let built_in = resolve_one(&core, scope, key).await;
		set(&core, key, SettingScope::Plane, SettingValue::Flag(false))
			.await
			.unwrap();
		let from_plane = resolve_one(&core, scope, key).await;
		set(&core, key, scope, SettingValue::Flag(true))
			.await
			.unwrap();
		let from_conversation = resolve_one(&core, scope, key).await;
		clear(&core, key, scope).await.unwrap();
		let after_clearing = resolve_one(&core, scope, key).await;

		assert_eq!(
			[built_in, from_plane, from_conversation, after_clearing],
			[
				resolved(key, SettingValue::Flag(true), SettingSource::BuiltIn),
				resolved(
					key,
					SettingValue::Flag(false),
					SettingSource::Scope(SettingScope::Plane)
				),
				resolved(
					key,
					SettingValue::Flag(true),
					SettingSource::Scope(scope)
				),
				resolved(
					key,
					SettingValue::Flag(false),
					SettingSource::Scope(SettingScope::Plane)
				),
			]
		);
	}

	#[tokio::test]
	async fn a_project_resolves_its_own_values_over_the_planes() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let key = SettingKey::UtilityAutomaticNaming;
		let scope = SettingScope::Project {
			project_id: register_repository(&core, &dir.path().join("repo"))
				.await,
		};
		let elsewhere = SettingScope::Project {
			project_id: register_repository(
				&core,
				&dir.path().join("elsewhere"),
			)
			.await,
		};

		set(&core, key, SettingScope::Plane, SettingValue::Flag(false))
			.await
			.unwrap();
		set(&core, key, scope, SettingValue::Flag(true))
			.await
			.unwrap();

		assert_eq!(
			[
				resolve_one(&core, scope, key).await,
				resolve_one(&core, elsewhere, key).await
			],
			[
				resolved(
					key,
					SettingValue::Flag(true),
					SettingSource::Scope(scope)
				),
				resolved(
					key,
					SettingValue::Flag(false),
					SettingSource::Scope(SettingScope::Plane)
				),
			]
		);
	}

	/// A Project scope names a registered Project (ADR-0085), so a Setting
	/// cannot be stored for, or resolved through, a Project this Plane does
	/// not have.
	#[tokio::test]
	async fn a_setting_for_an_unregistered_project_is_refused() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let key = SettingKey::UtilityAutomaticNaming;
		let scope = SettingScope::Project {
			project_id: ProjectId(Uuid::now_v7()),
		};

		let stored = set(&core, key, scope, SettingValue::Flag(true))
			.await
			.unwrap_err();
		let resolved = resolve(&core, scope, SettingSelection::Key(key))
			.await
			.unwrap_err();

		assert_eq!(
			[
				(stored.category, stored.code),
				(resolved.category, resolved.code)
			],
			[
				(ErrorCategory::NotFound, "project.not_found".into()),
				(ErrorCategory::NotFound, "project.not_found".into()),
			]
		);
	}

	#[tokio::test]
	async fn a_scope_resolves_every_setting_that_applies_to_it() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let scope = conversation(&core).await;

		set(
			&core,
			SettingKey::GitAutoCommit,
			scope,
			SettingValue::Flag(true),
		)
		.await
		.unwrap();
		set(
			&core,
			SettingKey::GitMessageInstructions,
			SettingScope::Plane,
			SettingValue::Text("Explain why, not what".into()),
		)
		.await
		.unwrap();

		assert_eq!(
			(
				resolve(&core, scope, SettingSelection::All).await.unwrap(),
				resolve(&core, SettingScope::Plane, SettingSelection::All)
					.await
					.unwrap()
			),
			(
				vec![
					resolved(
						SettingKey::StorageDisposableMiB,
						SettingValue::Count(5120),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::GitAutoBranch,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::GitAutoPush,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::GitAutoDraftPullRequest,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::GitBranchPrefix,
						SettingValue::Text("jet/".into()),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::UtilityAutomaticNaming,
						SettingValue::Flag(true),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::GitAutoCommit,
						SettingValue::Flag(true),
						SettingSource::Scope(scope)
					),
					// Plane-wide guidance applies to the Conversation even
					// though the Conversation cannot override it.
					resolved(
						SettingKey::GitMessageInstructions,
						SettingValue::Text("Explain why, not what".into()),
						SettingSource::Scope(SettingScope::Plane)
					),
					resolved(
						SettingKey::SecurityAuditRetentionDays,
						SettingValue::Count(365),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::UtilityGitText,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::UtilityContentConsent,
						SettingValue::Text(String::new()),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::UtilityAccountBinding,
						SettingValue::Text(String::new()),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::UtilityAutodeleteCompilation,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::DeveloperMode,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::AutomaticReview,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::AutomaticReviewBinding,
						SettingValue::Text(String::new()),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::AutomaticReviewConsent,
						SettingValue::Text(String::new()),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::ArtifactMaxMiB,
						SettingValue::Count(512),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::ArtifactRunMiB,
						SettingValue::Count(2048),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::EnergyConcurrency,
						SettingValue::Count(8),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::EnergyLowPowerConcurrency,
						SettingValue::Count(1),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::EnergyConstrained,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::EnergyForegroundOverride,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
				],
				vec![
					resolved(
						SettingKey::StorageDisposableMiB,
						SettingValue::Count(5120),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::GitAutoBranch,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::GitAutoPush,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::GitAutoDraftPullRequest,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::GitBranchPrefix,
						SettingValue::Text("jet/".into()),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::UtilityAutomaticNaming,
						SettingValue::Flag(true),
						SettingSource::BuiltIn
					),
					// No scope the Plane resolves through stores Git
					// automation, so its built-in default is what applies.
					resolved(
						SettingKey::GitAutoCommit,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::GitMessageInstructions,
						SettingValue::Text("Explain why, not what".into()),
						SettingSource::Scope(SettingScope::Plane)
					),
					resolved(
						SettingKey::SecurityAuditRetentionDays,
						SettingValue::Count(365),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::UtilityGitText,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::UtilityContentConsent,
						SettingValue::Text(String::new()),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::UtilityAccountBinding,
						SettingValue::Text(String::new()),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::UtilityAutodeleteCompilation,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::DeveloperMode,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::AutomaticReview,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::AutomaticReviewBinding,
						SettingValue::Text(String::new()),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::AutomaticReviewConsent,
						SettingValue::Text(String::new()),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::ArtifactMaxMiB,
						SettingValue::Count(512),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::ArtifactRunMiB,
						SettingValue::Count(2048),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::EnergyConcurrency,
						SettingValue::Count(8),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::EnergyLowPowerConcurrency,
						SettingValue::Count(1),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::EnergyConstrained,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
					resolved(
						SettingKey::EnergyForegroundOverride,
						SettingValue::Flag(false),
						SettingSource::BuiltIn
					),
				]
			)
		);
	}

	/// ADR-0085 restricts where some Settings may be stored. The restriction
	/// binds the Commands that store them; what a restricted Setting applies to
	/// is unchanged, so a Conversation still resolves the Plane's value.
	#[tokio::test]
	async fn a_restricted_setting_refuses_the_scopes_it_is_not_stored_at() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let scope = conversation(&core).await;

		let written = set(
			&core,
			SettingKey::GitMessageInstructions,
			scope,
			SettingValue::Text("Explain why, not what".into()),
		)
		.await
		.unwrap_err();
		let cleared =
			clear(&core, SettingKey::GitAutoCommit, SettingScope::Plane)
				.await
				.unwrap_err();
		set(
			&core,
			SettingKey::GitMessageInstructions,
			SettingScope::Plane,
			SettingValue::Text("Explain why, not what".into()),
		)
		.await
		.unwrap();
		let read =
			resolve_one(&core, scope, SettingKey::GitMessageInstructions).await;

		assert_eq!(
			(
				[&written, &cleared].map(|error| (
					error.category,
					error.code.as_str(),
					error.retryable
				)),
				written.message.as_str(),
				read,
			),
			(
				[(
					ErrorCategory::InvalidInput,
					"setting.scope_unsupported",
					false
				); 2],
				"the Setting git.message_instructions is stored at the Plane \
			 scope only, not the Conversation scope",
				resolved(
					SettingKey::GitMessageInstructions,
					SettingValue::Text("Explain why, not what".into()),
					SettingSource::Scope(SettingScope::Plane)
				),
			)
		);
	}

	#[tokio::test]
	async fn a_setting_refuses_a_value_it_cannot_hold() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;

		let wrong_shape = set(
			&core,
			SettingKey::UtilityAutomaticNaming,
			SettingScope::Plane,
			SettingValue::Text("yes".into()),
		)
		.await
		.unwrap_err();
		let too_long = set(
			&core,
			SettingKey::GitMessageInstructions,
			SettingScope::Plane,
			SettingValue::Text("x".repeat(2049)),
		)
		.await
		.unwrap_err();

		assert_eq!(
			[&wrong_shape, &too_long]
				.map(|error| (error.category, error.code.as_str())),
			[
				(ErrorCategory::InvalidInput, "setting.value_unsupported"),
				(ErrorCategory::InvalidInput, "setting.value_too_long"),
			]
		);
	}

	#[tokio::test]
	async fn a_conversation_scope_names_a_conversation_this_plane_has() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let scope = SettingScope::Conversation {
			conversation_id: ConversationId(Uuid::now_v7()),
		};
		let key = SettingKey::UtilityAutomaticNaming;

		let written = set(&core, key, scope, SettingValue::Flag(false))
			.await
			.unwrap_err();
		let read = resolve(&core, scope, SettingSelection::All)
			.await
			.unwrap_err();

		assert_eq!(
			[&written, &read]
				.map(|error| (error.category, error.code.as_str())),
			[(ErrorCategory::NotFound, "conversation.not_found"); 2]
		);
	}

	/// A Setting change is Conversation-independent history a Security audit
	/// and a client's replay both need, so it lands in the journal beside the
	/// Commands that changed Runs (ADR-0020).
	#[tokio::test]
	async fn setting_changes_reach_the_event_journal() {
		let dir = tempfile::tempdir().unwrap();
		let core = start(&dir).await;
		let key = SettingKey::GitMessageInstructions;
		let value = SettingValue::Text("Explain why, not what".into());

		set(&core, key, SettingScope::Plane, value.clone())
			.await
			.unwrap();
		clear(&core, key, SettingScope::Plane).await.unwrap();

		let result = core
			.query(
				&actor(),
				Query::Events {
					after: crate::EventSequence(0),
				},
			)
			.await
			.unwrap();
		let QueryResult::Events(page) = result else {
			panic!("expected QueryResult::Events");
		};
		assert_eq!(
			page.events
				.iter()
				.map(|event| (event.kind.clone(), event.conversation_id))
				.collect::<Vec<_>>(),
			vec![
				(
					EventKind::SettingChanged {
						key,
						scope: SettingScope::Plane,
						value
					},
					None
				),
				(
					EventKind::SettingCleared {
						key,
						scope: SettingScope::Plane
					},
					None
				),
			]
		);
	}
}
