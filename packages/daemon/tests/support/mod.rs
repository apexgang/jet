//! A real `jetd` subprocess over a temporary Jet home.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use jet_client::Client;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use uuid::Uuid;

pub struct Daemon {
	pub child: Child,
	pub socket: PathBuf,
}

pub fn jetd(home: &Path) -> Command {
	let mut command = Command::new(env!("CARGO_BIN_EXE_jetd"));
	command
		.arg("run")
		.arg("--home")
		.arg(home)
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.kill_on_drop(true);
	command
}

pub async fn start_jetd(home: &Path) -> Daemon {
	let mut child = jetd(home).spawn().unwrap();
	let stdout = child.stdout.take().unwrap();
	let mut lines = BufReader::new(stdout).lines();
	let ready = lines.next_line().await.unwrap().expect("jetd exited early");
	let ready: serde_json::Value = serde_json::from_str(&ready).unwrap();
	assert_eq!(ready["event"], "ready");
	Daemon {
		child,
		socket: PathBuf::from(ready["socket"].as_str().unwrap()),
	}
}

pub async fn connect(daemon: &Daemon, client_id: Uuid) -> Client {
	Client::connect_local(&daemon.socket, client_id)
		.await
		.unwrap()
}
