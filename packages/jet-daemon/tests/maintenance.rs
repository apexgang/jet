//! Durable non-Run work must receive an initial sweep on an idle restart.
mod support;
use jet_core::*;
use pretty_assertions::assert_eq;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

#[tokio::test]
async fn idle_restart_settles_a_staged_extension_without_a_waking_command() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let home = dir.path().join("jet");
	jet_runtime::JetHome::at(home.clone()).prepare().unwrap();
	let core = Core::start(
		jet_store::Store::open(&home.join("plane.sqlite3"))
			.await
			.unwrap(),
		WorkspaceHome(home.join("workspaces")),
	)
	.await
	.unwrap()
	.with_extension_host(Arc::new(Extensions));
	let owner = Uuid::new_v4();
	let actor = Actor::InteractiveClient {
		client_id: ClientId(owner),
	};
	let QueryResult::ExtensionCatalog(catalog) = core
		.query(
			&actor,
			Query::ExtensionCatalog {
				craft_id: "demo".into(),
			},
		)
		.await
		.unwrap()
	else {
		panic!("catalog")
	};
	let command = Command::ChangeExtension {
		confirmation: ExtensionConfirmation {
			catalog,
			extension_id: "tool@official".into(),
			action: ExtensionAction::Install,
			scope: ExtensionScope::User,
			trust: ExtensionTrust::SameUserExecutable,
		},
	};
	let envelope = CommandEnvelope::new(
		CommandId(Uuid::now_v7()),
		command.clone(),
		&serde_json::to_vec(&command).unwrap(),
	)
	.unwrap();
	let CommandOutcome::ExtensionChangeQueued { change_id } =
		core.execute(&actor, envelope).await.unwrap()
	else {
		panic!("staged")
	};
	core.close().await;
	let daemon = support::start_jetd(&home).await;
	let mut wire = support::connect_raw(&daemon, owner).await;
	tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            wire.send(&serde_json::json!({"kind":"query","id":1,"query":{"type":"extension_change","change_id":change_id}})).await;
            let reply: serde_json::Value = wire.receive().await;
            assert_eq!(reply["kind"], "query_result", "{reply}");
            if reply["result"]["state"] == "refused" { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("missing Craft must settle as refused without an unrelated Command");
}
#[derive(Debug)]
struct Extensions;
impl ExtensionHost for Extensions {
	fn pin<'a>(
		&'a self,
		id: &'a str,
	) -> RunFuture<'a, Result<PinnedCraft, CoreError>> {
		Box::pin(async move {
			Ok(PinnedCraft {
				id: id.into(),
				executable: "/bin/cat".into(),
				sha256: "a".repeat(64),
				adapter_state: "fixture".into(),
			})
		})
	}
	fn catalog<'a>(
		&'a self,
		pin: &'a PinnedCraft,
	) -> RunFuture<'a, Result<ExtensionCatalog, CoreError>> {
		Box::pin(async move {
			Ok(ExtensionCatalog {
				craft_id: pin.id.clone(),
				harness: "demo".into(),
				native_metadata: "{}".into(),
			})
		})
	}
	fn inspect<'a>(
		&'a self,
		pin: &'a PinnedCraft,
		_id: &'a str,
	) -> RunFuture<'a, Result<ExtensionCatalog, CoreError>> {
		self.catalog(pin)
	}
	fn apply<'a>(
		&'a self,
		_pin: &'a PinnedCraft,
		_confirmation: &'a ExtensionConfirmation,
	) -> RunFuture<'a, Result<(), CoreError>> {
		Box::pin(async { panic!("staging must not apply native changes") })
	}
}
