//! Generate the client protocol contract directly from the Rust wire DTOs.
use jet_protocol::{
	ClientHello, ClientMessage, ConnectionProof, Event, PlaneStatus,
	RemotePairingRequest, RemotePairingResponse, ServerHello, ServerMessage,
	StreamControl, WireError,
};

#[derive(schemars::JsonSchema)]
#[expect(dead_code, reason = "schema roots, not instantiated runtime data")]
struct JetContracts {
	client_hello: ClientHello,
	server_hello: ServerHello,
	connection_proof: ConnectionProof,
	pairing_request: RemotePairingRequest,
	pairing_response: RemotePairingResponse,
	client_message: ClientMessage,
	server_message: ServerMessage,
	stream_control: StreamControl,
	event: Event,
	plane_status: PlaneStatus,
	wire_error: WireError,
}

fn main() {
	println!(
		"{}",
		serde_json::to_string_pretty(&schemars::schema_for!(JetContracts))
			.expect("schema serializes")
	);
}
