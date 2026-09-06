use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::test_support::{actor, register_repository, request, start_core};
use crate::{
	Command, CommandOutcome, ConversationId, Core, CoreError, ErrorCategory,
	EventKind, ProjectId, Query, QueryResult, ResolvedSetting, RetentionPolicy,
	SettingKey, SettingScope, SettingSelection, SettingSource, SettingValue,
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
			}),
		)
		.await
		.unwrap();
	let CommandOutcome::ConversationCreated(conversation) = outcome else {
		panic!("unexpected outcome {outcome:?}");
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
	core.execute(&actor(), request(Command::SetSetting { key, scope, value }))
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
		panic!("unexpected result {result:?}");
	};
	assert_eq!(snapshot.scope, scope);
	Ok(snapshot.settings)
}

async fn resolve_one(
	core: &Core,
	scope: SettingScope,
	key: SettingKey,
) -> ResolvedSetting {
	let mut settings = resolve(core, scope, SettingSelection::Key(key))
		.await
		.unwrap();
	assert_eq!(settings.len(), 1);
	settings.remove(0)
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
		project_id: register_repository(&core, &dir.path().join("repo")).await,
	};
	let elsewhere = SettingScope::Project {
		project_id: register_repository(&core, &dir.path().join("elsewhere"))
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
			],
			vec![
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
	let cleared = clear(&core, SettingKey::GitAutoCommit, SettingScope::Plane)
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
		[&written, &read].map(|error| (error.category, error.code.as_str())),
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
		panic!("unexpected result {result:?}");
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
