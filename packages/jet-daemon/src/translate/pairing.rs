//! The Pairing half of the translation seam (ADR-0049, ADR-0017).

use jet_core::{PairingGate, PairingSnapshot};
use jet_protocol as wire;

pub(super) fn snapshot(snapshot: PairingSnapshot) -> wire::PairingSnapshot {
	wire::PairingSnapshot {
		cursor: snapshot.cursor.0,
		gate: gate(snapshot.gate),
	}
}

pub(super) fn gate(gate: PairingGate) -> wire::PairingGate {
	match gate {
		PairingGate::Open => wire::PairingGate::Open,
		PairingGate::Closed => wire::PairingGate::Closed,
	}
}

pub(super) fn gate_from_wire(gate: wire::PairingGate) -> PairingGate {
	match gate {
		wire::PairingGate::Open => PairingGate::Open,
		wire::PairingGate::Closed => PairingGate::Closed,
	}
}
