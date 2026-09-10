//! The external GitHub and platform-credential boundary, independent of Git policy.
use crate::{CoreError, RunFuture, git_delivery::state::refused};
use reqwest::{Client, Method};
use serde_json::json;
use std::time::Duration;
use tokio::process::Command;

/// Closed GitHub requests. None can merge, close, publish, or delete a PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubRequest {
	/// Repository permissions and default branch.
	Repository,
	/// Current commit of a hosted branch.
	Branch {
		/// Branch name, encoded as one URL path segment.
		branch: String,
	},
	/// Bounded PR lookup by exact owner and head branch.
	FindDrafts {
		/// Owner-qualified head branch.
		head: String,
	},
	/// Inspect a known draft.
	ReadDraft {
		/// GitHub PR number.
		number: u64,
	},
	/// Create a draft, never a ready-for-review PR.
	CreateDraft {
		/// Subject text, never executable.
		title: String,
		/// Body text including the Conversation's stable marker.
		body: String,
		/// Existing pushed branch.
		head: String,
		/// Target branch.
		base: String,
	},
	/// Update only the text of an existing draft.
	UpdateDraft {
		/// GitHub PR number.
		number: u64,
		/// Subject text.
		title: String,
		/// Body text.
		body: String,
	},
}
/// Trusted GitHub transport. Implementations resolve platform-store credentials
/// at every request, prohibit redirects/retries, and bound responses to 1 MiB.
/// Returned JSON is untrusted and validated by the core before any later action.
pub trait GitHubHost: std::fmt::Debug + Send + Sync {
	/// Perform one request against the exact GitHub repository, `owner/name`.
	fn request<'a>(
		&'a self,
		repository: &'a str,
		request: &'a GitHubRequest,
	) -> RunFuture<'a, Result<Vec<u8>, CoreError>>;
}
#[derive(Debug)]
pub(crate) struct SystemGitHub;
impl GitHubHost for SystemGitHub {
	fn request<'a>(
		&'a self,
		repository: &'a str,
		request: &'a GitHubRequest,
	) -> RunFuture<'a, Result<Vec<u8>, CoreError>> {
		Box::pin(async move {
			let token = credential().await?;
			let client = Client::builder()
				.timeout(Duration::from_secs(20))
				.redirect(reqwest::redirect::Policy::none())
				.retry(reqwest::retry::never())
				.user_agent("jet-git-delivery")
				.build()
				.map_err(|_| refused("git.github_unavailable"))?;
			let mut url = reqwest::Url::parse(&format!(
				"https://api.github.com/repos/{repository}"
			))
			.map_err(|_| refused("git.github_invalid"))?;
			let (method, body) = match request {
				GitHubRequest::Repository => (Method::GET, None),
				GitHubRequest::Branch { branch } => {
					url.path_segments_mut()
						.map_err(|_| refused("git.github_invalid"))?
						.extend(["branches", branch]);
					(Method::GET, None)
				}
				GitHubRequest::FindDrafts { head } => {
					url.path_segments_mut()
						.map_err(|_| refused("git.github_invalid"))?
						.push("pulls");
					url.query_pairs_mut().extend_pairs([
						("state", "all"),
						("head", head),
						("per_page", "100"),
					]);
					(Method::GET, None)
				}
				GitHubRequest::ReadDraft { number } => {
					url.path_segments_mut()
						.map_err(|_| refused("git.github_invalid"))?
						.extend(["pulls", &number.to_string()]);
					(Method::GET, None)
				}
				GitHubRequest::CreateDraft {
					title,
					body,
					head,
					base,
				} => {
					url.path_segments_mut()
						.map_err(|_| refused("git.github_invalid"))?
						.push("pulls");
					(
						Method::POST,
						Some(
							json!({"title":title,"body":body,"head":head,"base":base,"draft":true,"maintainer_can_modify":false}),
						),
					)
				}
				GitHubRequest::UpdateDraft {
					number,
					title,
					body,
				} => {
					url.path_segments_mut()
						.map_err(|_| refused("git.github_invalid"))?
						.extend(["pulls", &number.to_string()]);
					(Method::PATCH, Some(json!({"title":title,"body":body})))
				}
			};
			let mut request = client
				.request(method, url)
				.bearer_auth(token)
				.header("Accept", "application/vnd.github+json");
			if let Some(body) = body {
				request = request
					.header("Content-Type", "application/json")
					.body(body.to_string());
			}
			let mut response = request
				.send()
				.await
				.map_err(|_| refused("git.github_unavailable"))?;
			if !response.status().is_success() {
				return Err(refused("git.github_refused"));
			}
			let mut bytes = Vec::new();
			while let Some(chunk) = response
				.chunk()
				.await
				.map_err(|_| refused("git.github_unavailable"))?
			{
				if bytes.len() + chunk.len() > 1_048_576 {
					return Err(refused("git.github_output_limit"));
				}
				bytes.extend_from_slice(&chunk);
			}
			Ok(bytes)
		})
	}
}
async fn credential() -> Result<String, CoreError> {
	// ASVS 13.3.1: no environment, gh plaintext file, or database fallback.
	#[cfg(target_os = "macos")]
	let mut command = {
		let mut command = Command::new("/usr/bin/security");
		command.args([
			"find-generic-password",
			"-s",
			"me.heeka.jet.github",
			"-a",
			"github.com",
			"-w",
		]);
		command
	};
	#[cfg(not(target_os = "macos"))]
	let mut command = {
		let mut command = Command::new("secret-tool");
		command.args([
			"lookup",
			"service",
			"me.heeka.jet.github",
			"account",
			"github.com",
		]);
		command
	};
	let token = crate::git_delivery::io::run(&mut command, &[], 4096)
		.await
		.map_err(|_| refused("git.github_credential_unavailable"))?;
	if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
		return Err(refused("git.github_credential_unavailable"));
	}
	Ok(token)
}
