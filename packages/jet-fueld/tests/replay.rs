//! Lossless replay and backpressure through a real helper's public protocol.
use jet_protocol::*;
use pretty_assertions::assert_eq;
use std::{
	os::unix::fs::{OpenOptionsExt, PermissionsExt},
	path::Path,
	time::Duration,
};
use tokio::net::{
	UnixStream,
	unix::{OwnedReadHalf, OwnedWriteHalf},
};
use uuid::Uuid;

type Reader = FrameReader<OwnedReadHalf>;
type Writer = FrameWriter<OwnedWriteHalf>;

#[tokio::test]
async fn a_full_run_spool_backpressures_and_replays_every_native_byte() {
	tokio::time::timeout(Duration::from_secs(240), async {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let id = Uuid::new_v4();
        let config = HelperConfig { execution_id: id, working_directory: root.to_str().unwrap().into(), project_directory: root.to_str().unwrap().into(), craft_digest: "test-craft".into(), executables: vec!["/bin/sh".into()] };
        let path = root.join("config.json");
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).unwrap();
        std::io::Write::write_all(&mut file, &encode_control(&config).unwrap()).unwrap();
        let mut helper = tokio::process::Command::new(env!("CARGO_BIN_EXE_jetfueld")).args(["run", "--config"]).arg(path).kill_on_drop(true).spawn().unwrap();
        let socket = root.join("h.sock");
        let (mut reader, mut writer, _) = connect(&socket, id).await;
        send(&mut writer, &HelperCommand::Launch { program: "/bin/sh".into(), arguments: vec!["-c".into(), "/bin/dd if=/dev/zero bs=4096 count=20000 2>/dev/null; printf done > complete".into()], input: String::new(), input_mode: NativeInputMode::Sealed }).await;
        assert!(matches!(receive::<HelperRecord>(&mut reader).await.event, HelperEvent::Started { .. }));
        drop((reader, writer));
        let bound = 64 * 1024 * 1024;
        let fill_started = tokio::time::Instant::now();
        loop {
            let (_, mut writer, ready) = connect(&socket, id).await;
            send(&mut writer, &HelperCommand::Inspect).await;
            assert!(ready.descriptor.replay.produced <= bound);
            assert!(fill_started.elapsed() < Duration::from_secs(90), "spool filling stalled at {:?}", ready.descriptor.replay);
            if ready.descriptor.replay.produced > bound - 20_000 { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!root.join("complete").exists(), "native writes must block at the spool limit");
        // An offset inside a retained record is not permission to discard it.
        let (mut reader, mut writer, _) = connect(&socket, id).await;
        send(&mut writer, &HelperCommand::Recover { source_offset: 1 }).await;
        assert!(reader.read().await.is_err());
        let (mut reader, mut writer, ready) = connect(&socket, id).await;
        assert_eq!(ready.descriptor.replay.acknowledged, 0);
        send(&mut writer, &HelperCommand::Recover { source_offset: 0 }).await;
        eprintln!("spool filled in {:?}", fill_started.elapsed());
        let mut bytes = 0;
        loop {
            let record: HelperRecord = receive(&mut reader).await;
            let ended = match record.event {
                HelperEvent::Output { stream: NativeStream::Stdout, bytes: data } => {
                    assert!(data.iter().all(|byte| *byte == 0));
                    bytes += data.len();
                    false
                }
                HelperEvent::Started { .. } => false,
                HelperEvent::Exited { exit_code } => { assert_eq!(exit_code, Some(0)); true }
                HelperEvent::LaunchFailed | HelperEvent::Output { stream: NativeStream::Stderr, .. } => panic!("unexpected native observation"),
            };
            send(&mut writer, &HelperCommand::Acknowledge { source_offset: record.source_offset }).await;
            if ended { break; }
        }
        assert_eq!(bytes, 81_920_000);
        assert_eq!(std::fs::read_to_string(root.join("complete")).unwrap(), "done");
        assert!(helper.wait().await.unwrap().success());
        assert!(!root.join("descriptor.json").exists());
    }).await.unwrap();
}

async fn connect(socket: &Path, id: Uuid) -> (Reader, Writer, HelperReady) {
	let stream = loop {
		match UnixStream::connect(socket).await {
			Ok(stream) => break stream,
			Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
		}
	};
	let (read, write) = stream.into_split();
	let mut reader = FrameReader::new(read);
	let mut writer = FrameWriter::new(write);
	send(
		&mut writer,
		&HelperHello {
			execution_id: id,
			protocol: ProtocolOffer {
				family: ProtocolFamily::Helper,
				versions: vec![ProtocolVersion { major: 1, minor: 1 }],
				capabilities: vec![],
			},
		},
	)
	.await;
	let ready = receive(&mut reader).await;
	(reader, writer, ready)
}
async fn send(writer: &mut Writer, value: &impl serde::Serialize) {
	writer
		.write(&Frame::control(encode_control(value).unwrap()))
		.await
		.unwrap();
}
async fn receive<T: serde::de::DeserializeOwned>(reader: &mut Reader) -> T {
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!("control")
	};
	decode_control(&payload).unwrap()
}
