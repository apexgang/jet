//! The same compatibility corpus is consumed by Rust, Swift, and TypeScript.
use jet_protocol::{CraftCommand, CraftEvent, ProtocolOffer, decode_control};
use pretty_assertions::assert_eq;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
	schema: String,
	valid: bool,
	payload: String,
}

#[test]
fn shared_gui_contract_fixtures_match_the_wire_decoder() {
	let fixtures: Vec<Fixture> =
		serde_json::from_str(include_str!("../contracts/fixtures.json"))
			.unwrap();
	for fixture in fixtures {
		let payload = fixture.payload.as_bytes();
		let accepted = match fixture.schema.as_str() {
			"CraftCommand" => decode_control::<CraftCommand>(payload).is_ok(),
			"CraftEvent" => decode_control::<CraftEvent>(payload).is_ok(),
			"ProtocolOffer" => decode_control::<ProtocolOffer>(payload).is_ok(),
			_ => panic!("unknown fixture schema"),
		};
		assert_eq!(accepted, fixture.valid, "{}", fixture.payload);
	}
}
