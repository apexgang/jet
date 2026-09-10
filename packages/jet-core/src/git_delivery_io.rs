//! Bounded Git calls with literal arguments and compare-and-swap publication.
use crate::git_delivery_state::{Document, refused};
use crate::{CoreError, GitOperation};
use std::{
	path::{Path, PathBuf},
	process::Stdio,
	time::Duration,
};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	process::Command,
};

pub(crate) struct RepositoryState {
	pub head: String,
	pub index: String,
	pub branch: Option<String>,
	pub remote_url: Option<String>,
}
pub(crate) fn validate_operation(
	operation: &GitOperation,
) -> Result<(), CoreError> {
	let names: Vec<&str> = match operation {
		GitOperation::Branch { name } => vec![name],
		GitOperation::Commit => vec![],
		GitOperation::Push { remote } => vec![remote],
		GitOperation::DraftPullRequest { remote, base } => {
			std::iter::once(remote.as_str())
				.chain(base.as_deref())
				.collect()
		}
	};
	// ASVS 2.2.1, 1.2.5: bounded literal names cannot become flags or refspecs.
	if names.iter().any(|name| {
		name.is_empty()
			|| name.len() > 240
			|| name.starts_with(['-', '.'])
			|| name.ends_with(['/', '.'])
			|| name.contains("..")
			|| name.contains("@{")
			|| name.contains("//")
			|| name.ends_with(".lock")
			|| name
				.bytes()
				.any(|b| !b.is_ascii_alphanumeric() && !b"/_-.".contains(&b))
	}) {
		return Err(CoreError::invalid_input(
			"git.invalid_name",
			"use a bounded Git branch or remote name",
		));
	}
	Ok(())
}
pub(crate) async fn run(
	command: &mut Command,
	input: &[u8],
	limit: usize,
) -> Result<String, CoreError> {
	command
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.kill_on_drop(true);
	tokio::time::timeout(Duration::from_secs(30), async {
		let mut child = command
			.spawn()
			.map_err(|_| refused("git.tool_unavailable"))?;
		let mut stdin = child
			.stdin
			.take()
			.ok_or_else(|| refused("git.tool_unavailable"))?;
		let stdout = child
			.stdout
			.take()
			.ok_or_else(|| refused("git.tool_unavailable"))?;
		let write = async {
			stdin.write_all(input).await?;
			drop(stdin);
			Ok::<_, std::io::Error>(())
		};
		let read = async {
			let mut bytes = Vec::new();
			stdout
				.take(limit as u64 + 1)
				.read_to_end(&mut bytes)
				.await?;
			Ok::<_, std::io::Error>(bytes)
		};
		let (_, bytes) = tokio::try_join!(write, read)
			.map_err(|_| refused("git.io_failed"))?;
		if bytes.len() > limit {
			return Err(refused("git.output_limit"));
		}
		if !child
			.wait()
			.await
			.map_err(|_| refused("git.io_failed"))?
			.success()
		{
			return Err(refused("git.command_failed"));
		}
		String::from_utf8(bytes)
			.map(|text| text.trim_end().to_owned())
			.map_err(|_| refused("git.output_invalid"))
	})
	.await
	.map_err(|_| refused("git.timeout"))?
}
pub(crate) async fn git(
	root: &Path,
	args: &[&str],
) -> Result<String, CoreError> {
	run(crate::repository::command(root).args(args), &[], 65536).await
}
pub(crate) async fn branch(root: &Path) -> Result<Option<String>, CoreError> {
	let branch = git(root, &["branch", "--show-current"]).await?;
	Ok((!branch.is_empty()).then_some(branch))
}
pub(crate) async fn inspect(
	root: &Path,
	operation: &GitOperation,
) -> Result<RepositoryState, CoreError> {
	if tokio::fs::canonicalize(root)
		.await
		.map_err(|_| refused("git.root_changed"))?
		!= root
		|| crate::repository::verdict(root).await?
			!= crate::repository::Verdict::Registrable
	{
		return Err(refused("git.root_changed"));
	}
	for entry in [
		"MERGE_HEAD",
		"CHERRY_PICK_HEAD",
		"REVERT_HEAD",
		"rebase-merge",
		"rebase-apply",
		"sequencer",
	] {
		let path = git(
			root,
			&["rev-parse", "--path-format=absolute", "--git-path", entry],
		)
		.await?;
		if tokio::fs::try_exists(path)
			.await
			.map_err(|_| refused("git.inspect_failed"))?
		{
			return Err(refused("git.operation_in_progress"));
		}
	}
	let remote_url = match operation {
		GitOperation::Push { remote }
		| GitOperation::DraftPullRequest { remote, .. } => {
			let url =
				git(root, &["remote", "get-url", "--push", "--all", remote])
					.await?;
			if url.lines().count() != 1
				|| url.contains("::")
				|| url.chars().any(char::is_control)
				|| url.starts_with('-')
				|| url.len() > 2048
			{
				return Err(refused("git.remote_invalid"));
			}
			if url.contains("://") {
				let parsed = reqwest::Url::parse(&url)
					.map_err(|_| refused("git.remote_invalid"))?;
				if !matches!(parsed.scheme(), "https" | "ssh" | "git" | "file")
					|| parsed.password().is_some()
					|| parsed.query().is_some()
					|| parsed.fragment().is_some()
					|| (parsed.scheme() == "https"
						&& !parsed.username().is_empty())
				{
					return Err(refused("git.remote_invalid"));
				}
			}
			Some(url)
		}
		GitOperation::Branch { .. } | GitOperation::Commit => None,
	};
	Ok(RepositoryState {
		head: crate::worktree::resolve_commit(root, "HEAD").await?,
		index: git(root, &["write-tree"]).await?,
		branch: branch(root).await?,
		remote_url,
	})
}
pub(crate) async fn prepare_commit(
	doc: &Document,
	title: &str,
	body: &str,
) -> Result<String, CoreError> {
	let tree = doc
		.tree
		.as_deref()
		.ok_or_else(|| refused("git.checkpoint_required"))?;
	if git(&doc.root, &["rev-parse", &format!("{}^{{tree}}", doc.head)]).await?
		== tree
	{
		return Ok(doc.head.clone());
	}
	let message = format!(
		"{title}\n\n{body}\n\nJet-Delivery: {}\n",
		doc.delivery.delivery_id
	);
	// Identity resolution is a real Git capability check; no fabricated author.
	git(&doc.root, &["var", "GIT_AUTHOR_IDENT"]).await?;
	git(&doc.root, &["var", "GIT_COMMITTER_IDENT"]).await?;
	run(
		crate::repository::command(&doc.root).args([
			"commit-tree",
			tree,
			"-p",
			&doc.head,
			"-F",
			"-",
		]),
		message.as_bytes(),
		128,
	)
	.await
}
struct IndexLock {
	path: PathBuf,
	owned: bool,
}
impl Drop for IndexLock {
	fn drop(&mut self) {
		if self.owned {
			let _ = std::fs::remove_file(&self.path);
		}
	}
}
pub(crate) async fn commit(
	doc: &Document,
	home: &crate::WorkspaceHome,
) -> Result<(), CoreError> {
	let commit = doc
		.prepared_commit
		.as_deref()
		.ok_or_else(|| refused("git.commit_not_prepared"))?;
	if commit == doc.head {
		return Ok(());
	}
	let index = PathBuf::from(
		git(
			&doc.root,
			&["rev-parse", "--path-format=absolute", "--git-path", "index"],
		)
		.await?,
	);
	let lock = index.with_extension("lock");
	let mut file = tokio::fs::OpenOptions::new()
		.write(true)
		.create_new(true)
		.open(&lock)
		.await
		.map_err(|_| refused("git.index_locked"))?;
	let mut guard = IndexLock {
		path: lock.clone(),
		owned: true,
	};
	// write-tree itself takes index.lock. Compare against the retained index tree
	// through a read-only command while our lock excludes other Git writers.
	git(
		&doc.root,
		&[
			"diff-index",
			"--cached",
			"--quiet",
			"--no-ext-diff",
			&doc.index,
			"--",
		],
	)
	.await
	.map_err(|_| refused("git.index_changed"))?;
	if branch(&doc.root).await? != doc.branch {
		return Err(refused("git.index_changed"));
	}
	crate::workspace::with_scratch(home, "delivery", async |scratch| {
		let temporary = scratch.join("index");
		// Keep sparse-checkout and index metadata, and stage into the copy only.
		let captured = crate::tree_capture::ScratchIndex::new(
			&doc.root,
			&temporary,
			|_| refused("git.index_failed"),
		);
		captured.copy_from_checkout().await?;
		let (tree, _) = captured.capture_everything(&doc.head).await?;
		if Some(&tree) != doc.tree.as_ref() {
			return Err(refused("git.content_changed"));
		}
		let bytes = tokio::fs::read(&temporary)
			.await
			.map_err(|_| refused("git.index_failed"))?;
		file.write_all(&bytes)
			.await
			.map_err(|_| refused("git.index_failed"))?;
		file.sync_all()
			.await
			.map_err(|_| refused("git.index_failed"))?;
		let reference = doc
			.branch
			.as_ref()
			.map_or_else(|| "HEAD".into(), |b| format!("refs/heads/{b}"));
		// ASVS 2.3.4: only the captured tip can advance. Detached HEAD stays detached.
		git(
			&doc.root,
			&[
				"-c",
				"core.fsync=loose-object,reference",
				"update-ref",
				"--no-deref",
				&reference,
				commit,
				&doc.head,
			],
		)
		.await?;
		tokio::fs::rename(&lock, &index)
			.await
			.map_err(|_| refused("git.index_failed"))?;
		guard.owned = false;
		Ok(())
	})
	.await
}
pub(crate) enum PushMode {
	Preflight,
	Execute,
}
pub(crate) async fn push(
	doc: &Document,
	mode: PushMode,
) -> Result<(), CoreError> {
	let url = doc
		.remote_url
		.as_deref()
		.ok_or_else(|| refused("git.remote_required"))?;
	let branch = doc
		.branch
		.as_deref()
		.ok_or_else(|| refused("git.branch_required"))?;
	let spec = format!("{}:refs/heads/{branch}", doc.head);
	let mut command = crate::repository::command(&doc.root);
	command.args([
		"-c",
		"push.followTags=false",
		"push",
		"--porcelain",
		"--recurse-submodules=no",
	]);
	if matches!(mode, PushMode::Preflight) {
		command.args(["--dry-run", "--no-verify"]);
	}
	run(command.args(["--", url, &spec]), &[], 65536).await?;
	Ok(())
}
pub(crate) async fn pushed(doc: &Document) -> Result<bool, CoreError> {
	let url = doc
		.remote_url
		.as_deref()
		.ok_or_else(|| refused("git.remote_required"))?;
	let reference = format!(
		"refs/heads/{}",
		doc.branch
			.as_deref()
			.ok_or_else(|| refused("git.branch_required"))?
	);
	Ok(
		git(&doc.root, &["ls-remote", "--refs", "--", url, &reference])
			.await?
			.split_whitespace()
			.next() == Some(doc.head.as_str()),
	)
}
