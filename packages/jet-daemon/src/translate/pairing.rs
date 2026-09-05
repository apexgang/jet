//! The Pairing half of the translation seam (ADR-0049, ADR-0017).

use jet_core::{
	AuthenticationString, ClientPublicKey, PairedClient, PairedClientAccess,
	PairingDisclosure, PairingEnd, PairingGate, PairingKeyAlgorithm,
	PairingMethod, PairingProgress, PairingSnapshot, PendingPairing,
};
use jet_protocol as wire;

use super::unix_ms;

pub(super) fn snapshot(snapshot: PairingSnapshot) -> wire::PairingSnapshot {
	wire::PairingSnapshot {
		cursor: snapshot.cursor.0,
		gate: gate(snapshot.gate),
		pending: snapshot.pending.map(pending),
		clients: snapshot.clients.into_iter().map(client).collect(),
	}
}

pub(super) fn access_from_wire(
	access: wire::PairedClientAccess,
) -> PairedClientAccess {
	match access {
		wire::PairedClientAccess::Enabled => PairedClientAccess::Enabled,
		wire::PairedClientAccess::Disabled => PairedClientAccess::Disabled,
	}
}

pub(crate) fn client(client: PairedClient) -> wire::PairedClient {
	wire::PairedClient {
		client_id: client.client_id.0,
		key: key(client.key),
		pairing_protocol: client.pairing_protocol,
		access: match client.access {
			PairedClientAccess::Enabled => wire::PairedClientAccess::Enabled,
			PairedClientAccess::Disabled => wire::PairedClientAccess::Disabled,
		},
		paired_at_unix_ms: unix_ms(client.paired_at),
	}
}

fn key(key: ClientPublicKey) -> wire::ClientPublicKey {
	wire::ClientPublicKey {
		algorithm: match key.algorithm {
			PairingKeyAlgorithm::Ed25519 => wire::PairingKeyAlgorithm::Ed25519,
		},
		key: key.key,
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

pub(crate) fn pending(pending: PendingPairing) -> wire::PendingPairing {
	wire::PendingPairing {
		offer_id: pending.offer_id.0,
		method: method(pending.method),
		progress: progress(pending.progress),
		attempts_remaining: pending.attempts_remaining,
		opened_at_unix_ms: unix_ms(pending.opened_at),
		expires_at_unix_ms: unix_ms(pending.expires_at),
	}
}

pub(super) fn disclosure(
	disclosure: PairingDisclosure,
) -> wire::PairingDisclosure {
	match disclosure {
		PairingDisclosure::ManualCode { code } => {
			wire::PairingDisclosure::ManualCode { code }
		}
		PairingDisclosure::QrPayload { payload } => {
			wire::PairingDisclosure::QrPayload { payload }
		}
		PairingDisclosure::AlreadyDisclosed => {
			wire::PairingDisclosure::AlreadyDisclosed
		}
	}
}

pub(super) fn method_from_wire(method: &wire::PairingMethod) -> PairingMethod {
	match method {
		wire::PairingMethod::ManualCode => PairingMethod::ManualCode,
		wire::PairingMethod::QrPayload { endpoint } => {
			PairingMethod::QrPayload {
				endpoint: endpoint.clone(),
			}
		}
	}
}

pub(super) fn key_from_wire(key: &wire::ClientPublicKey) -> ClientPublicKey {
	ClientPublicKey {
		algorithm: match key.algorithm {
			wire::PairingKeyAlgorithm::Ed25519 => PairingKeyAlgorithm::Ed25519,
		},
		key: key.key,
	}
}

fn method(method: PairingMethod) -> wire::PairingMethod {
	match method {
		PairingMethod::ManualCode => wire::PairingMethod::ManualCode,
		PairingMethod::QrPayload { endpoint } => {
			wire::PairingMethod::QrPayload { endpoint }
		}
	}
}

fn progress(progress: PairingProgress) -> wire::PairingProgress {
	match progress {
		PairingProgress::Offered => wire::PairingProgress::Offered,
		PairingProgress::AwaitingConfirmation {
			client_id,
			authentication_string: AuthenticationString(displayed),
		} => wire::PairingProgress::AwaitingConfirmation {
			client_id: client_id.0,
			authentication_string: displayed,
		},
		PairingProgress::Confirmed {
			client_id,
			authentication_string: AuthenticationString(displayed),
		} => wire::PairingProgress::Confirmed {
			client_id: client_id.0,
			authentication_string: displayed,
		},
		PairingProgress::Ended { reason } => wire::PairingProgress::Ended {
			reason: end(reason),
		},
	}
}

fn end(reason: PairingEnd) -> wire::PairingEnd {
	match reason {
		PairingEnd::Expired => wire::PairingEnd::Expired,
		PairingEnd::TooManyAttempts => wire::PairingEnd::TooManyAttempts,
		PairingEnd::GateClosed => wire::PairingEnd::GateClosed,
	}
}
