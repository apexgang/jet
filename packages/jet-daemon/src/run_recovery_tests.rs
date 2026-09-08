//! Boot evidence at the public host port remains independent of a reused PID.
use crate::run_host::CraftProcesses;
use jet_core::{LaunchPlan, PinnedCraft, RunHost, RunId, RunRecoveryError};
use pretty_assertions::assert_eq;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn a_changed_boot_proves_loss_even_when_the_previous_pid_is_live() {
	let home = tempfile::tempdir().unwrap();
	let host = CraftProcesses::default();
	let id = RunId(Uuid::new_v4());
	let current = jet_runtime::execution_boot_identity().unwrap();
	let plan = |boot: &str| LaunchPlan {
		visa: None,
		turn_id: None,
		native_conversation: None,
		fork: None,
		version: 1,
		root: home.path().into(),
		project_root: home.path().into(),
		craft: PinnedCraft {
			executable: home.path().join("craft"),
			sha256: String::new(),
			adapter_state: json!({
				"version":1,"boot_identity":boot,
				"craft_protocol":{"major":1,"minor":2},
				"helper_protocol":{"major":1,"minor":1},
				"specification":{
					"schema":{"major":1,"minor":0},"id":"test","harness":"test",
					"protocol":{"family":"craft","versions":[{"major":1,"minor":2}]}
				}
			})
			.to_string(),
		},
		prompt: "accepted input".into(),
		client_id: jet_core::ClientId(Uuid::new_v4()),
	};
	let prior_boot = Some(plan("an earlier OS boot"));
	let same_boot = Some(plan(&current));
	let live_pid = Some(std::process::id());
	let gone = host
		.probe(home.path().into(), id, prior_boot, live_pid)
		.await;
	let unsafe_match = host
		.probe(home.path().into(), id, same_boot, live_pid)
		.await;
	assert_eq!(
		(gone, unsafe_match),
		(Err(RunRecoveryError::Gone), Err(RunRecoveryError::Unsafe))
	);
}
