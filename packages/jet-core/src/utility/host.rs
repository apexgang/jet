//! The external inference boundary has no broker, tools, roots, or Commands.
use crate::{AccountBinding, CoreError, RunFuture};
use serde::{Deserialize, Serialize};

/// A Craft's smallest suitable Model at minimum reasoning effort.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtilityModel {
	/// Exact Model identity, never an alias to be substituted after selection.
	pub name: String,
	/// Bounded opaque host pin; never sent as model input or returned to clients.
	pub adapter_state: String,
}
/// Only these allowlisted content fields may reach the inference request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "purpose", rename_all = "snake_case", deny_unknown_fields)]
pub enum UtilityInput {
	/// Title and initial user input, excluding native and terminal output.
	Naming {
		/// At most 256 UTF-8 bytes.
		title: String,
		/// At most 4096 UTF-8 bytes.
		opening_context: String,
	},
	/// One retained checkpoint and Plane-wide message guidance.
	GitText {
		/// At most 16384 UTF-8 bytes of the checkpoint patch.
		patch: String,
		/// At most 2048 UTF-8 bytes.
		instructions: String,
	},
	/// An isolated rule prompt without Conversation or checkpoint context.
	Autodelete {
		/// At most 4096 UTF-8 bytes.
		prompt: String,
	},
}
/// Untrusted response to one inference request.
#[derive(Debug)]
pub struct UtilityReply {
	/// Actual Model reported by the Provider, checked against the selected one.
	pub model: String,
	/// At most 8192 bytes; hosts must bound reads before allocating them.
	pub output: Vec<u8>,
}
/// Trusted Adapter for one selected Account binding's Craft. Implementations
/// select the smallest suitable Model at minimum reasoning, resolve only this
/// binding's credentials, and perform exactly one inference request with no
/// tools or job filesystem access. Errors never authorize another account,
/// Provider, Plane, or retry. Response reads must be bounded before allocation.
pub trait UtilityHost: std::fmt::Debug + Send + Sync {
	/// Select a Model without receiving any user content.
	fn select<'a>(
		&'a self,
		binding: &'a AccountBinding,
	) -> RunFuture<'a, Result<UtilityModel, CoreError>>;
	/// Execute the pinned selection. Credentials are transport authentication,
	/// never prompt content. Return a stable error without native diagnostics.
	fn infer<'a>(
		&'a self,
		binding: &'a AccountBinding,
		model: &'a UtilityModel,
		input: &'a UtilityInput,
	) -> RunFuture<'a, Result<UtilityReply, CoreError>>;
}
