//! Choosing the reviewer for one Run, and refusing to choose badly.
//!
//! The default reviewer is the Run's own: the Provider and Account binding
//! its native execution already authenticates through. Sending the same
//! Conversation content somewhere else is a separate decision, so a binding
//! whose Provider is not the Run's own reviews nothing until the owner has
//! recorded persistent consent naming exactly that binding.

use uuid::Uuid;

use crate::{
	AccountBinding, AccountBindingId, AutomaticReviewPolicy, Core, CoreError,
	CredentialState, PinnedCraft, ProviderId, RunId, SettingKey, SettingScope,
	SettingValue, review::unavailable, run_state,
};

/// The Craft and binding one review may use.
pub(crate) struct Selection {
	/// The Run's own accepted Craft; the reviewer runs beside it, never
	/// inside the Run.
	pub(crate) craft: PinnedCraft,
	/// The binding whose Credential the reviewer authenticates with.
	pub(crate) binding: AccountBinding,
}

/// Where one review would be routed, and the attribution recorded either
/// way. A refusal is kept as a value rather than raised, because a review
/// that cannot happen is still a review the Plane has to account for.
pub(crate) struct Routing {
	pub(crate) policy: AutomaticReviewPolicy,
	pub(crate) binding_id: Option<AccountBindingId>,
	pub(crate) provider: Option<ProviderId>,
	pub(crate) selection: Result<Selection, CoreError>,
}

impl Core {
	/// Resolves the reviewer for `run_id` against the Plane's current policy,
	/// or `None` on a Plane that never turned Automatic review on. A Plane
	/// that did not opt in does nothing at all: no Plane is observed, no
	/// reviewer is selected, and nothing is recorded about a request it was
	/// never going to answer.
	///
	/// # Errors
	/// Returns a store error. A policy that permits no reviewer is not an
	/// error: it is carried in [`Routing::selection`].
	pub(crate) async fn review_routing(
		&self,
		run_id: RunId,
	) -> Result<Option<Routing>, CoreError> {
		if self
			.store
			.read(async |tx| {
				crate::setting::resolve_plane(tx, SettingKey::AutomaticReview)
					.await
			})
			.await? != SettingValue::Flag(true)
		{
			return Ok(None);
		}
		let plan = self
			.store
			.read(async |tx| {
				let record = tx
					.run_execution(run_id.0)
					.await?
					.ok_or_else(|| unavailable("review.run_unavailable"))?;
				run_state::decode::<crate::LaunchPlan>(&record.plan)
			})
			.await?;
		let host = self.run_host.as_ref();
		let run_provider =
			host.and_then(|host| host.native_provider(&plan.craft).ok());
		let security = self.security().await;
		let capabilities = self.observe_capabilities().await;
		self.store
			.read(async |tx| {
				let stored =
					tx.settings_for_scope(SettingScope::Plane.record()).await?;
				let values = crate::setting::resolve(
					&[
						SettingKey::AutomaticReviewBinding,
						SettingKey::AutomaticReviewConsent,
					],
					&stored,
				);
				// Both execution modes record the binding their native
				// authentication uses here: a No-Visa Run is admitted
				// through the same Visa selection, keeping its credentials
				// on the origin. That binding is the Run's own reviewer
				// unless the Plane names another.
				let binding_id = binding_of(&values[0].value)
					.or_else(|| plan.visa.map(|visa| visa.account_binding_id));
				let consent = binding_id.is_some_and(|id| {
					values[1].value == SettingValue::Text(id.0.to_string())
				});
				let binding = match binding_id {
					Some(id) => tx
						.account_binding(id.0)
						.await?
						.map(AccountBinding::from),
					None => None,
				};
				let cross_provider = match (&binding, &run_provider) {
					(Some(binding), Some(provider)) => {
						&binding.provider != provider
					}
					// An unknown native Provider is not a matching one.
					(Some(_), None) => true,
					(None, _) => false,
				};
				let policy = AutomaticReviewPolicy {
					version: 2,
					cross_provider_consent: consent,
				};
				let daemon_starts = tx.plane().await?.daemon_starts;
				let selection = select(
					Requested {
						cross_provider,
						consent,
						security,
					},
					binding.clone(),
					plan.craft.clone(),
					|reference| {
						CredentialState::of(
							reference,
							capabilities.credential_store,
							daemon_starts,
						)
					},
				);
				Ok(Some(Routing {
					policy,
					binding_id,
					provider: binding.map(|binding| binding.provider),
					selection,
				}))
			})
			.await
	}
}

/// What the Plane's policy says about one candidate reviewer.
struct Requested {
	cross_provider: bool,
	consent: bool,
	security: crate::SecurityState,
}

fn select(
	requested: Requested,
	binding: Option<AccountBinding>,
	craft: PinnedCraft,
	credential: impl Fn(&crate::CredentialReference) -> CredentialState,
) -> Result<Selection, CoreError> {
	// A Plane that cannot vouch for its Security audit does not decide on
	// a person's behalf; the request waits for them instead (ADR-0105).
	requested
		.security
		.admit(crate::security::SecurityClass::Guarded)
		.map_err(|_| unavailable("review.audit_degraded"))?;
	let binding =
		binding.ok_or_else(|| unavailable("review.binding_unavailable"))?;
	// ASVS 8.3.1: consent names the exact binding, so enabling review can
	// never widen where Conversation content is sent on its own.
	if requested.cross_provider && !requested.consent {
		return Err(unavailable("review.consent_required"));
	}
	if !matches!(
		credential(&binding.credential_reference),
		CredentialState::Resolvable | CredentialState::ResolvedAtUse
	) {
		return Err(unavailable("review.credential_unavailable"));
	}
	Ok(Selection { craft, binding })
}

fn binding_of(value: &SettingValue) -> Option<AccountBindingId> {
	match value {
		SettingValue::Text(text) => {
			Uuid::parse_str(text).ok().map(AccountBindingId)
		}
		SettingValue::Flag(_) | SettingValue::Count(_) => None,
	}
}

/// Rechecks mutable authorization in the decision's commit transaction.
pub(crate) async fn unchanged(
	tx: &mut jet_store::ReadTransaction,
	review: &crate::ApprovalReview,
	plan: &crate::LaunchPlan,
) -> Result<bool, CoreError> {
	let stored = tx.settings_for_scope(SettingScope::Plane.record()).await?;
	let values = crate::setting::resolve(
		&[
			SettingKey::AutomaticReview,
			SettingKey::AutomaticReviewBinding,
			SettingKey::AutomaticReviewConsent,
		],
		&stored,
	);
	let binding = binding_of(&values[1].value)
		.or_else(|| plan.visa.map(|visa| visa.account_binding_id));
	let consent = binding.is_some_and(|id| {
		values[2].value == SettingValue::Text(id.0.to_string())
	});
	// ASVS 8.3.2: disabling review or changing its binding/consent takes
	// effect before any in-flight judgement can become an authorization.
	Ok(values[0].value == SettingValue::Flag(true)
		&& binding == review.binding_id
		&& consent == review.policy.cross_provider_consent)
}
