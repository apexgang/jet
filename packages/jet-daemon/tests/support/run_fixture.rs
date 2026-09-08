//! Controlled external peers for the real Run conformance boundary.
#[path = "queue_fixture.rs"]
mod queue_fixture;
use jet_craft_sdk::CraftConnection;
use jet_protocol::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::Path};
use tokio::net::{UnixListener, UnixStream};

pub fn install(home: &Path) {
	install_craft(home, CraftProfile::Standard, ForkCapture::Ignore, "fake");
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkSupport {
	Native,
	Portable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForkCapture {
	Record,
	Ignore,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CraftProfile {
	Standard,
	NativeFork,
	PortableFallback,
	AtMinor(u32),
}

#[allow(dead_code)]
pub fn install_with_fork(home: &Path, support: ForkSupport) {
	let profile = match support {
		ForkSupport::Native => CraftProfile::NativeFork,
		ForkSupport::Portable => CraftProfile::PortableFallback,
	};
	install_craft(home, profile, ForkCapture::Record, "fake");
}

#[allow(dead_code)]
pub fn install_at_minor(home: &Path, craft_minor: u32) {
	install_craft(
		home,
		CraftProfile::AtMinor(craft_minor),
		ForkCapture::Ignore,
		"fake",
	);
}

#[allow(dead_code)]
pub fn install_handoff_target(home: &Path) {
	install_craft(
		home,
		CraftProfile::NativeFork,
		ForkCapture::Record,
		"target",
	);
}

fn install_craft(
	home: &Path,
	profile: CraftProfile,
	capture: ForkCapture,
	identity: &str,
) {
	std::fs::create_dir_all(home.join("crafts")).unwrap();
	let executable = std::env::current_exe().unwrap();
	let program = home.join(format!("crafts/{identity}-craft"));
	let (minor, capabilities, features) = match profile {
		CraftProfile::Standard => (
			5,
			json!(["runs", "resume"]),
			json!([{"name":"turns"},{"name":"resume"}]),
		),
		CraftProfile::NativeFork => (
			5,
			json!(["runs", "resume", "fork"]),
			json!([{"name":"turns"},{"name":"resume"},{"name":"fork"}]),
		),
		CraftProfile::PortableFallback => (
			3,
			json!(["runs", "resume"]),
			json!([{"name":"turns"},{"name":"resume"}]),
		),
		CraftProfile::AtMinor(minor) => (
			minor,
			json!(["runs", "resume"]),
			json!([{"name":"turns"},{"name":"resume"}]),
		),
	};
	let specification = json!({
		"schema":{"major":1,"minor":0},"id":identity,"harness":identity,
		"protocol":{"family":"craft","versions":[{"major":1,"minor":minor}],"capabilities":capabilities},
		"features":features,"broker_permissions":[],
		"host_access":[{"kind":"executable","name":executable},{"kind":"executable","name":"/bin/sh"},{"kind":"executable","name":"/missing-jet-test-harness"}]
	});
	let manifest = home.join(format!("crafts/{identity}.json"));
	let capture = match capture {
		ForkCapture::Record => "export JET_FAKE_CAPTURE_FORK=1\n",
		ForkCapture::Ignore => "",
	};
	let script = format!(
		"#!/bin/sh\nexport JET_FAKE_MANIFEST={}\nexport JET_CRAFT_SOCKET=\"$2\"\n{}exec {} --ignored --exact --nocapture fixture::fake_craft_process\n",
		quote(manifest.to_str().unwrap()),
		capture,
		quote(executable.to_str().unwrap())
	);
	std::fs::write(&program, &script).unwrap();
	std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
		.unwrap();
	let installation = json!({"executable":program.canonicalize().unwrap(),"sha256":format!("{:x}", Sha256::digest(script.as_bytes())),"specification":specification});
	std::fs::write(manifest, installation.to_string()).unwrap();
}
fn quote(text: &str) -> String {
	format!("'{}'", text.replace('\'', "'\\''"))
}

#[tokio::test]
#[ignore = "invoked as a real out-of-process Craft"]
async fn fake_craft_process() {
	let manifest: serde_json::Value = serde_json::from_slice(
		&std::fs::read(std::env::var_os("JET_FAKE_MANIFEST").unwrap()).unwrap(),
	)
	.unwrap();
	let specification: CraftSpecification =
		serde_json::from_value(manifest["specification"].clone()).unwrap();
	use std::io::Write;
	let manifest_path = std::path::PathBuf::from(
		std::env::var_os("JET_FAKE_MANIFEST").unwrap(),
	);
	let mut starts = std::fs::OpenOptions::new()
		.create(true)
		.append(true)
		.open(manifest_path.with_extension("starts"))
		.unwrap();
	writeln!(starts, "{}", std::process::id()).unwrap();

	let listener =
		UnixListener::bind(std::env::var_os("JET_CRAFT_SOCKET").unwrap())
			.unwrap();
	loop {
		let accepted = tokio::time::timeout(
			std::time::Duration::from_millis(100),
			listener.accept(),
		)
		.await;
		let (stream, _) = match accepted {
			Ok(result) => result.unwrap(),
			Err(_) => {
				if !Path::new(&std::env::var_os("JET_FAKE_MANIFEST").unwrap())
					.exists()
				{
					return;
				}
				continue;
			}
		};
		let specification = specification.clone();
		tokio::spawn(async move {
			execution(stream, specification).await;
		});
	}
}

/// A Harness that ignores every signal it is allowed to ignore, so a stop
/// has to escalate all the way to kill. It keeps its file and its output.
const DEAF_HARNESS: &str = r#"
trap 'printf "%s\n" "{\"process_title\":\"Stopping Harness\",\"text\":\"Still stopping\"}"' INT TERM
printf 'Harness work\n' > result.txt
printf '%s\n' '{"text":"Working before the stop"}'
: > deaf-ready
while true; do sleep 1; done
"#;

fn launch(text: &str, root: &Path) -> HelperCommand {
	if text == "Fail native launch" {
		return HelperCommand::Launch {
			program: "/missing-jet-test-harness".into(),
			arguments: vec![],
			input: format!("{text}\n"),
			input_mode: NativeInputMode::Sealed,
		};
	}
	if root.join("deaf").exists() {
		return HelperCommand::Launch {
			program: "/bin/sh".into(),
			arguments: vec!["-c".into(), DEAF_HARNESS.into()],
			input: format!("{text}\n"),
			input_mode: NativeInputMode::Sealed,
		};
	}
	HelperCommand::Launch {
		program: std::env::current_exe().unwrap().to_str().unwrap().into(),
		arguments: vec![
			"--ignored".into(),
			"--exact".into(),
			"--nocapture".into(),
			"fixture::fake_harness_process".into(),
		],
		input: format!("{text}\n"),
		input_mode: NativeInputMode::Sealed,
	}
}

async fn execution(stream: UnixStream, specification: CraftSpecification) {
	let (read, write) = stream.into_split();
	let connection = CraftConnection::accept(read, write, specification)
		.await
		.unwrap();
	let execution_id = connection.hello().execution_id;
	let craft_minor = connection.negotiated().version.minor;
	let resume = connection.hello().resume.clone();
	let fork = connection.hello().fork.clone();
	let (mut receiver, mut sender) = connection.split();
	let (id, text, helper_socket, source_offset, checkpoint) =
		match receiver.receive().await.unwrap() {
			CraftCommand::Start {
				id,
				text,
				helper_socket,
			} => (id, Some(text), helper_socket, 0, String::new()),
			CraftCommand::Recover {
				id,
				helper_socket,
				source_offset,
				checkpoint,
			} => {
				let manifest = std::path::PathBuf::from(
					std::env::var_os("JET_FAKE_MANIFEST").unwrap(),
				);
				assert!(
					!manifest.with_extension("crash").exists(),
					"injected recovery crash"
				);
				(id, None, helper_socket, source_offset, checkpoint)
			}
			_ => panic!("expected Start or Recover"),
		};
	let helper = UnixStream::connect(helper_socket).await.unwrap();
	let (read, write) = helper.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	writer
		.write(&Frame::control(
			encode_control(&HelperHello {
				execution_id,
				protocol: ProtocolOffer {
					family: ProtocolFamily::Helper,
					versions: vec![ProtocolVersion { major: 1, minor: 1 }],
					capabilities: vec![],
				},
			})
			.unwrap(),
		))
		.await
		.unwrap();
	let ready: HelperReady = receive(&mut reader).await;
	let queued_execution =
		Path::new(&ready.descriptor.config.working_directory)
			.join("queue")
			.exists();
	let recovering = text.is_none();
	if !recovering {
		let root = Path::new(&ready.descriptor.config.working_directory);
		std::fs::write(
			root.join("native-resume"),
			serde_json::to_string(&resume).unwrap(),
		)
		.unwrap();
		if std::env::var_os("JET_FAKE_CAPTURE_FORK").is_some() {
			std::fs::write(
				root.join("native-fork"),
				serde_json::to_string(&fork).unwrap(),
			)
			.unwrap();
			std::fs::write(
				root.join("initial-input"),
				format!("{}\n", text.as_deref().unwrap()),
			)
			.unwrap();
		}
	}
	writer
		.write(&Frame::control(
			encode_control(&if let Some(text) = text {
				launch(
					&text,
					Path::new(&ready.descriptor.config.working_directory),
				)
			} else {
				HelperCommand::Recover { source_offset }
			})
			.unwrap(),
		))
		.await
		.unwrap();
	if recovering {
		sender
			.send(&CraftEvent::RunRecovered {
				helper_pid: ready.helper_pid,
				source_offset,
			})
			.await
			.unwrap();
	}
	let mut pending: Vec<u8> = if checkpoint.is_empty() {
		vec![]
	} else {
		serde_json::from_str(&checkpoint).unwrap()
	};
	let mut harness_pid = None;
	// A bounded reader task keeps partial Craft frames alive across select!.
	let (commands, mut requests) = tokio::sync::mpsc::channel(8);
	tokio::spawn(async move {
		while let Ok(command) = receiver.receive().await {
			if commands.send(command).await.is_err() {
				break;
			}
		}
	});
	loop {
		let incoming = receive::<HelperRecord>(&mut reader);
		tokio::pin!(incoming);
		let record = loop {
			tokio::select! {
				record = &mut incoming => break record,
				command = requests.recv() => {
					let root = Path::new(&ready.descriptor.config.working_directory);
					match command {
						Some(CraftCommand::Turn { id, text }) => {
							assert!(root.join("queue").exists());
							std::fs::write(root.join("turn-input.tmp"), json!({"id":id,"text":text}).to_string()).unwrap();
							std::fs::rename(root.join("turn-input.tmp"), root.join("turn-input")).unwrap();
						}
						// Native cancellation: the Harness is told to abandon
						// this turn, and reports the boundary itself. The Run's
						// own identity names the input it started with.
						Some(CraftCommand::Interrupt { id: cancelled }) => {
							let turn = if cancelled == id { "initial".to_string() } else { cancelled };
							std::fs::write(root.join(format!("interrupted-{turn}")), "cancel").unwrap();
						}
						_ => return,
					}
				}
			}
		};
		let ended = matches!(
			record.event,
			HelperEvent::Exited { .. } | HelperEvent::LaunchFailed
		);
		let mut interpreted = 0;
		match record.event {
			HelperEvent::LaunchFailed => {
				sender.send(&CraftEvent::RunLaunchFailed).await.unwrap()
			}
			HelperEvent::Started {
				harness_pid: started_pid,
			} => {
				harness_pid = Some(started_pid);
				sender
					.send(&CraftEvent::RunStarted {
						helper_pid: ready.helper_pid,
						harness_pid: started_pid,
					})
					.await
					.unwrap();
			}
			HelperEvent::Output {
				stream: NativeStream::Stdout,
				bytes,
			} => {
				pending.extend(bytes);
				while let Some(end) = pending.iter().position(|b| *b == b'\n') {
					let line: Vec<u8> = pending.drain(..=end).collect();
					let Ok(native_event) = serde_json::from_slice::<
						Box<serde_json::value::RawValue>,
					>(&line) else {
						continue;
					};
					let native: serde_json::Value =
						serde_json::from_str(native_event.get()).unwrap();
					if native["interrupted_turn"].is_string() {
						sender
							.send(&CraftEvent::TurnEnded {
								outcome: TurnOutcome::Interrupted,
							})
							.await
							.unwrap();
					}
					if craft_minor >= 5 {
						if let Some(title) =
							native["conversation_title"].as_str()
						{
							sender
								.send(&CraftEvent::ConversationTitle {
									title: title.into(),
								})
								.await
								.unwrap();
						}
						if let Some(title) = native["run_title"].as_str() {
							sender
								.send(&CraftEvent::RunTitle {
									title: title.into(),
								})
								.await
								.unwrap();
						}
						if let Some(title) = native["process_title"].as_str() {
							sender
								.send(&CraftEvent::ProcessTitle {
									pid: harness_pid.unwrap(),
									title: title.into(),
								})
								.await
								.unwrap();
						}
					}
					if let Some(turn_id) = native["turn_id"].as_str() {
						sender
							.send(&CraftEvent::Completed {
								id: if turn_id == "initial" {
									id.clone()
								} else {
									turn_id.into()
								},
								native_conversation: "fake-native-1".into(),
							})
							.await
							.unwrap();
					}
					sender
						.send(&CraftEvent::Output {
							native_event,
							presentation: vec![
								PresentationBlock::new(&Presentation::Text {
									text: native["text"]
										.as_str()
										.unwrap_or("Waiting")
										.into(),
								})
								.unwrap(),
							],
						})
						.await
						.unwrap();
					interpreted += 1;
					let interrupted = std::path::PathBuf::from(
						std::env::var_os("JET_FAKE_MANIFEST").unwrap(),
					)
					.with_extension("mid-batch");
					if interpreted == 70 && interrupted.exists() {
						std::fs::remove_file(interrupted).unwrap();
						tokio::time::sleep(std::time::Duration::from_millis(
							100,
						))
						.await;
						panic!(
							"injected crash inside dense source record after durable prefix"
						);
					}
					if let Some(change) = native.get("file_change")
						&& craft_minor >= 3
					{
						sender
							.send(&CraftEvent::FileChanged {
								change: serde_json::from_value(change.clone())
									.unwrap(),
							})
							.await
							.unwrap();
					}
					if native["phase"] == "waiting" {
						for activity in [
							RunActivity::WaitingForUser,
							RunActivity::WaitingForAuth,
							RunActivity::WaitingForQuota,
							RunActivity::Reconnecting,
							RunActivity::WaitingForApproval,
						] {
							sender
								.send(&CraftEvent::Activity { activity })
								.await
								.unwrap();
						}
					}
					if native["phase"] == "auth" {
						sender
							.send(&CraftEvent::Activity {
								activity: RunActivity::WaitingForAuth,
							})
							.await
							.unwrap();
					}
				}
			}
			HelperEvent::Output {
				stream: NativeStream::Stderr,
				..
			} => {}
			HelperEvent::Exited { exit_code } => {
				if !queued_execution {
					sender
						.send(&CraftEvent::Completed {
							id: id.clone(),
							native_conversation: "fake-native-1".into(),
						})
						.await
						.unwrap();
				}
				sender
					.send(&CraftEvent::RunEnded { exit_code })
					.await
					.unwrap();
			}
		}
		let stalled = std::path::PathBuf::from(
			std::env::var_os("JET_FAKE_MANIFEST").unwrap(),
		)
		.with_extension("stall");
		if stalled.exists() {
			tokio::time::sleep(std::time::Duration::from_secs(60)).await;
		}
		sender
			.send(&CraftEvent::Progress {
				source_offset: record.source_offset,
				checkpoint: serde_json::to_string(&pending).unwrap(),
			})
			.await
			.unwrap();
		let CraftCommand::Acknowledge { source_offset } =
			requests.recv().await.unwrap()
		else {
			panic!("Ack")
		};
		let dropped = std::path::PathBuf::from(
			std::env::var_os("JET_FAKE_MANIFEST").unwrap(),
		)
		.with_extension("drop-ack");
		if dropped.exists() {
			std::fs::remove_file(dropped).unwrap();
			panic!("injected loss after commit before helper acknowledgement");
		}
		writer
			.write(&Frame::control(
				encode_control(&HelperCommand::Acknowledge { source_offset })
					.unwrap(),
			))
			.await
			.unwrap();
		if pending == b"{\"text\":\"par" {
			std::fs::write(
				Path::new(&ready.descriptor.config.working_directory)
					.join("parser-acknowledged"),
				"durable",
			)
			.unwrap();
		}
		if ended {
			break;
		}
	}
	assert!(matches!(
		requests.recv().await.unwrap(),
		CraftCommand::Shutdown
	));
}

async fn receive<T: serde::de::DeserializeOwned>(
	reader: &mut FrameReader<tokio::net::unix::OwnedReadHalf>,
) -> T {
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!("Control")
	};
	decode_control(&payload).unwrap()
}

#[test]
#[ignore = "invoked as a real native Harness owned by jetfueld"]
fn fake_harness_process() {
	use std::io::{Read, Write};
	let mut input = String::new();
	std::io::stdin().read_to_string(&mut input).unwrap();
	if input.trim() == "Fail after spawn" {
		std::process::exit(7);
	}
	assert!(
		input.starts_with("<jet-handoff-context")
			|| input.trim() == "Make a change"
			|| input.trim() == "Continue from checkpoint"
			|| (input.starts_with(
				"<jet-fork-context version=\"1\" data-only=\"true\">\n"
			) && input.trim().ends_with("Continue from checkpoint"))
	);
	if Path::new("queue").exists() {
		return queue_fixture::harness();
	}
	if Path::new("auth-wait").exists() {
		println!("{}", json!({"phase":"auth"}));
		std::io::stdout().flush().unwrap();
		while !Path::new("authenticated").exists() {
			std::thread::sleep(std::time::Duration::from_millis(10));
		}
	}
	let before = if Path::new("result.txt").exists() {
		file_object("result.txt")
	} else {
		"0".repeat(40)
	};
	let before_mode = if Path::new("result.txt").exists() {
		"100644"
	} else {
		"000000"
	};
	std::fs::write("result.txt", "Harness work\n").unwrap();
	let file_change = json!({"activity_id":"result-write", "path":"result.txt", "before_object":before, "after_object":file_object("result.txt"), "before_mode":before_mode, "after_mode":"100644"});
	if Path::new("name-events").exists() {
		println!(
			"{}",
			json!({
				"conversation_title":"Harness Conversation",
				"run_title":"Harness Run",
				"process_title":"Harness Process",
				"text":"Named"
			})
		);
	}
	if Path::new("dense").exists() {
		let source = (0..100)
			.map(|i| format!("{}\n", json!({"dense":i})))
			.collect::<String>();
		std::io::stdout().write_all(source.as_bytes()).unwrap();
	}
	for _ in 0..40 {
		println!("{}", json!({"text":"x".repeat(8192)}));
	}
	println!("{}", json!({"phase":"waiting", "file_change":file_change}));
	std::io::stdout().flush().unwrap();
	if Path::new("partial").exists() {
		print!("{{\"text\":\"par");
		std::io::stdout().flush().unwrap();
	}
	while !Path::new("continue").exists() {
		std::thread::sleep(std::time::Duration::from_millis(10));
	}
	if Path::new("partial").exists() {
		println!("tial\"}}");
	}
	if Path::new("name-events").exists() {
		println!(
			"{}",
			json!({
				"conversation_title":"Late Harness Conversation",
				"run_title":"Late Harness Run",
				"process_title":"Late Harness Process",
				"text":"Renamed"
			})
		);
	}

	println!(
		"{{ \"text\": \"Finished\", \"native_integer\": 9007199254740993 }}"
	);
	std::io::stdout().flush().unwrap();
}

fn file_object(path: &str) -> String {
	let output = std::process::Command::new("git")
		.args(["hash-object", "--", path])
		.output()
		.unwrap();
	assert!(output.status.success());
	String::from_utf8(output.stdout).unwrap().trim().into()
}
