//! Resource measurements drive the real helper protocol, including source ACKs.
use jet_protocol::*;
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
use tokio::{
	net::{
		UnixStream,
		unix::{OwnedReadHalf, OwnedWriteHalf},
	},
	process::{Child, Command},
};
use uuid::Uuid;

pub fn helper(root: &Path) -> (Child, Uuid) {
	std::fs::create_dir_all(root).unwrap();
	let root = root.canonicalize().unwrap();
	std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
		.unwrap();
	let id = Uuid::new_v4();
	let config = root.join("config.json");
	std::fs::write(
		&config,
		encode_control(&HelperConfig {
			execution_id: id,
			working_directory: root.to_str().unwrap().into(),
			project_directory: root.to_str().unwrap().into(),
			executables: vec!["/bin/cat".into()],
			craft_digest: "a".repeat(64),
		})
		.unwrap(),
	)
	.unwrap();
	std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600))
		.unwrap();
	let binary =
		Path::new(env!("CARGO_BIN_EXE_jetd")).with_file_name("jetfueld");
	(
		Command::new(binary)
			.arg("run")
			.arg("--config")
			.arg(config)
			.kill_on_drop(true)
			.spawn()
			.unwrap(),
		id,
	)
}

pub struct Helper {
	pub child: Child,
	reader: FrameReader<OwnedReadHalf>,
	writer: FrameWriter<OwnedWriteHalf>,
}
impl Helper {
	pub async fn start(root: &Path) -> Self {
		let (child, id) = helper(root);
		let stream = tokio::time::timeout(Duration::from_secs(5), async {
			loop {
				if let Ok(stream) =
					UnixStream::connect(root.join("h.sock")).await
				{
					break stream;
				}
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
		let (read, write) = stream.into_split();
		let mut helper = Self {
			child,
			reader: FrameReader::new(read),
			writer: FrameWriter::new(write),
		};
		helper
			.send(&HelperHello {
				execution_id: id,
				protocol: ProtocolOffer {
					family: ProtocolFamily::Helper,
					versions: vec![ProtocolVersion { major: 1, minor: 3 }],
					capabilities: vec![],
				},
			})
			.await;
		let _: HelperReady = helper.receive().await;
		helper
			.send(&HelperCommand::Launch {
				program: "/bin/cat".into(),
				arguments: vec![],
				input: String::new(),
				input_mode: NativeInputMode::Streaming,
			})
			.await;
		let started: HelperRecord = helper.receive().await;
		assert!(matches!(started.event, HelperEvent::Started { .. }));
		helper.acknowledge(started.source_offset).await;
		helper
	}
	// Exercise repeated native input, output, spool writes, and acknowledgements
	// before measuring settled memory and idle disk growth.
	pub async fn churn(&mut self) {
		for _ in 0..32 {
			self.send(&HelperCommand::Input {
				text: "x".repeat(32 * 1024),
			})
			.await;
			let mut bytes = 0;
			while bytes < 32 * 1024 {
				let record: HelperRecord = self.receive().await;
				let HelperEvent::Output { bytes: data, .. } = record.event
				else {
					panic!("output")
				};
				assert!(data.iter().all(|byte| *byte == b'x'));
				bytes += data.len();
				self.acknowledge(record.source_offset).await;
			}
		}
	}
	pub async fn finish(mut self) {
		self.send(&HelperCommand::CloseInput).await;
		let record: HelperRecord = self.receive().await;
		assert!(matches!(
			record.event,
			HelperEvent::Exited { exit_code: Some(0) }
		));
		self.acknowledge(record.source_offset).await;
		assert!(
			tokio::time::timeout(Duration::from_secs(5), self.child.wait())
				.await
				.unwrap()
				.unwrap()
				.success()
		);
	}
	async fn acknowledge(&mut self, source_offset: u64) {
		self.send(&HelperCommand::Acknowledge { source_offset })
			.await;
	}
	async fn send(&mut self, value: &impl serde::Serialize) {
		self.writer
			.write(&Frame::control(encode_control(value).unwrap()))
			.await
			.unwrap();
	}
	async fn receive<T: serde::de::DeserializeOwned>(&mut self) -> T {
		let Frame::Control { payload, .. } =
			tokio::time::timeout(Duration::from_secs(5), self.reader.read())
				.await
				.unwrap()
				.unwrap()
		else {
			panic!("control")
		};
		decode_control(&payload).unwrap()
	}
}
