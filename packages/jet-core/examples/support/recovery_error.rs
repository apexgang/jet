//! Same shared error payload in both size probes, without widening core API.
pub struct CoreError(jet_core::CoreError);
impl std::fmt::Debug for CoreError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		self.0.fmt(f)
	}
}
impl CoreError {
	pub(crate) fn invalid_input(code: &'static str, message: &str) -> Self {
		Self(jet_core::CoreError {
			category: jet_core::ErrorCategory::InvalidInput,
			code: code.into(),
			message: message.into(),
			retryable: false,
			detail: None,
			revision_conflict: None,
			recovery_actions: vec![],
		})
	}
}
