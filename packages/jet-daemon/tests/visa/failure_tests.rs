//! Destination capability changes and process failure through Visa admission.
use super::{
	assertions, connect, connect_raw, init_repository, install, native_binding,
	selection, start_jetd,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use uuid::Uuid;

enum ChangedCapability {
	Workspace,
	Project,
	Craft,
	Protocol,
}

#[tokio::test]
async fn destination_rechecks_working_roots_and_the_accepted_craft() {
	for changed in [
		ChangedCapability::Workspace,
		ChangedCapability::Project,
		ChangedCapability::Craft,
		ChangedCapability::Protocol,
	] {
		let dir = tempfile::tempdir_in("/tmp").unwrap();
		let home = dir.path().join("jet");
		install(&home);
		let daemon = start_jetd(&home).await;
		let client = connect(&daemon, Uuid::new_v4()).await;
		let root = init_repository(&dir.path().join("repo"));
		let mut request = selection(&client, &root).await;
		request.account_binding_id = native_binding(&client).await.binding_id;
		let snapshot =
			client.conversation(request.conversation_id).await.unwrap();
		let expected = match changed {
			ChangedCapability::Workspace => {
				std::fs::rename(
					snapshot.workspace.unwrap().root,
					dir.path().join("moved"),
				)
				.unwrap();
				"run.working_tree_unavailable"
			}
			ChangedCapability::Project => {
				std::fs::rename(root.join(".git"), root.join("replaced-git"))
					.unwrap();
				"run.working_tree_unavailable"
			}
			ChangedCapability::Craft => {
				std::fs::write(
					home.join("crafts/fake-craft"),
					"replaced executable",
				)
				.unwrap();
				"craft.unavailable"
			}
			ChangedCapability::Protocol => {
				let path = home.join("crafts/fake.json");
				let mut manifest: Value =
					serde_json::from_slice(&std::fs::read(&path).unwrap())
						.unwrap();
				manifest["specification"]["protocol"]["capabilities"] =
					json!([]);
				std::fs::write(path, serde_json::to_vec(&manifest).unwrap())
					.unwrap();
				"craft.unavailable"
			}
		};
		let error = client
			.start_visa_run(Uuid::now_v7(), request.clone())
			.await
			.unwrap_err();
		let jet_client::ClientError::Remote(error) = error else {
			panic!("{error:?}")
		};
		assert_eq!(error.code, expected);
		assert!(
			client
				.conversation(request.conversation_id)
				.await
				.unwrap()
				.runs
				.is_empty()
		);
	}
}

#[tokio::test]
async fn destination_failure_never_reissues_an_admitted_visa_run() {
	tokio::time::timeout(std::time::Duration::from_secs(40), async {
		for prompt in
			["Fail native launch", "Fail after spawn", "Make a change"]
		{
			let dir = tempfile::tempdir_in("/tmp").unwrap();
			let home = dir.path().join("jet");
			install(&home);
			let mut daemon = start_jetd(&home).await;
			let client_id = Uuid::new_v4();
			let client = connect(&daemon, client_id).await;
			let mut request =
				selection(&client, &init_repository(&dir.path().join("repo")))
					.await;
			request.account_binding_id =
				native_binding(&client).await.binding_id;
			request.prompt = prompt.into();
			let command_id = Uuid::now_v7();
			let run = client
				.start_visa_run(command_id, request.clone())
				.await
				.unwrap();
			let mut wire = connect_raw(&daemon, client_id).await;
			let state = if prompt == "Make a change" {
				"waiting_for_approval"
			} else {
				"failed"
			};
			let before =
				assertions::wait_for(&mut wire, &run.run_id.to_string(), state)
					.await;
			daemon.child.kill().await.unwrap();
			if prompt == "Make a change" {
				// Only PIDs reported for this test's controlled native execution.
				for process in
					before["processes"].as_array().unwrap().iter().rev()
				{
					let pid = rustix::process::Pid::from_raw(
						process["pid"].as_u64().unwrap() as i32,
					)
					.unwrap();
					let _ = rustix::process::kill_process(
						pid,
						rustix::process::Signal::KILL,
					);
				}
			}
			let daemon = start_jetd(&home).await;
			let client = connect(&daemon, client_id).await;
			let mut wire = connect_raw(&daemon, client_id).await;
			let state = if prompt == "Make a change" {
				"lost"
			} else {
				"failed"
			};
			let terminal =
				assertions::wait_for(&mut wire, &run.run_id.to_string(), state)
					.await;
			assert_eq!(terminal["activity"], Value::Null);
			assert_eq!(
				terminal["exit_code"],
				if prompt == "Fail after spawn" {
					json!(7)
				} else {
					Value::Null
				}
			);
			assert_eq!(
				client
					.start_visa_run(command_id, request.clone())
					.await
					.unwrap(),
				run
			);
			assert_eq!(
				client
					.conversation(request.conversation_id)
					.await
					.unwrap()
					.runs
					.len(),
				1
			);
			assert!(
				terminal["processes"]
					.as_array()
					.unwrap()
					.iter()
					.all(|process| process["running"] == false)
			);
		}
	})
	.await
	.unwrap();
}
