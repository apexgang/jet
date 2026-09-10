//! Real reference measurements. Run alone with `just resource-test`.
#[path = "support/resources.rs"]
mod measured;
mod support;
use pretty_assertions::assert_eq;
use std::{
	path::Path,
	time::{Duration, Instant},
};
use tokio::process::Command;
use uuid::Uuid;

#[tokio::test]
#[ignore = "five-minute macOS/Linux reference measurement; just resource-test"]
async fn reference_idle_resource_budgets() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let home = dir.path().join("jet");
	jet_runtime::JetHome::at(home.clone()).prepare().unwrap();
	let store = jet_store::Store::open(&home.join("plane.sqlite3"))
		.await
		.unwrap();
	store
		.write(async |tx| {
			for _ in 0..10_000 {
				tx.insert_conversation(jet_store::NewConversation {
					conversation_id: Uuid::now_v7(),
					retention: jet_store::RetentionPolicy::Retain,
					working_tree: jet_store::WorkingTreeRecord::NoProject,
					origin: jet_store::ConversationOriginRecord::New,
					created_at_unix_ms: 0,
				})
				.await?;
			}
			Ok::<_, jet_store::StoreError>(())
		})
		.await
		.unwrap();
	store.close().await;
	let daemon = support::start_jetd(&home).await;
	let pid = daemon.child.id().unwrap();
	let capabilities = &daemon.ready["capabilities"]["resource_budgets"];
	assert!(
		capabilities.is_object(),
		"structured resource capabilities required"
	);

	let mut crafts = Vec::new();
	for name in ["jet-craft-codex", "jet-craft-claude"] {
		let binary = Path::new(env!("CARGO_BIN_EXE_jetd")).with_file_name(name);
		let bundle = dir.path().join(name);
		std::fs::create_dir_all(bundle.join(".jet")).unwrap();
		std::fs::copy(binary, bundle.join(name)).unwrap();
		let declaration = Path::new(env!("CARGO_MANIFEST_DIR"))
			.join("..")
			.join(name)
			.join(".jet/craft-spec.toml");
		std::fs::copy(declaration, bundle.join(".jet/craft-spec.toml"))
			.unwrap();
		crafts.push(
			Command::new(bundle.join(name))
				.arg("--socket")
				.arg(bundle.join("h.sock"))
				.kill_on_drop(true)
				.spawn()
				.unwrap(),
		);
	}
	let helper_root = dir.path().join("helper");
	let mut helper = measured::Helper::start(&helper_root).await;
	helper.churn().await;
	let processes = [
		("jetd", pid, 35 * 1024),
		("jet-craft-codex", crafts[0].id().unwrap(), 15 * 1024),
		("jet-craft-claude", crafts[1].id().unwrap(), 15 * 1024),
		("jetfueld", helper.child.id().unwrap(), 8 * 1024),
	];
	// Exclude startup and settle the helper after one MiB of acknowledged output.
	tokio::time::sleep(Duration::from_secs(5)).await;
	let mut before = Vec::new();
	for (_, id, _) in processes {
		before.push(sample(id).await);
	}
	let disk_before = disk_bytes(dir.path());
	let started = Instant::now();
	let mut peaks: Vec<_> = before.iter().map(|sample| sample.0).collect();
	let mut after = before.clone();
	for _ in 0..10 {
		tokio::time::sleep(Duration::from_secs(30)).await;
		for (index, (name, id, budget)) in processes.iter().enumerate() {
			after[index] = sample(*id).await;
			peaks[index] = peaks[index].max(after[index].0);
			assert!(
				peaks[index] <= *budget,
				"{name} RSS {} KiB exceeds {budget} KiB",
				peaks[index]
			);
		}
		eprintln!(
			"{}",
			serde_json::json!({"phase":"idle","elapsed_seconds":started.elapsed().as_secs(),"peak_rss_kib":peaks})
		);
	}
	let cpu_percent = after
		.iter()
		.zip(&before)
		.map(|(after, before)| after.1 - before.1)
		.sum::<f64>()
		* 100.0
		/ started.elapsed().as_secs_f64();
	let disk_growth = disk_bytes(dir.path()).saturating_sub(disk_before);
	let children = Command::new("ps")
		.args(["-axo", "ppid="])
		.output()
		.await
		.unwrap();
	assert!(children.status.success());
	let children = String::from_utf8(children.stdout)
		.unwrap()
		.lines()
		.filter(|line| line.trim().parse::<u32>().ok() == Some(pid))
		.count();
	eprintln!(
		"{}",
		serde_json::json!({"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,
		"conversations":10000,"seconds":started.elapsed().as_secs_f64(),"process_order":processes.map(|(name,_,_)|name),"peak_rss_kib":peaks,
		"whole_core_cpu_percent":cpu_percent,"disk_growth_bytes":disk_growth,"unneeded_daemon_children":children,"capabilities":capabilities})
	);
	assert!(
		cpu_percent < 0.2,
		"whole idle core CPU {cpu_percent}% exceeds 0.2%"
	);
	assert_eq!(
		(disk_growth, children),
		(0, 0),
		"idle disk and unneeded helper counts must not grow"
	);
	helper.finish().await;
	assert!(!helper_root.join("descriptor.json").exists());
	for mut craft in crafts {
		craft.kill().await.unwrap();
	}
}

async fn sample(pid: u32) -> (u64, f64) {
	let output = Command::new("ps")
		.args(["-p", &pid.to_string(), "-o", "rss=,time="])
		.output()
		.await
		.unwrap();
	assert!(output.status.success(), "reference process exited");
	let text = String::from_utf8(output.stdout).unwrap();
	let mut fields = text.split_whitespace();
	let rss = fields.next().unwrap().parse().unwrap();
	assert!(rss > 0, "reference process exited or became a zombie");
	let cpu = fields
		.next()
		.unwrap()
		.split(':')
		.fold(0.0, |seconds, field| {
			seconds * 60.0 + field.parse::<f64>().unwrap()
		});
	(rss, cpu)
}

#[tokio::test]
async fn an_unlaunched_helper_stays_within_budget_and_retires() {
	let dir = tempfile::tempdir_in("/tmp").unwrap();
	let root = dir.path();
	let (mut helper, _) = measured::helper(root);
	tokio::time::sleep(Duration::from_secs(1)).await;
	let rss = sample(helper.id().unwrap()).await.0;
	eprintln!(
		"{}",
		serde_json::json!({"helper":"unlaunched","rss_kib":rss})
	);
	let status = tokio::time::timeout(Duration::from_secs(12), helper.wait())
		.await
		.unwrap()
		.unwrap();
	assert!(status.success());
	assert!(!root.join("descriptor.json").exists());
	assert!(rss <= 8 * 1024, "helper RSS {rss} KiB exceeds 8 MiB");
}

fn disk_bytes(path: &Path) -> u64 {
	std::fs::read_dir(path)
		.unwrap()
		.map(|entry| {
			let entry = entry.unwrap();
			let metadata = entry.metadata().unwrap();
			if metadata.is_dir() {
				disk_bytes(&entry.path())
			} else {
				metadata.len()
			}
		})
		.sum()
}
