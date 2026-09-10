//! GitHub-only draft delivery; credentials come exclusively from the platform store.
use crate::{
	CoreError, GitHubRequest, GitOperation,
	git_delivery::{
		io,
		state::{Document, refused},
	},
};
use serde_json::Value;

fn repository(doc: &Document) -> Result<String, CoreError> {
	let url = doc
		.remote_url
		.as_deref()
		.ok_or_else(|| refused("git.remote_required"))?;
	let path = url
		.strip_prefix("https://github.com/")
		.or_else(|| url.strip_prefix("git@github.com:"))
		.or_else(|| url.strip_prefix("ssh://git@github.com/"))
		.ok_or_else(|| refused("git.github_required"))?;
	let path = path.strip_suffix(".git").unwrap_or(path);
	let parts: Vec<_> = path.split('/').collect();
	if parts.len() != 2
		|| parts.iter().any(|s| {
			s.is_empty()
				|| *s == "." || *s == ".."
				|| s.bytes()
					.any(|b| !b.is_ascii_alphanumeric() && !b"_.-".contains(&b))
		}) {
		return Err(refused("git.github_required"));
	}
	Ok(path.into())
}
struct GitHub<'a> {
	host: &'a dyn crate::GitHubHost,
	repository: String,
}
impl GitHub<'_> {
	async fn request(
		&self,
		request: crate::GitHubRequest,
	) -> Result<Value, CoreError> {
		let bytes = tokio::time::timeout(
			std::time::Duration::from_secs(30),
			self.host.request(&self.repository, &request),
		)
		.await
		.map_err(|_| refused("git.github_timeout"))??;
		if bytes.len() > 1_048_576 {
			return Err(refused("git.github_output_limit"));
		}
		serde_json::from_slice(&bytes)
			.map_err(|_| refused("git.github_invalid"))
	}
	async fn existing(
		&self,
		doc: &Document,
	) -> Result<Option<Value>, CoreError> {
		if let Some(number) = doc.draft_number {
			return self
				.request(GitHubRequest::ReadDraft { number })
				.await
				.map(Some);
		}
		let branch = doc
			.branch
			.as_deref()
			.ok_or_else(|| refused("git.branch_required"))?;
		let owner = self
			.repository
			.split('/')
			.next()
			.ok_or_else(|| refused("git.github_invalid"))?;
		let head = format!("{owner}:{branch}");
		let pulls = self.request(GitHubRequest::FindDrafts { head }).await?;
		let pulls = pulls
			.as_array()
			.ok_or_else(|| refused("git.github_invalid"))?;
		if pulls.len() == 100 {
			return Err(refused("git.github_output_limit"));
		}
		let marker = marker(doc);
		let matching: Vec<_> = pulls
			.iter()
			.filter(|p| {
				p["body"]
					.as_str()
					.is_some_and(|body| body.contains(&marker))
			})
			.collect();
		match matching.as_slice() {
			[] if pulls.is_empty() => Ok(None),
			[one] => Ok(Some((*one).clone())),
			_ => Err(refused("git.draft_conflict")),
		}
	}
}
fn marker(doc: &Document) -> String {
	format!(
		"<!-- jet-conversation:{} -->",
		doc.delivery.conversation_id.0
	)
}
fn body(doc: &Document) -> String {
	format!(
		"{}\n\n{}\n<!-- jet-delivery:{} -->",
		doc.body(),
		marker(doc),
		doc.delivery.delivery_id
	)
}
fn validate(doc: &Document, pull: &Value) -> Result<(), CoreError> {
	if pull["draft"] != true
		|| pull["state"] != "open"
		|| pull["head"]["ref"].as_str() != doc.branch.as_deref()
		|| pull["head"]["sha"] != doc.head
		|| !pull["body"]
			.as_str()
			.is_some_and(|text| text.contains(&marker(doc)))
	{
		return Err(refused("git.draft_conflict"));
	}
	if let GitOperation::DraftPullRequest {
		base: Some(base), ..
	} = &doc.delivery.operation
		&& pull["base"]["ref"] != *base
	{
		return Err(refused("git.draft_conflict"));
	}
	Ok(())
}
pub(crate) async fn preflight(
	host: &dyn crate::GitHubHost,
	doc: &mut Document,
) -> Result<(), CoreError> {
	let host = GitHub {
		host,
		repository: repository(doc)?,
	};
	if let Some(url) = &doc.draft_url {
		let prefix = format!("https://github.com/{}/pull/", host.repository);
		doc.draft_number = Some(
			url.strip_prefix(&prefix)
				.and_then(|number| number.parse().ok())
				.ok_or_else(|| refused("git.draft_conflict"))?,
		);
	}
	let repo = host.request(GitHubRequest::Repository).await?;
	if repo["permissions"]["push"] != true {
		return Err(refused("git.github_permission_denied"));
	}
	let branch = doc
		.branch
		.clone()
		.ok_or_else(|| refused("git.branch_required"))?;
	if host.request(GitHubRequest::Branch { branch }).await?["commit"]["sha"]
		!= doc.head
	{
		return Err(refused("git.push_required"));
	}
	if let GitOperation::DraftPullRequest { base, .. } =
		&mut doc.delivery.operation
		&& base.is_none()
	{
		*base = Some(
			repo["default_branch"]
				.as_str()
				.ok_or_else(|| refused("git.github_invalid"))?
				.into(),
		);
	}
	io::validate_operation(&doc.delivery.operation)?;
	if let Some(existing) = host.existing(doc).await? {
		validate(doc, &existing)?;
		doc.draft_number = Some(
			existing["number"]
				.as_u64()
				.ok_or_else(|| refused("git.github_invalid"))?,
		);
	}
	Ok(())
}
pub(crate) async fn apply(
	host: &dyn crate::GitHubHost,
	doc: &Document,
) -> Result<String, CoreError> {
	let host = GitHub {
		host,
		repository: repository(doc)?,
	};
	let GitOperation::DraftPullRequest { base, .. } = &doc.delivery.operation
	else {
		return Err(refused("git.github_invalid"));
	};
	// Observe immediately before mutation; a promoted or closed PR is never reopened.
	let existing = host.existing(doc).await?;
	let body = body(doc);
	let pull = if let Some(existing) = existing {
		validate(doc, &existing)?;
		let number = existing["number"]
			.as_u64()
			.ok_or_else(|| refused("git.github_invalid"))?;
		host.request(GitHubRequest::UpdateDraft {
			number,
			title: doc.title().into(),
			body,
		})
		.await?
	} else {
		host.request(GitHubRequest::CreateDraft {
			title: doc.title().into(),
			body,
			head: doc
				.branch
				.clone()
				.ok_or_else(|| refused("git.branch_required"))?,
			base: base.clone().ok_or_else(|| refused("git.base_required"))?,
		})
		.await?
	};
	validate(doc, &pull)?;
	url(&host.repository, &pull)
}
pub(crate) async fn reconcile(
	host: &dyn crate::GitHubHost,
	doc: &Document,
) -> Result<Option<String>, CoreError> {
	let host = GitHub {
		host,
		repository: repository(doc)?,
	};
	let Some(pull) = host.existing(doc).await? else {
		return Ok(None);
	};
	validate(doc, &pull)?;
	if pull["title"] != doc.title()
		|| pull["body"] != body(doc)
		|| pull["head"]["sha"] != doc.head
	{
		return Ok(None);
	}
	url(&host.repository, &pull).map(Some)
}
fn url(repository: &str, pull: &Value) -> Result<String, CoreError> {
	let number = pull["number"]
		.as_u64()
		.ok_or_else(|| refused("git.github_invalid"))?;
	let url = format!("https://github.com/{repository}/pull/{number}");
	if pull["html_url"] != url {
		return Err(refused("git.github_invalid"));
	}
	Ok(url)
}
