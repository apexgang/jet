use pretty_assertions::assert_eq;

use crate::test_support::{actor, request, start_core};
use crate::{
	AuditDecision, AuditOutcome, AuditRisk, AuditSequence, Command,
	CommandOutcome, Core, CoreError, EventKind, EventSequence, PairingGate,
	PairingSnapshot, Query, QueryResult,
};

async fn start(dir: &tempfile::TempDir) -> Core {
	start_core(&dir.path().join("plane.sqlite3")).await
}

async fn set_gate(
	core: &Core,
	gate: PairingGate,
) -> Result<CommandOutcome, CoreError> {
	core.execute(&actor(), request(Command::SetPairingGate { gate }))
		.await
}

async fn pairing(core: &Core) -> PairingSnapshot {
	let result = core.query(&actor(), Query::Pairing).await.unwrap();
	let QueryResult::Pairing(snapshot) = result else {
		panic!("unexpected result {result:?}");
	};
	snapshot
}

async fn events(core: &Core) -> Vec<EventKind> {
	let result = core
		.query(
			&actor(),
			Query::Events {
				after: EventSequence(0),
			},
		)
		.await
		.unwrap();
	let QueryResult::Events(page) = result else {
		panic!("unexpected result {result:?}");
	};
	page.events.into_iter().map(|event| event.kind).collect()
}

async fn decisions(core: &Core) -> Vec<(String, AuditRisk, AuditOutcome)> {
	let result = core
		.query(
			&actor(),
			Query::SecurityAudit {
				after: AuditSequence(0),
			},
		)
		.await
		.unwrap();
	let QueryResult::SecurityAudit(page) = result else {
		panic!("unexpected result {result:?}");
	};
	page.entries
		.into_iter()
		.map(|entry| (entry.decision, entry.risk, entry.outcome))
		.collect()
}

#[tokio::test]
async fn a_plane_accepts_no_new_client_until_its_owner_opens_the_gate() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;

	let untouched = pairing(&core).await;
	let opened = set_gate(&core, PairingGate::Open).await.unwrap();
	let while_open = pairing(&core).await;
	let closed = set_gate(&core, PairingGate::Closed).await.unwrap();
	let after_closing = pairing(&core).await;

	assert_eq!(
		(untouched, opened, while_open, closed, after_closing),
		(
			PairingSnapshot {
				cursor: EventSequence(0),
				gate: PairingGate::Closed,
				pending: None,
			},
			CommandOutcome::PairingGateSet {
				gate: PairingGate::Open,
			},
			PairingSnapshot {
				cursor: EventSequence(1),
				gate: PairingGate::Open,
				pending: None,
			},
			CommandOutcome::PairingGateSet {
				gate: PairingGate::Closed,
			},
			PairingSnapshot {
				cursor: EventSequence(2),
				gate: PairingGate::Closed,
				pending: None,
			}
		)
	);
}

#[tokio::test]
async fn opening_the_gate_is_journaled_and_recorded_as_a_decision() {
	let dir = tempfile::tempdir().unwrap();
	let core = start(&dir).await;

	set_gate(&core, PairingGate::Open).await.unwrap();
	set_gate(&core, PairingGate::Closed).await.unwrap();

	assert_eq!(
		(events(&core).await, decisions(&core).await),
		(
			vec![
				EventKind::PairingGateChanged {
					gate: PairingGate::Open,
				},
				EventKind::PairingGateChanged {
					gate: PairingGate::Closed,
				},
			],
			vec![
				(
					AuditDecision::PairingGateOpened.as_str().into(),
					AuditRisk::Elevated,
					AuditOutcome::Succeeded,
				),
				(
					AuditDecision::PairingGateClosed.as_str().into(),
					AuditRisk::Routine,
					AuditOutcome::Succeeded,
				),
			]
		)
	);
}
