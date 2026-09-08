//! Destination policy and native operation behavior through authenticated wire traffic.
use super::*;
use pretty_assertions::assert_eq;

async fn exchange(
	reader: &mut FrameReader<tokio::process::ChildStdout>,
	writer: &mut FrameWriter<tokio::process::ChildStdin>,
	stream: u32,
	request: &Value,
) -> Value {
	writer
		.write(&Frame::stream_control(
			StreamId::new(stream).unwrap(),
			encode_control(request).unwrap(),
		))
		.await
		.unwrap();
	let Frame::Control { payload, .. } = reader.read().await.unwrap() else {
		panic!("expected result")
	};
	decode_control(&payload).unwrap()
}

#[tokio::test]
async fn declared_tools_enforce_roots_and_replay_identity_before_effects() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let daemon = support::start_jetd(&dir.path().join("jet")).await;
	let owner = support::connect(&daemon, Uuid::new_v4()).await;
	let (_, workspace) =
		run_tests::workspace(&owner, &dir.path().join("repo")).await;
	let outside = dir.path().join("outside");
	std::fs::write(&outside, "private outside content").unwrap();
	std::os::unix::fs::symlink(
		&outside,
		std::path::Path::new(&workspace.root).join("escape"),
	)
	.unwrap();
	let (_bridge, mut reader, mut writer, _) = paired_wire(&daemon).await;
	let base = json!({"kind":"remote_tool","id":1,"request":{
		"operation_id":Uuid::now_v7(),"origin":{"plane_id":Uuid::new_v4(),"conversation_id":Uuid::new_v4(),"run_id":Uuid::new_v4()},
		"destination_plane_id":owner.status().await.unwrap().plane_id,"workspace_id":workspace.workspace_id,
		"permissions":["remote_tools"],"action":{"type":"read_file","path":"escape"}
	}});
	let cases = [
		("path", json!("../outside"), "path.parent_traversal"),
		("path", json!(outside), "path.absolute"),
		("path", json!("escape"), "path.escapes_root"),
	];
	let mut stream = 1;
	for (field, value, code) in cases {
		let mut request = base.clone();
		request["request"]["operation_id"] = json!(Uuid::now_v7());
		request["request"]["action"][field] = value;
		assert_eq!(
			exchange(&mut reader, &mut writer, stream, &request).await["error"]
				["code"],
			code
		);
		stream += 1;
	}
	let mut request = base.clone();
	request["request"]["permissions"] = json!([]);
	assert_eq!(
		exchange(&mut reader, &mut writer, stream, &request).await["error"]["code"],
		"remote.permission_denied"
	);
	stream += 1;
	request["request"]["permissions"] = json!(["remote_tools"]);
	request["request"]["action"] =
		json!({"type":"write_file","path":"result.txt","content":"first"});
	assert_eq!(
		exchange(&mut reader, &mut writer, stream, &request).await["result"],
		json!({"type":"written"})
	);
	stream += 1;
	request["request"]["action"]["content"] = json!("changed intent");
	assert_eq!(
		exchange(&mut reader, &mut writer, stream, &request).await["error"]["code"],
		"remote.identity_reused"
	);
	assert_eq!(
		std::fs::read_to_string(
			std::path::Path::new(&workspace.root).join("result.txt")
		)
		.unwrap(),
		"first"
	);
}

#[tokio::test]
async fn reviewed_processes_preserve_literal_arguments_and_terminals_use_a_real_pty()
 {
	tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let daemon = support::start_jetd(&dir.path().join("jet")).await;
        let owner_id = Uuid::new_v4(); let owner = support::connect(&daemon, owner_id).await;
        let (_, workspace) = run_tests::workspace(&owner, &dir.path().join("repo")).await;
        let (_bridge, mut reader, mut writer, client_id) = paired_wire(&daemon).await;
        let mut review = support::connect_raw(&daemon, owner_id).await;
        let mut stream = 1;
        for action in [
            json!({"type":"process","arguments":["/usr/bin/printf","%s","literal; $(touch injected)"],"directory":"","environment":[]}),
            json!({"type":"terminal","input":"test -t 0 && printf 'PTY_CONFIRMED\\n'","directory":"","rows":24,"columns":80}),
        ] {
            let operation_id = Uuid::now_v7();
            let terminal = action["type"] == "terminal";
            let request = json!({"kind":"remote_tool","id":1,"request":{
                "operation_id":operation_id,"origin":{"plane_id":Uuid::new_v4(),"conversation_id":Uuid::new_v4(),"run_id":Uuid::new_v4()},
                "destination_plane_id":owner.status().await.unwrap().plane_id,"workspace_id":workspace.workspace_id,"permissions":["remote_tools"],"action":action
            }});
            assert_eq!(exchange(&mut reader, &mut writer, stream, &request).await["result"], json!({"type":"approval_required","operation_id":operation_id})); stream += 1;
            review.send(&json!({"kind":"query","id":2,"query":{"type":"remote_tool_review","client_id":client_id,"operation_id":operation_id}})).await;
            let exact:Value = review.receive().await;
            assert_eq!(exact["result"]["action"], action);
            review.send(&json!({"kind":"command","id":3,"command_id":Uuid::now_v7(),"command":{"type":"review_remote_tool","client_id":client_id,"operation_id":operation_id,"decision":"allow_once"}})).await;
            assert_eq!(review.receive::<Value>().await["result"]["type"], "remote_tool_reviewed");
            let result = exchange(&mut reader, &mut writer, stream, &request).await; stream += 1;
            if terminal {
                assert_eq!(result["result"]["exit_code"], 0, "{result}");
                assert!(result["result"]["stdout"].as_str().unwrap().contains("PTY_CONFIRMED\r\n"), "{result}");
            } else {
                assert_eq!(result["result"], json!({"type":"process","exit_code":0,"stdout":"literal; $(touch injected)","stderr":""}));
            }
        }
        assert!(!std::path::Path::new(&workspace.root).join("injected").exists());
        let request = json!({"kind":"remote_tool","id":1,"request":{
            "operation_id":Uuid::now_v7(),"origin":{"plane_id":Uuid::new_v4(),"conversation_id":Uuid::new_v4(),"run_id":Uuid::new_v4()},
            "destination_plane_id":owner.status().await.unwrap().plane_id,"workspace_id":workspace.workspace_id,"permissions":["remote_tools"],"action":{"type":"git","operation":"status"}
        }});
        assert_eq!(exchange(&mut reader, &mut writer, stream, &request).await["result"], json!({"type":"process","exit_code":0,"stdout":"","stderr":""}));
    }).await.unwrap();
}

#[tokio::test]
async fn git_ignores_repository_worktree_redirection() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let daemon = support::start_jetd(&dir.path().join("jet")).await;
	let owner = support::connect(&daemon, Uuid::new_v4()).await;
	let (_, workspace) =
		run_tests::workspace(&owner, &dir.path().join("repo")).await;
	let outside = dir.path().join("outside");
	std::fs::create_dir(&outside).unwrap();
	std::fs::write(
		outside.join("secret.txt"),
		"must not enter the selected Workspace result",
	)
	.unwrap();
	assert!(
		std::process::Command::new("git")
			.arg("-C")
			.arg(&workspace.root)
			.args(["config", "core.worktree"])
			.arg(&outside)
			.status()
			.unwrap()
			.success()
	);
	let (_bridge, mut reader, mut writer, _) = paired_wire(&daemon).await;
	for (index, operation) in ["status", "diff"].iter().enumerate() {
		let request = json!({"kind":"remote_tool","id":1,"request":{
			"operation_id":Uuid::now_v7(),"origin":{"plane_id":Uuid::new_v4(),"conversation_id":Uuid::new_v4(),"run_id":Uuid::new_v4()},
			"destination_plane_id":owner.status().await.unwrap().plane_id,"workspace_id":workspace.workspace_id,"permissions":["remote_tools"],"action":{"type":"git","operation":operation}
		}});
		assert_eq!(
			exchange(
				&mut reader,
				&mut writer,
				u32::try_from(index + 1).unwrap(),
				&request
			)
			.await["result"],
			json!({"type":"process","exit_code":0,"stdout":"","stderr":""})
		);
	}
}
