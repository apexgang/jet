//! Admission budgets never revoke an existing execution or remove pending input.
use crate::{
	CoreError, RunId, SettingKey, SettingScope, SettingValue, TurnSource,
};
use jet_store::ReadTransaction;

pub(crate) struct Policy {
	pub(crate) limit: u32,
	pub(crate) foreground_override: bool,
	pub(crate) child_work: ChildWork,
}

/// Admission-only native child policy. Active children always keep running.
#[derive(
	Debug,
	Clone,
	Copy,
	Default,
	PartialEq,
	Eq,
	serde::Serialize,
	serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ChildWork {
	/// Preserve the Harness's normal child policy.
	#[default]
	Native,
	/// Reserve the constrained Plane budget for Runs; admit no new children.
	Paused,
}

pub(crate) async fn policy(
	core: &crate::Core,
	tx: &mut ReadTransaction,
) -> Result<Policy, CoreError> {
	let stored = tx.settings_for_scope(SettingScope::Plane.record()).await?;
	let values = crate::setting::resolve(
		&[
			SettingKey::EnergyConcurrency,
			SettingKey::EnergyLowPowerConcurrency,
			SettingKey::EnergyConstrained,
			SettingKey::EnergyForegroundOverride,
		],
		&stored,
	);
	let [normal, reduced, constrained, foreground] = values.as_slice() else {
		unreachable!("four requested Settings")
	};
	let (
		SettingValue::Count(normal),
		SettingValue::Count(reduced),
		SettingValue::Flag(constrained),
		SettingValue::Flag(foreground_override),
	) = (
		&normal.value,
		&reduced.value,
		&constrained.value,
		&foreground.value,
	)
	else {
		return Err(CoreError::conflict(
			"energy.policy_unreadable",
			"the Plane Energy policy cannot be read",
		));
	};
	let constrained = *constrained
		|| core.probe.power().await != jet_runtime::PowerState::Normal;
	Ok(Policy {
		limit: if constrained {
			(*normal).min(*reduced)
		} else {
			*normal
		},
		foreground_override: *foreground_override,
		child_work: if constrained {
			ChildWork::Paused
		} else {
			ChildWork::Native
		},
	})
}

pub(crate) enum Admission {
	NewRun,
	ExistingRun(RunId),
}

pub(crate) async fn admit(
	core: &crate::Core,
	tx: &mut ReadTransaction,
	source: TurnSource,
	admission: Admission,
) -> Result<ChildWork, CoreError> {
	let policy = policy(core, tx).await?;
	if source == TurnSource::User && policy.foreground_override {
		return Ok(policy.child_work);
	}
	// ASVS 2.3.4, 15.4.2: count reservations and claim new work in the same
	// store transaction. Starting and stopping executions still consume capacity.
	let mut count = 0;
	let mut after = String::new();
	while count < policy.limit {
		let ids = tx.active_execution_ids(&after).await?;
		let Some(last) = ids.last() else {
			return Ok(policy.child_work);
		};
		after = last.to_string();
		count += ids
			.into_iter()
			.filter(
				|id| !matches!(admission, Admission::ExistingRun(run) if run.0 == *id),
			)
			.count() as u32;
	}
	Err(CoreError::conflict(
		"energy.budget_exhausted",
		"the Plane Energy budget is full; queued input is retained. Foreground work requires an explicit energy.foreground_override Setting",
	))
}

impl crate::Core {
	/// Apply the current energy constraint to capable active Crafts. Unsupported
	/// Crafts remain monitor-only; no control terminates an existing child.
	/// # Errors
	/// Returns a policy or transport error without altering Runs or their queues.
	pub async fn constrain_child_work(&self) -> Result<(), CoreError> {
		let connections: Vec<_> = self
			.run_recovery
			.connections
			.lock()
			.expect("connection lock")
			.values()
			.cloned()
			.collect();
		if connections.is_empty() {
			return Ok(());
		}
		let (policy, pending) = self
			.store
			.read(async |tx| {
				Ok::<_, CoreError>((
					policy(self, tx).await?,
					!tx.pending_turn_queues("").await?.is_empty(),
				))
			})
			.await?;
		if pending {
			// Admission may have seen a transient power constraint between two
			// normal maintenance samples. Retry retained input on every bounded
			// active-work deadline, even when the policy appears unchanged.
			self.turn_wake.send_replace(());
			self.run_work.notify_one();
		}
		for connection in connections {
			connection.constrain_children(policy.child_work).await?;
		}
		Ok(())
	}
}
