use super::{install_craft, start_core};
use crate::test_support::{
	actor, conversation_snapshot, register_repository, request,
};
use crate::{
	Command, CommandOutcome, Core, RetentionPolicy, SettingKey, SettingScope,
	SettingValue, WorkingTreeRequest,
};
use pretty_assertions::assert_eq;

async fn set(core: &Core, key: SettingKey, value: SettingValue) {
	core.execute(
		&actor(),
		request(Command::SetSetting {
			key,
			scope: SettingScope::Plane,
			value,
		}),
	)
	.await
	.unwrap();
}

async fn conversation(
	core: &Core,
	root: &std::path::Path,
) -> crate::ConversationId {
	let project_id = register_repository(core, root).await;
	let CommandOutcome::ConversationCreated(conversation) = core
		.execute(
			&actor(),
			request(Command::CreateConversation {
				retention: RetentionPolicy::Retain,
				working_tree: WorkingTreeRequest::LocalCheckout { project_id },
			}),
		)
		.await
		.unwrap()
	else {
		panic!("Conversation expected")
	};
	conversation.conversation_id
}

fn start(id: crate::ConversationId) -> Command {
	Command::StartRun {
		conversation_id: id,
		craft: "fake".into(),
		prompt: "Continue".into(),
	}
}

#[tokio::test]
async fn energy_budget_changes_preserve_admitted_runs_and_require_explicit_foreground_override()
 {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("plane.sqlite3")).await;
	install_craft(dir.path());
	let first = conversation(&core, &dir.path().join("first")).await;
	let second = conversation(&core, &dir.path().join("second")).await;
	set(
		&core,
		SettingKey::EnergyConstrained,
		SettingValue::Flag(true),
	)
	.await;
	set(
		&core,
		SettingKey::EnergyLowPowerConcurrency,
		SettingValue::Count(1),
	)
	.await;
	core.execute(&actor(), request(start(first))).await.unwrap();
	let admitted = conversation_snapshot(&core, first).await;
	let refused = core
		.execute(&actor(), request(start(second)))
		.await
		.unwrap_err();
	assert_eq!(refused.code, "energy.budget_exhausted");
	set(
		&core,
		SettingKey::EnergyLowPowerConcurrency,
		SettingValue::Count(0),
	)
	.await;
	assert_eq!(
		conversation_snapshot(&core, first).await.runs,
		admitted.runs
	);
	assert_eq!(conversation_snapshot(&core, second).await.runs, vec![]);
	set(
		&core,
		SettingKey::EnergyForegroundOverride,
		SettingValue::Flag(true),
	)
	.await;
	core.execute(&actor(), request(start(second)))
		.await
		.unwrap();
}

#[tokio::test]
async fn concurrent_commands_cannot_double_book_the_last_energy_slot() {
	let dir = tempfile::tempdir().unwrap();
	let core = start_core(&dir.path().join("plane.sqlite3")).await;
	install_craft(dir.path());
	let first = conversation(&core, &dir.path().join("first")).await;
	let second = conversation(&core, &dir.path().join("second")).await;
	set(
		&core,
		SettingKey::EnergyConstrained,
		SettingValue::Flag(true),
	)
	.await;
	let actor = actor();
	let (first, second) = tokio::join!(
		core.execute(&actor, request(start(first))),
		core.execute(&actor, request(start(second))),
	);
	let results = [first, second];
	assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
	assert_eq!(
		results
			.into_iter()
			.filter_map(Result::err)
			.map(|error| error.code)
			.collect::<Vec<_>>(),
		vec!["energy.budget_exhausted"]
	);
}
