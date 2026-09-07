//! Controlled external peers for the real Run conformance boundary.
use jet_craft_sdk::CraftConnection;
use jet_protocol::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::Path};
use tokio::net::{UnixListener, UnixStream};

pub fn install(home: &Path) {
	std::fs::create_dir_all(home.join("crafts")).unwrap();
	let executable = std::env::current_exe().unwrap();
	let program = home.join("crafts/fake-craft");
	let specification = json!({
		"schema":{"major":1,"minor":0},"id":"fake","harness":"fake",
		"protocol":{"family":"craft","versions":[{"major":1,"minor":2}],"capabilities":["runs"]},
		"features":[{"name":"turns"}],"broker_permissions":[],
		"host_access":[{"kind":"executable","name":executable},{"kind":"executable","name":"/missing-jet-test-harness"}]
	});
	let manifest = home.join("crafts/fake.json");
	let script = format!(
		"#!/bin/sh\nexport JET_FAKE_MANIFEST={}\nexport JET_CRAFT_SOCKET=\"$2\"\nexec {} --ignored --exact --nocapture fixture::fake_craft_process\n",
		quote(manifest.to_str().unwrap()),
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

async fn execution(stream: UnixStream, specification: CraftSpecification) {
	let (read, write) = stream.into_split();
	let connection = CraftConnection::accept(read, write, specification)
		.await
		.unwrap();
	let execution_id = connection.hello().execution_id;
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
	let recovering = text.is_none();
	writer
		.write(&Frame::control(
			encode_control(&if let Some(text) = text {
				HelperCommand::Launch {
					program: if text == "Fail native launch" {
						"/missing-jet-test-harness".into()
					} else {
						std::env::current_exe()
							.unwrap()
							.to_str()
							.unwrap()
							.into()
					},
					arguments: vec![
						"--ignored".into(),
						"--exact".into(),
						"--nocapture".into(),
						"fixture::fake_harness_process".into(),
					],
					input: format!("{text}\n"),
				}
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
	loop {
		let record: HelperRecord = receive(&mut reader).await;
		let ended = matches!(
			record.event,
			HelperEvent::Exited { .. } | HelperEvent::LaunchFailed
		);
		let mut interpreted = 0;
		match record.event {
			HelperEvent::LaunchFailed => {
				sender.send(&CraftEvent::RunLaunchFailed).await.unwrap()
			}
			HelperEvent::Started { harness_pid } => sender
				.send(&CraftEvent::RunStarted {
					helper_pid: ready.helper_pid,
					harness_pid,
				})
				.await
				.unwrap(),
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
				}
			}
			HelperEvent::Output {
				stream: NativeStream::Stderr,
				..
			} => {}
			HelperEvent::Exited { exit_code } => {
				sender
					.send(&CraftEvent::Completed {
						id: id.clone(),
						native_conversation: "fake-native-1".into(),
					})
					.await
					.unwrap();
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
			receiver.receive().await.unwrap()
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
		receiver.receive().await.unwrap(),
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
	assert_eq!(input.trim(), "Make a change");
	std::fs::write("result.txt", "Harness work\n").unwrap();
	if Path::new("dense").exists() {
		let source = (0..100)
			.map(|i| format!("{}\n", json!({"dense":i})))
			.collect::<String>();
		std::io::stdout().write_all(source.as_bytes()).unwrap();
	}
	for _ in 0..40 {
		println!("{}", json!({"text":"x".repeat(8192)}));
	}
	println!("{{\"phase\":\"waiting\"}}");
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

	println!(
		"{{ \"text\": \"Finished\", \"native_integer\": 9007199254740993 }}"
	);
	std::io::stdout().flush().unwrap();
}
