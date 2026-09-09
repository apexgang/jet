use crate::test_support::{actor, request, start_core};
use crate::*;
use pretty_assertions::assert_eq;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct NativeExtensions(Mutex<Vec<ExtensionConfirmation>>);
impl ExtensionHost for NativeExtensions {
	fn pin<'a>(
		&'a self,
		id: &'a str,
	) -> RunFuture<'a, Result<PinnedCraft, CoreError>> {
		Box::pin(async move {
			Ok(PinnedCraft {
				id: id.into(),
				executable: "/bin/cat".into(),
				sha256: "a".repeat(64),
				adapter_state: "test".into(),
			})
		})
	}

	fn catalog<'a>(
		&'a self,
		craft: &'a PinnedCraft,
	) -> RunFuture<'a, Result<ExtensionCatalog, CoreError>> {
		Box::pin(async move {
			Ok(ExtensionCatalog {
				craft_id: craft.id.clone(),
				harness: "demo".into(),
				native_metadata:
					r#"{"plugins":[{"id":"tool@official","version":"1"}]}"#
						.into(),
			})
		})
	}
	fn inspect<'a>(
		&'a self,
		craft: &'a PinnedCraft,
		_id: &'a str,
	) -> RunFuture<'a, Result<ExtensionCatalog, CoreError>> {
		self.catalog(craft)
	}
	fn apply<'a>(
		&'a self,
		_craft: &'a PinnedCraft,
		confirmation: &'a ExtensionConfirmation,
	) -> RunFuture<'a, Result<(), CoreError>> {
		Box::pin(async move {
			self.0.lock().unwrap().push(confirmation.clone());
			Ok(())
		})
	}
}

#[tokio::test]
async fn a_confirmed_native_extension_change_is_durable_and_audited() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let host = Arc::new(NativeExtensions(Mutex::new(vec![])));
	let core = start_core(&path).await.with_extension_host(host.clone());
	let QueryResult::ExtensionCatalog(catalog) = core
		.query(
			&actor(),
			Query::ExtensionCatalog {
				craft_id: "demo".into(),
			},
		)
		.await
		.unwrap()
	else {
		panic!("catalog expected")
	};
	let confirmation = ExtensionConfirmation {
		catalog,
		extension_id: "tool@official".into(),
		action: ExtensionAction::Install,
		scope: ExtensionScope::User,
		trust: ExtensionTrust::SameUserExecutable,
	};
	let result = core
		.execute(
			&actor(),
			request(Command::ChangeExtension {
				confirmation: confirmation.clone(),
			}),
		)
		.await
		.unwrap();
	let CommandOutcome::ExtensionChangeQueued { change_id } = result else {
		panic!("queued change expected")
	};
	assert!(host.0.lock().unwrap().is_empty());
	drop(core);
	let core = start_core(&path).await.with_extension_host(host.clone());
	core.perform_extension_changes().await.unwrap();
	assert_eq!(*host.0.lock().unwrap(), vec![confirmation]);
	let QueryResult::ExtensionChange(change) = core
		.query(&actor(), Query::ExtensionChange { change_id })
		.await
		.unwrap()
	else {
		panic!("change expected")
	};
	assert_eq!(change.state, ExtensionChangeState::Applied);
	let QueryResult::SecurityAudit(audit) = core
		.query(
			&actor(),
			Query::SecurityAudit {
				after: AuditSequence(0),
			},
		)
		.await
		.unwrap()
	else {
		panic!("audit expected")
	};
	assert!(
		audit
			.entries
			.iter()
			.any(|entry| entry.decision == "extension.install")
	);
}

#[tokio::test]
async fn stale_native_metadata_is_refused_before_staging() {
	let dir = tempfile::tempdir().unwrap();
	let host = Arc::new(NativeExtensions(Mutex::new(vec![])));
	let core = start_core(&dir.path().join("plane.sqlite3"))
		.await
		.with_extension_host(host.clone());
	let QueryResult::ExtensionCatalog(mut catalog) = core
		.query(
			&actor(),
			Query::ExtensionCatalog {
				craft_id: "demo".into(),
			},
		)
		.await
		.unwrap()
	else {
		panic!()
	};
	catalog.native_metadata = r#"{"permissions":["changed"]}"#.into();
	let error = core
		.execute(
			&actor(),
			request(Command::ChangeExtension {
				confirmation: ExtensionConfirmation {
					catalog,
					extension_id: "tool@official".into(),
					action: ExtensionAction::Update,
					scope: ExtensionScope::User,
					trust: ExtensionTrust::SameUserExecutable,
				},
			}),
		)
		.await
		.unwrap_err();
	assert_eq!(error.code, "extension.preview_stale");
	core.perform_extension_changes().await.unwrap();
	assert!(host.0.lock().unwrap().is_empty());
}

#[derive(Debug)]
struct RunAdmission(PinnedCraft);
impl RunHost for RunAdmission {
	fn pin(
		&self,
		_home: std::path::PathBuf,
		_id: String,
	) -> RunFuture<'_, Result<PinnedCraft, CoreError>> {
		Box::pin(async { Ok(self.0.clone()) })
	}
	fn prepare_next_run(
		&self,
		plan: LaunchPlan,
	) -> RunFuture<'_, Result<LaunchPlan, CoreError>> {
		Box::pin(async { Ok(plan) })
	}
	fn start(
		&self,
		_home: std::path::PathBuf,
		_id: RunId,
		_plan: LaunchPlan,
	) -> RunFuture<'_, Result<Box<dyn RunConnection>, RunStartError>> {
		Box::pin(async { Err(RunStartError::NotStarted) })
	}
}
#[tokio::test]
async fn staged_changes_wait_for_existing_runs_and_hold_subsequent_runs() {
	use sha2::{Digest, Sha256};
	let dir = tempfile::tempdir().unwrap();
	let host = Arc::new(NativeExtensions(Mutex::new(vec![])));
	let executable = std::path::Path::new("/bin/cat").canonicalize().unwrap();
	let pin = PinnedCraft {
		id: "demo".into(),
		sha256: format!(
			"{:x}",
			Sha256::digest(std::fs::read(&executable).unwrap())
		),
		executable,
		adapter_state: "test".into(),
	};
	let core = Arc::new(
		start_core(&dir.path().join("plane.sqlite3"))
			.await
			.with_extension_host(host.clone())
			.with_run_host(Arc::new(RunAdmission(pin))),
	);
	let project_id = crate::test_support::register_repository(
		&core,
		&dir.path().join("repo"),
	)
	.await;
	let mut conversations = vec![];
	for _ in 0..2 {
		let CommandOutcome::ConversationCreated(conversation) = core
			.execute(
				&actor(),
				request(Command::CreateConversation {
					retention: RetentionPolicy::Retain,
					working_tree: WorkingTreeRequest::LocalCheckout {
						project_id,
					},
				}),
			)
			.await
			.unwrap()
		else {
			panic!()
		};
		conversations.push(conversation.conversation_id);
	}
	core.execute(
		&actor(),
		request(Command::StartRun {
			conversation_id: conversations[0],
			craft: "demo".into(),
			prompt: "first".into(),
		}),
	)
	.await
	.unwrap();
	let QueryResult::ExtensionCatalog(catalog) = core
		.query(
			&actor(),
			Query::ExtensionCatalog {
				craft_id: "demo".into(),
			},
		)
		.await
		.unwrap()
	else {
		panic!()
	};
	core.execute(
		&actor(),
		request(Command::ChangeExtension {
			confirmation: ExtensionConfirmation {
				catalog,
				extension_id: "tool@official".into(),
				action: ExtensionAction::Remove,
				scope: ExtensionScope::User,
				trust: ExtensionTrust::SameUserExecutable,
			},
		}),
	)
	.await
	.unwrap();
	core.perform_extension_changes().await.unwrap();
	assert!(host.0.lock().unwrap().is_empty());
	let error = core
		.execute(
			&actor(),
			request(Command::StartRun {
				conversation_id: conversations[1],
				craft: "demo".into(),
				prompt: "second".into(),
			}),
		)
		.await
		.unwrap_err();
	assert_eq!(error.code, "extension.change_pending");
	core.perform_runs().await.unwrap();
	core.perform_extension_changes().await.unwrap();
	assert_eq!(host.0.lock().unwrap().len(), 1);
	core.execute(
		&actor(),
		request(Command::StartRun {
			conversation_id: conversations[1],
			craft: "demo".into(),
			prompt: "second".into(),
		}),
	)
	.await
	.unwrap();
}

#[tokio::test]
async fn disabling_a_craft_refuses_staged_extensions_and_future_discovery() {
	let dir = tempfile::tempdir().unwrap();
	let host = Arc::new(NativeExtensions(Mutex::new(vec![])));
	let core = start_core(&dir.path().join("plane.sqlite3"))
		.await
		.with_extension_host(host.clone());
	let QueryResult::ExtensionCatalog(catalog) = core
		.query(
			&actor(),
			Query::InspectExtension {
				craft_id: "demo".into(),
				extension_id: "tool@official".into(),
			},
		)
		.await
		.unwrap()
	else {
		panic!()
	};
	let CommandOutcome::ExtensionChangeQueued { change_id } = core
		.execute(
			&actor(),
			request(Command::ChangeExtension {
				confirmation: ExtensionConfirmation {
					catalog,
					extension_id: "tool@official".into(),
					action: ExtensionAction::Install,
					scope: ExtensionScope::User,
					trust: ExtensionTrust::SameUserExecutable,
				},
			}),
		)
		.await
		.unwrap()
	else {
		panic!()
	};
	core.execute(
		&actor(),
		request(Command::DisableCraft {
			craft_id: "demo".into(),
			mode: CraftDisableMode::Wait,
		}),
	)
	.await
	.unwrap();
	core.perform_extension_changes().await.unwrap();
	assert!(host.0.lock().unwrap().is_empty());
	let QueryResult::ExtensionChange(change) = core
		.query(&actor(), Query::ExtensionChange { change_id })
		.await
		.unwrap()
	else {
		panic!()
	};
	assert_eq!(change.state, ExtensionChangeState::Refused);
	let error = core
		.query(
			&actor(),
			Query::ExtensionCatalog {
				craft_id: "demo".into(),
			},
		)
		.await
		.unwrap_err();
	assert_eq!(error.code, "craft.disabled");
}

#[derive(Debug)]
struct InterruptedExtensions(NativeExtensions);
impl ExtensionHost for InterruptedExtensions {
	fn pin<'a>(
		&'a self,
		id: &'a str,
	) -> RunFuture<'a, Result<PinnedCraft, CoreError>> {
		self.0.pin(id)
	}
	fn catalog<'a>(
		&'a self,
		craft: &'a PinnedCraft,
	) -> RunFuture<'a, Result<ExtensionCatalog, CoreError>> {
		self.0.catalog(craft)
	}
	fn inspect<'a>(
		&'a self,
		craft: &'a PinnedCraft,
		id: &'a str,
	) -> RunFuture<'a, Result<ExtensionCatalog, CoreError>> {
		self.0.inspect(craft, id)
	}
	fn apply<'a>(
		&'a self,
		craft: &'a PinnedCraft,
		confirmation: &'a ExtensionConfirmation,
	) -> RunFuture<'a, Result<(), CoreError>> {
		Box::pin(async move {
			self.0.apply(craft, confirmation).await?;
			Err(CoreError::conflict(
				"native.interrupted",
				"native process ended without a conclusive reply",
			))
		})
	}
}
#[tokio::test]
async fn an_uncertain_extension_mutation_is_audited_and_never_retried_after_restart()
 {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("plane.sqlite3");
	let host =
		Arc::new(InterruptedExtensions(NativeExtensions(Mutex::new(vec![]))));
	let core = start_core(&path).await.with_extension_host(host.clone());
	let QueryResult::ExtensionCatalog(catalog) = core
		.query(
			&actor(),
			Query::InspectExtension {
				craft_id: "demo".into(),
				extension_id: "tool@official".into(),
			},
		)
		.await
		.unwrap()
	else {
		panic!()
	};
	let command = request(Command::ChangeExtension {
		confirmation: ExtensionConfirmation {
			catalog,
			extension_id: "tool@official".into(),
			action: ExtensionAction::Remove,
			scope: ExtensionScope::User,
			trust: ExtensionTrust::SameUserExecutable,
		},
	});
	let accepted = core.execute(&actor(), command.clone()).await.unwrap();
	let CommandOutcome::ExtensionChangeQueued { change_id } = accepted.clone()
	else {
		panic!()
	};
	core.perform_extension_changes().await.unwrap();
	drop(core);
	let core = start_core(&path).await.with_extension_host(host.clone());
	assert_eq!(core.execute(&actor(), command).await.unwrap(), accepted);
	core.perform_extension_changes().await.unwrap();
	assert_eq!(host.0.0.lock().unwrap().len(), 1);
	let QueryResult::ExtensionChange(change) = core
		.query(&actor(), Query::ExtensionChange { change_id })
		.await
		.unwrap()
	else {
		panic!()
	};
	assert_eq!(change.state, ExtensionChangeState::OutcomeUnknown);
	let QueryResult::SecurityAudit(audit) = core
		.query(
			&actor(),
			Query::SecurityAudit {
				after: AuditSequence(0),
			},
		)
		.await
		.unwrap()
	else {
		panic!()
	};
	assert!(
		audit
			.entries
			.iter()
			.any(|entry| entry.decision == "extension.remove"
				&& entry.outcome == crate::AuditOutcome::Failed)
	);
}
