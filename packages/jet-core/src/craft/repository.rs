//! Bounded HTTPS access to public GitHub Craft releases (ADR-0013).

use crate::{CoreError, craft::publication::ArtifactStaging};
use serde::Deserialize;
use std::{future::Future, pin::Pin, time::Duration};

pub(crate) const MAX_SPECIFICATION_BYTES: usize = 64 * 1024;
pub(crate) const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleasedCraft {
	pub(crate) repository: String,
	pub(crate) tag: String,
	pub(crate) commit: String,
	pub(crate) specification: Vec<u8>,
	pub(crate) artifacts: Vec<ReleasedArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleasedArtifact {
	pub(crate) name: String,
	pub(crate) url: String,
	pub(crate) sha256: String,
	pub(crate) size: u64,
}

/// Boundary for resolving immutable release metadata and streaming the exact
/// Artifact it named. Implementations must enforce the byte bound and must not
/// treat a mutable repository reference as verified provenance.
pub(crate) trait CraftRepository: std::fmt::Debug + Send + Sync {
	/// Resolves one tag to its commit, versioned specification, and assets.
	fn release(
		&self,
		repository: &str,
		tag: &str,
	) -> Pin<
		Box<dyn Future<Output = Result<ReleasedCraft, CoreError>> + Send + '_>,
	>;

	/// Downloads one release Artifact without exceeding `limit` bytes.
	fn download(
		&self,
		url: &str,
		limit: u64,
		staging: ArtifactStaging,
	) -> Pin<Box<dyn Future<Output = Result<(), CoreError>> + Send + '_>>;
}

#[derive(Debug)]
pub(crate) struct SystemCraftRepository;

#[derive(Deserialize)]
struct CommitResponse {
	sha: String,
}

#[derive(Deserialize)]
struct ReleaseResponse {
	tag_name: String,
	assets: Vec<AssetResponse>,
}

#[derive(Deserialize)]
struct AssetResponse {
	name: String,
	browser_download_url: String,
	digest: Option<String>,
	size: u64,
}

impl CraftRepository for SystemCraftRepository {
	fn release(
		&self,
		repository: &str,
		tag: &str,
	) -> Pin<
		Box<dyn Future<Output = Result<ReleasedCraft, CoreError>> + Send + '_>,
	> {
		let repository = repository.to_owned();
		let tag = tag.to_owned();
		Box::pin(async move { github_release(&repository, &tag).await })
	}

	fn download(
		&self,
		url: &str,
		limit: u64,
		staging: ArtifactStaging,
	) -> Pin<Box<dyn Future<Output = Result<(), CoreError>> + Send + '_>> {
		let url = url.to_owned();
		Box::pin(async move {
			let url = match reqwest::Url::parse(&url) {
				Ok(url) => url,
				Err(_) => {
					staging.abort().await;
					return Err(unavailable());
				}
			};
			if !allowed_github_url(&url) {
				staging.abort().await;
				return Err(unavailable());
			}
			get_artifact(url, limit, staging).await
		})
	}
}

async fn github_release(
	repository: &str,
	tag: &str,
) -> Result<ReleasedCraft, CoreError> {
	let (owner, name) = repository.split_once('/').ok_or_else(unavailable)?;
	let commit_url = github_api(&["repos", owner, name, "commits", tag])?;
	let release_url =
		github_api(&["repos", owner, name, "releases", "tags", tag])?;
	let (commit, release) = tokio::try_join!(
		get_json::<CommitResponse>(commit_url),
		get_json::<ReleaseResponse>(release_url),
	)?;
	let specification_url = github_raw(repository, &commit.sha)?;
	let specification = get(specification_url, MAX_SPECIFICATION_BYTES).await?;
	let artifacts = release
		.assets
		.into_iter()
		.filter_map(released_artifact)
		.collect();
	Ok(ReleasedCraft {
		repository: repository.into(),
		tag: release.tag_name,
		commit: commit.sha,
		specification,
		artifacts,
	})
}

fn released_artifact(asset: AssetResponse) -> Option<ReleasedArtifact> {
	let sha256 = asset.digest?.strip_prefix("sha256:")?.to_owned();
	Some(ReleasedArtifact {
		name: asset.name,
		url: asset.browser_download_url,
		sha256,
		size: asset.size,
	})
}

async fn get_json<T: serde::de::DeserializeOwned>(
	url: reqwest::Url,
) -> Result<T, CoreError> {
	let bytes = get(url, MAX_METADATA_BYTES).await?;
	serde_json::from_slice(&bytes).map_err(|_| unavailable())
}

async fn get(url: reqwest::Url, limit: usize) -> Result<Vec<u8>, CoreError> {
	// ASVS 12.3.1-2, 13.2.4: TLS is mandatory and redirects stay on GitHub.
	let client = github_client()?;
	let mut response =
		client.get(url).send().await.map_err(|_| unavailable())?;
	if !response.status().is_success()
		|| response
			.content_length()
			.is_some_and(|length| length > limit as u64)
	{
		return Err(unavailable());
	}
	let mut bytes = Vec::new();
	while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
		if bytes.len().saturating_add(chunk.len()) > limit {
			return Err(unavailable());
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(bytes)
}

fn github_client() -> Result<reqwest::Client, CoreError> {
	reqwest::Client::builder()
		.https_only(true)
		.connect_timeout(Duration::from_secs(10))
		.timeout(Duration::from_secs(30))
		.redirect(reqwest::redirect::Policy::custom(|attempt| {
			if attempt.previous().len() >= 3
				|| !allowed_github_url(attempt.url())
			{
				attempt.stop()
			} else {
				attempt.follow()
			}
		}))
		.user_agent("jetd-craft-discovery/0.2")
		.build()
		.map_err(|_| unavailable())
}

async fn get_artifact(
	url: reqwest::Url,
	limit: u64,
	mut staging: ArtifactStaging,
) -> Result<(), CoreError> {
	let result = async {
		let client = github_client()?;
		let mut response =
			client.get(url).send().await.map_err(|_| unavailable())?;
		if !response.status().is_success()
			|| response
				.content_length()
				.is_some_and(|length| length > limit)
		{
			return Err(unavailable());
		}
		let mut received = 0_u64;
		while let Some(chunk) =
			response.chunk().await.map_err(|_| unavailable())?
		{
			received = received
				.checked_add(chunk.len() as u64)
				.filter(|received| *received <= limit)
				.ok_or_else(unavailable)?;
			staging.write_chunk(&chunk).await?;
		}
		Ok(())
	}
	.await;
	match result {
		Ok(()) => staging.finish().await,
		Err(error) => {
			staging.abort().await;
			Err(error)
		}
	}
}

fn github_api(parts: &[&str]) -> Result<reqwest::Url, CoreError> {
	github_url("https://api.github.com/", parts)
}

fn github_raw(
	repository: &str,
	commit: &str,
) -> Result<reqwest::Url, CoreError> {
	let (owner, name) = repository.split_once('/').ok_or_else(unavailable)?;
	github_url(
		"https://raw.githubusercontent.com/",
		&[owner, name, commit, ".jet", "craft-spec.toml"],
	)
}

fn github_url(base: &str, parts: &[&str]) -> Result<reqwest::Url, CoreError> {
	let mut url = reqwest::Url::parse(base).map_err(|_| unavailable())?;
	let mut segments = url.path_segments_mut().map_err(|_| unavailable())?;
	for part in parts {
		segments.push(part);
	}
	drop(segments);
	Ok(url)
}

fn allowed_github_url(url: &reqwest::Url) -> bool {
	if url.scheme() != "https"
		|| !url.username().is_empty()
		|| url.password().is_some()
	{
		return false;
	}
	url.host_str().is_some_and(|host| {
		host == "github.com"
			|| host == "api.github.com"
			|| host == "raw.githubusercontent.com"
			|| host.ends_with(".githubusercontent.com")
	})
}

fn unavailable() -> CoreError {
	CoreError::unavailable(
		"craft.repository_unavailable",
		"the Craft repository could not be verified",
		"GitHub release access failed",
	)
}

#[cfg(test)]
mod tests {
	use super::{AssetResponse, released_artifact};

	#[test]
	fn only_assets_with_github_sha256_evidence_enter_discovery() {
		let asset = |digest| AssetResponse {
			name: "craft".into(),
			browser_download_url: "https://github.com/example/craft".into(),
			digest,
			size: 42,
		};

		assert!(released_artifact(asset(None)).is_none());
		assert!(released_artifact(asset(Some("sha512:abcd".into()))).is_none());
		assert_eq!(
			released_artifact(asset(Some(format!(
				"sha256:{}",
				"ab".repeat(32)
			))))
			.unwrap()
			.sha256,
			"ab".repeat(32)
		);
	}
}
