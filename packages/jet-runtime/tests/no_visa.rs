//! Process-group behavior through the public No-Visa operation seam.
use jet_runtime::NoVisaOperation;
use pretty_assertions::assert_eq;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

#[tokio::test]
async fn normal_leader_exit_does_not_leave_background_descendants() {
	let mut command = Command::new("sh");
	command.args(["-c", "sleep 60 &"]).stdout(Stdio::piped());
	let mut operation =
		NoVisaOperation::spawn(&mut command, std::future::pending()).unwrap();
	let mut output = operation.take_stdout().unwrap();
	assert!(
		timeout(Duration::from_secs(2), operation.wait())
			.await
			.unwrap()
			.unwrap()
			.success()
	);
	// A surviving background child would keep this inherited pipe open.
	assert_eq!(
		timeout(Duration::from_secs(1), output.read_to_end(&mut Vec::new()))
			.await
			.unwrap()
			.unwrap(),
		0
	);
}

#[tokio::test]
async fn descendants_keep_the_grace_period_when_the_leader_exits_on_term() {
	let mut command = Command::new("sh");
	command.args(["-c", r#"trap 'exit 0' TERM
sh -c 'trap "sleep 0.2; echo cleaned; exit 0" TERM; echo ready; while :; do sleep 1; done' &
wait"#]).stdout(Stdio::piped()).stderr(Stdio::null());
	let (stop, stopped) = tokio::sync::oneshot::channel();
	let mut operation = NoVisaOperation::spawn(&mut command, async {
		let _ = stopped.await;
	})
	.unwrap();
	let mut output = BufReader::new(operation.take_stdout().unwrap()).lines();
	assert_eq!(output.next_line().await.unwrap().as_deref(), Some("ready"));
	stop.send(()).unwrap();
	assert_eq!(
		timeout(Duration::from_secs(4), output.next_line())
			.await
			.unwrap()
			.unwrap()
			.as_deref(),
		Some("cleaned")
	);
	assert!(
		timeout(Duration::from_secs(3), operation.wait())
			.await
			.unwrap()
			.unwrap()
			.success()
	);
}
