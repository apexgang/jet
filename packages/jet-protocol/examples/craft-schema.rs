//! Generate the language-neutral Craft contract directly from Rust DTOs.
use jet_protocol::{
	CraftCommand, CraftEvent, CraftHello, CraftReady, CraftSpecification,
	Presentation,
};

#[derive(schemars::JsonSchema)]
#[expect(dead_code, reason = "schema roots, not instantiated runtime data")]
struct CraftContracts {
	extension_request: jet_protocol::CraftExtensionRequest,
	extension_reply: jet_protocol::CraftExtensionReply,
	utility_model: jet_protocol::CraftUtilityModel,
	utility_request: jet_protocol::CraftUtilityRequest,
	utility_reply: jet_protocol::CraftUtilityReply,
	review_model: jet_protocol::CraftReviewModel,
	review_request: jet_protocol::CraftReviewRequest,
	review_reply: jet_protocol::CraftReviewReply,
	hello: CraftHello,
	ready: CraftReady,
	command: CraftCommand,
	event: CraftEvent,
	specification: CraftSpecification,
	presentation: Presentation,
}

fn main() {
	println!(
		"{}",
		serde_json::to_string_pretty(&schemars::schema_for!(CraftContracts))
			.expect("schema serializes")
	);
}
