//! The same compatibility corpora are consumed by Rust, Swift, and TypeScript.
use jet_protocol::{
	ClientHello, ClientMessage, ConnectionProof, CraftCommand, CraftEvent,
	Event, PlaneStatus, ProtocolOffer, ServerHello, ServerMessage,
	StreamControl, decode_control,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
	schema: String,
	valid: bool,
	payload: String,
}

/// Decodes every fixture in one corpus through the decoder its schema names.
fn accepts_exactly(corpus: &str, decode: impl Fn(&str, &[u8]) -> bool) {
	let fixtures: Vec<Fixture> = serde_json::from_str(corpus).unwrap();
	for fixture in fixtures {
		let accepted = decode(&fixture.schema, fixture.payload.as_bytes());
		assert_eq!(accepted, fixture.valid, "{}", fixture.payload);
	}
}

#[test]
fn shared_craft_contract_fixtures_match_the_wire_decoder() {
	accepts_exactly(
		include_str!("../contracts/craft-fixtures.json"),
		|schema, payload| match schema {
			"CraftExtensionRequest" => {
				decode_control::<jet_protocol::CraftExtensionRequest>(payload)
					.is_ok()
			}
			"CraftExtensionReply" => {
				decode_control::<jet_protocol::CraftExtensionReply>(payload)
					.is_ok()
			}
			"CraftUtilityModel" => {
				decode_control::<jet_protocol::CraftUtilityModel>(payload)
					.is_ok()
			}
			"CraftUtilityRequest" => {
				decode_control::<jet_protocol::CraftUtilityRequest>(payload)
					.is_ok()
			}
			"CraftUtilityReply" => {
				decode_control::<jet_protocol::CraftUtilityReply>(payload)
					.is_ok()
			}
			"CraftReviewModel" => {
				decode_control::<jet_protocol::CraftReviewModel>(payload)
					.is_ok()
			}
			"CraftReviewRequest" => {
				decode_control::<jet_protocol::CraftReviewRequest>(payload)
					.is_ok()
			}
			"CraftReviewReply" => {
				decode_control::<jet_protocol::CraftReviewReply>(payload)
					.is_ok()
			}
			"CraftCommand" => decode_control::<CraftCommand>(payload).is_ok(),
			"CraftEvent" => decode_control::<CraftEvent>(payload).is_ok(),
			"ProtocolOffer" => decode_control::<ProtocolOffer>(payload).is_ok(),
			_ => panic!("unknown fixture schema"),
		},
	);
}

#[test]
fn shared_client_contract_fixtures_match_the_wire_decoder() {
	accepts_exactly(
		include_str!("../contracts/jet-fixtures.json"),
		|schema, payload| match schema {
			"ClientHello" => decode_control::<ClientHello>(payload).is_ok(),
			"ServerHello" => decode_control::<ServerHello>(payload).is_ok(),
			"ClientMessage" => decode_control::<ClientMessage>(payload).is_ok(),
			"ServerMessage" => decode_control::<ServerMessage>(payload).is_ok(),
			"ConnectionProof" => {
				decode_control::<ConnectionProof>(payload).is_ok()
			}
			"StreamControl" => decode_control::<StreamControl>(payload).is_ok(),
			"ArtifactControl" => {
				decode_control::<jet_protocol::ArtifactControl>(payload).is_ok()
			}
			"Event" => decode_control::<Event>(payload).is_ok(),
			"PlaneStatus" => decode_control::<PlaneStatus>(payload).is_ok(),
			_ => panic!("unknown fixture schema"),
		},
	);
}
