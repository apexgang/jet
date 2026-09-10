//! An accepted Craft's isolated reviewer entrypoint; never a Run or native
//! session (ADR-0012).
//!
//! The reviewer for a Run is the Run's own Craft whenever that Craft speaks
//! for the selected binding's Provider, so a Harness that carries its own
//! separate reviewer is the one that answers. A binding for another
//! Provider — which Core admits only against recorded persistent consent —
//! is reviewed by the bundled Craft that speaks for it.
//!
//! The process this starts has no working directory of the Run, no broker,
//! and no way back into the execution it is judging.
use jet_core::{
	AccountBinding, CoreError, PinnedCraft, ReviewInput, ReviewReply,
	ReviewerSelection, RunFuture,
};
use jet_protocol::{
	CraftHostAccess, CraftReviewInput, CraftReviewModel, CraftReviewReply,
	CraftReviewRequest, CraftReviewer,
};
use std::path::PathBuf;

/// Most bytes one `--review` request may occupy.
const REQUEST_BYTES: usize = 131_072;
/// Most bytes one reviewer answer may occupy.
const REPLY_BYTES: u64 = 65_536;
/// Most bytes the content-free selection may occupy.
const SELECTION_BYTES: u64 = 1024;

#[derive(Debug)]
pub(crate) struct Reviews {
	pub(crate) home: PathBuf,
}

impl jet_core::ReviewHost for Reviews {
	fn select<'a>(
		&'a self,
		craft: &'a PinnedCraft,
		binding: &'a AccountBinding,
	) -> RunFuture<'a, Result<ReviewerSelection, CoreError>> {
		Box::pin(async move {
			let pin = self.reviewing_craft(craft, binding).await?;
			let bytes = crate::craft::utility_host::exchange(
				&pin,
				"--review-model",
				&[],
				SELECTION_BYTES,
			)
			.await?;
			let selection: CraftReviewModel =
				serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
			if selection.version != 1 {
				return Err(unavailable());
			}
			Ok(ReviewerSelection {
				reviewer: reviewer(selection.reviewer),
				model: selection.model,
				adapter_state: serde_json::to_string(&pin)
					.map_err(|_| unavailable())?,
			})
		})
	}

	fn review<'a>(
		&'a self,
		_craft: &'a PinnedCraft,
		binding: &'a AccountBinding,
		selection: &'a ReviewerSelection,
		input: &'a ReviewInput,
	) -> RunFuture<'a, Result<ReviewReply, CoreError>> {
		Box::pin(async move {
			// The pin travels in the selection, so the artifact reviewed
			// against is the one selection verified, not one resolved again.
			let pin: PinnedCraft =
				serde_json::from_str(&selection.adapter_state)
					.map_err(|_| unavailable())?;
			let request = CraftReviewRequest {
				version: 1,
				model: selection.model.clone(),
				binding_id: binding.binding_id.0,
				credential_reference: crate::translate::utility::reference(
					binding.credential_reference.clone(),
				),
				input: CraftReviewInput {
					transcript: input.transcript.clone(),
					tool: input.tool.clone(),
					action: input.action.clone(),
				},
			};
			let bytes =
				serde_json::to_vec(&request).map_err(|_| unavailable())?;
			if bytes.len() > REQUEST_BYTES {
				return Err(unavailable());
			}
			let bytes = crate::craft::utility_host::exchange(
				&pin,
				"--review",
				&bytes,
				REPLY_BYTES,
			)
			.await?;
			let reply: CraftReviewReply =
				serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
			if reply.version != 1 {
				return Err(unavailable());
			}
			Ok(ReviewReply {
				model: reply.model,
				reviewer: reviewer(reply.reviewer),
				output: reply.output.into_bytes(),
			})
		})
	}
}

impl Reviews {
	/// The accepted Craft that reviews for `binding`: the Run's own when it
	/// speaks for that Provider, and otherwise the installed Craft that
	/// does. Either way it must declare the reviewer feature and the host
	/// access the reviewer actually uses.
	async fn reviewing_craft(
		&self,
		craft: &PinnedCraft,
		binding: &AccountBinding,
	) -> Result<PinnedCraft, CoreError> {
		let (id, destination) = match binding.provider.0.as_str() {
			"anthropic" => ("claude-code", "api.anthropic.com"),
			"openai" => ("codex", "api.openai.com"),
			_ => return Err(unavailable()),
		};
		let pin = match crate::run::craft::native_provider(craft) {
			Ok(provider) if provider == binding.provider => craft.clone(),
			Ok(_) | Err(_) => crate::run::craft::load(&self.home, id).await?,
		};
		let contract = crate::run::craft::Contract::of(&pin)?;
		if !contract
			.specification
			.enabled_features()
			.map_err(|_| unavailable())?
			.iter()
			.any(|feature| feature == "review")
			|| !contract.specification.host_access.contains(
				&CraftHostAccess::Executable {
					name: "curl".into(),
				},
			) || !contract.specification.host_access.contains(
			&CraftHostAccess::Network {
				destination: destination.into(),
			},
		) {
			return Err(unavailable());
		}
		Ok(pin)
	}
}

fn reviewer(reviewer: CraftReviewer) -> jet_core::Reviewer {
	match reviewer {
		CraftReviewer::Native => jet_core::Reviewer::Native,
		CraftReviewer::Equivalent => jet_core::Reviewer::Equivalent,
	}
}

fn unavailable() -> CoreError {
	CoreError {
		category: jet_core::ErrorCategory::Unavailable,
		code: "review.craft_unavailable".into(),
		retryable: false,
		message: "the selected reviewer Craft is unavailable".into(),
		detail: None,
		revision_conflict: None,
		recovery_actions: vec![],
	}
}
