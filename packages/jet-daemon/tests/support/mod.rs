//! A real `jetd` subprocess over a temporary Jet home, plus raw Jet
//! protocol access beside the Rust client.

#![allow(dead_code)]

pub mod pairing;
#[cfg(not(target_os = "macos"))]
pub mod secret_service;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use jet_client::Client;
use jet_protocol::{
	CODEC_JSON_V1, ClientHello, Frame, FrameReader, FrameWriter,
	MAX_CONTROL_FRAME, MAX_DATA_FRAME, PREFACE, PROTOCOL_MINOR, ServerHello,
	SnapshotReason, StreamId, VersionRange, decode_control, encode_control,
};
use pretty_assertions::assert_eq;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use uuid::Uuid;

pub struct Daemon {
	pub child: Child,
	pub socket: PathBuf,
	/// The line `jetd` prints once it can serve, parsed.
	pub ready: serde_json::Value,
	/// The Jet home the daemon serves, as an absolute path.
	home: PathBuf,
	stderr: StderrCapture,
}

/// How many of the daemon's last Diagnostic records a failing test shows.
const DIAGNOSTIC_TAIL: usize = 40;

/// Everything `jetd` writes to stderr, drained as it arrives so a long
/// scenario never fills the pipe and a failure can show what the daemon
/// said (ADR-0061).
struct StderrCapture {
	captured: Arc<Mutex<Vec<u8>>>,
	drain: Option<JoinHandle<()>>,
}

impl StderrCapture {
	fn start(pipe: tokio::process::ChildStderr) -> Self {
		let captured = Arc::new(Mutex::new(Vec::new()));
		let sink = Arc::clone(&captured);
		let drain = tokio::spawn(async move {
			let mut pipe = pipe;
			let mut chunk = [0; 4096];
			while let Ok(read) = pipe.read(&mut chunk).await {
				if read == 0 {
					break;
				}
				sink.lock().unwrap().extend_from_slice(&chunk[..read]);
			}
		});
		Self {
			captured,
			drain: Some(drain),
		}
	}

	fn so_far(&self) -> String {
		String::from_utf8_lossy(&self.captured.lock().unwrap()).into_owned()
	}

	/// Waits for the pipe to close, which the process's exit brings about,
	/// and returns everything written to it.
	async fn drained(&mut self) -> String {
		if let Some(drain) = self.drain.take() {
			drain.await.unwrap();
		}
		self.so_far()
	}
}

impl Daemon {
	/// Everything `jetd` wrote to stderr; waits for the pipe to close, so
	/// call it once the process has exited.
	pub async fn stderr(&mut self) -> String {
		self.stderr.drained().await
	}
}

impl Drop for Daemon {
	/// A failing test shows the daemon's side of the story beside its own
	/// panic: its stderr, then the tail of its Diagnostic log (ADR-0061).
	/// `kill_on_drop` then stops the process.
	fn drop(&mut self) {
		if !std::thread::panicking() {
			return;
		}
		let stderr = self.stderr.so_far();
		if stderr.is_empty() {
			eprintln!(
				"jetd at {} wrote nothing to stderr",
				self.socket.display()
			);
		} else {
			eprintln!("jetd at {} stderr:\n{stderr}", self.socket.display());
		}
		let log = self.home.join("diagnostics").join("jetd.log");
		match std::fs::read_to_string(&log) {
			Ok(records) => {
				let lines: Vec<&str> = records.lines().collect();
				let shown = lines.len().min(DIAGNOSTIC_TAIL);
				eprintln!(
					"jetd Diagnostic log {} (last {shown} of {} records):",
					log.display(),
					lines.len()
				);
				for line in &lines[lines.len() - shown..] {
					eprintln!("{line}");
				}
			}
			Err(error) => {
				eprintln!(
					"jetd Diagnostic log {} cannot be read: {error}",
					log.display()
				);
			}
		}
	}
}

pub fn jetd(home: &Path) -> Command {
	let mut command = Command::new(env!("CARGO_BIN_EXE_jetd"));
	command
		.arg("serve")
		.arg("--home")
		.arg(home)
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.kill_on_drop(true);
	command
}

pub async fn start_jetd(home: &Path) -> Daemon {
	start_jetd_process(&mut jetd(home)).await
}

/// Starts `jetd` where none of the external tools it invokes can be found,
/// so its Capability snapshot reports them missing (ADR-0056). The search
/// path is given to the child alone; this process keeps its own.
pub async fn start_jetd_without_external_tools(home: &Path) -> Daemon {
	start_jetd_process(jetd(home).env("PATH", "/jet-has-no-tools-here")).await
}

/// Starts `jetd` on a Plane whose credential store answers, so a test that
/// binds an account through the platform store runs the same way wherever
/// it runs. On Linux the Plane is given a session bus of the test's own,
/// with an open Secret Service on it; macOS resolves through the Keychain.
#[cfg(not(target_os = "macos"))]
pub async fn start_jetd_with_credential_store(home: &Path) -> Daemon {
	let address = secret_service::serve(&home.with_file_name("bus")).await;
	start_jetd_process(jetd(home).env("DBUS_SESSION_BUS_ADDRESS", address))
		.await
}

/// See the Linux form; the Keychain is part of the operating system.
#[cfg(target_os = "macos")]
pub async fn start_jetd_with_credential_store(home: &Path) -> Daemon {
	start_jetd(home).await
}

/// The `--home` a [`jetd`] command was given, made absolute against the
/// directory the command runs in.
fn home_of(command: &Command) -> PathBuf {
	let command = command.as_std();
	let mut args = command.get_args();
	let home = loop {
		match args.next() {
			Some(flag) if flag == "--home" => {
				break PathBuf::from(args.next().expect("a home after --home"));
			}
			Some(_) => {}
			None => panic!("jetd command has no --home"),
		}
	};
	if home.is_absolute() {
		return home;
	}
	match command.get_current_dir() {
		Some(directory) => directory.join(home),
		None => std::env::current_dir().unwrap().join(home),
	}
}

/// Spawns one `jetd` and waits for the line that says it can serve.
pub async fn start_jetd_process(command: &mut Command) -> Daemon {
	let home = home_of(command);
	let mut child = command.spawn().unwrap();
	let mut stderr = StderrCapture::start(child.stderr.take().unwrap());
	let stdout = child.stdout.take().unwrap();
	let mut lines = BufReader::new(stdout).lines();
	let ready = match lines.next_line().await.unwrap() {
		Some(ready) => ready,
		None => panic!("jetd exited early: {}", stderr.drained().await),
	};
	let ready: serde_json::Value = serde_json::from_str(&ready).unwrap();
	assert_eq!(ready["status"], "ready");
	Daemon {
		child,
		socket: PathBuf::from(ready["socket"].as_str().unwrap()),
		ready,
		home,
		stderr,
	}
}

/// Waits for a Recovery snapshot taken for `reason` and returns its name.
/// The daemon takes the day's copy once it serves, so it lands moments
/// after the ready line or the first change of the day (ADR-0097,
/// ADR-0022).
pub async fn wait_for_snapshot(home: &Path, reason: SnapshotReason) -> String {
	// The reason is a segment of the snapshot's file name.
	let segment = match reason {
		SnapshotReason::Daily => "-daily.",
		SnapshotReason::Maintenance => "-maintenance.",
		SnapshotReason::Migration => "-migration-",
	};
	for _ in 0..500 {
		if let Ok(entries) = std::fs::read_dir(home.join("snapshots")) {
			let mut names: Vec<String> = entries
				.map(|entry| entry.unwrap().file_name().into_string().unwrap())
				.filter(|name| {
					name.starts_with("plane-")
						&& name.ends_with(".sqlite3")
						&& name.contains(segment)
				})
				.collect();
			if let Some(name) = names.pop() {
				return name;
			}
		}
		tokio::time::sleep(std::time::Duration::from_millis(20)).await;
	}
	panic!("no {reason:?} snapshot was taken");
}

pub async fn connect(daemon: &Daemon, client_id: Uuid) -> Client {
	Client::connect_local(&daemon.socket, client_id)
		.await
		.unwrap()
}

/// A hello that speaks exactly what this build of `jetd` speaks.
pub fn hello(client_id: Uuid) -> ClientHello {
	ClientHello {
		protocol: VersionRange { min: 1, max: 1 },
		minor: PROTOCOL_MINOR,
		codec: CODEC_JSON_V1.into(),
		client_id,
		max_control_frame: u32::try_from(MAX_CONTROL_FRAME).unwrap(),
		max_data_frame: u32::try_from(MAX_DATA_FRAME).unwrap(),
		capabilities: vec![],
	}
}

/// One framed connection driven by hand, below the Rust client.
pub struct RawConnection {
	reader: FrameReader<OwnedReadHalf>,
	writer: FrameWriter<OwnedWriteHalf>,
	multiplexed: bool,
}

impl RawConnection {
	pub async fn send_frame(&mut self, frame: Frame) {
		self.writer.write(&frame).await.unwrap();
	}

	pub async fn receive_frame(&mut self) -> Frame {
		self.reader.read().await.unwrap()
	}

	pub async fn send<T: serde::Serialize>(&mut self, message: &T) {
		self.send_bytes(encode_control(message).unwrap()).await;
	}

	pub async fn send_bytes(&mut self, payload: Vec<u8>) {
		let frame = if self.multiplexed {
			Frame::stream_control(StreamId::new(1).unwrap(), payload)
		} else {
			Frame::control(payload)
		};
		self.writer.write(&frame).await.unwrap();
	}

	pub async fn receive<T: serde::de::DeserializeOwned>(&mut self) -> T {
		let Frame::Control { payload: reply, .. } =
			self.reader.read().await.unwrap()
		else {
			panic!("expected a control frame");
		};
		decode_control(&reply).unwrap()
	}

	fn enable_multiplexing(&mut self) {
		self.reader.enable_multiplexing();
		self.writer.enable_multiplexing();
		self.multiplexed = true;
	}
}

/// Connects and sends the preface, leaving the handshake to the caller.
pub async fn open_raw(daemon: &Daemon) -> RawConnection {
	let mut stream = UnixStream::connect(&daemon.socket).await.unwrap();
	stream.write_all(PREFACE).await.unwrap();
	let (read, write) = stream.into_split();
	RawConnection {
		reader: FrameReader::new(read),
		writer: FrameWriter::new(write),
		multiplexed: false,
	}
}

/// Connects, sends `hello`, and returns the daemon's answer to it.
pub async fn handshake_raw(
	daemon: &Daemon,
	hello: &ClientHello,
) -> (RawConnection, ServerHello) {
	let mut connection = open_raw(daemon).await;
	connection.send(hello).await;
	let reply = connection.receive().await;
	if matches!(
		&reply,
		ServerHello::Welcome {
			minor,
			..
		} if *minor >= jet_protocol::MULTIPLEXED_STREAMS_MINOR
	) {
		connection.enable_multiplexing();
	}
	(connection, reply)
}

/// Connects as `client_id` and asserts the daemon welcomed it.
pub async fn connect_raw(daemon: &Daemon, client_id: Uuid) -> RawConnection {
	let (connection, reply) = handshake_raw(daemon, &hello(client_id)).await;
	assert!(
		matches!(reply, ServerHello::Welcome { .. }),
		"expected a welcome, got {reply:?}"
	);
	connection
}

/// Creates an ordinary repository at `dir` with one commit and returns its
/// canonical path. The test host needs `git`, as CI provisions it
/// (ADR-0056).
pub fn init_repository(dir: &Path) -> PathBuf {
	std::fs::create_dir_all(dir).unwrap();
	for args in [
		vec!["init", "-q"],
		vec!["add", "-A"],
		vec!["commit", "-q", "--allow-empty", "-m", "Initial"],
	] {
		let output = std::process::Command::new("git")
			.env("GIT_CONFIG_NOSYSTEM", "1")
			.env("GIT_CONFIG_GLOBAL", "/dev/null")
			.arg("-C")
			.arg(dir)
			.args([
				"-c",
				"user.name=Jet",
				"-c",
				"user.email=jet@example.invalid",
				"-c",
				"commit.gpgsign=false",
			])
			.args(&args)
			.output()
			.unwrap();
		assert!(
			output.status.success(),
			"git {args:?} failed: {}",
			String::from_utf8_lossy(&output.stderr)
		);
	}
	dir.canonicalize().unwrap()
}
