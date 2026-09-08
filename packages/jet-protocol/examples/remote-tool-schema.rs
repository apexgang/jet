//! Generate the native MCP tool's input contract from the same wire DTO.
fn main() {
	println!(
		"{}",
		serde_json::to_string_pretty(&schemars::schema_for!(
			jet_protocol::CraftRemoteTool
		))
		.expect("schema serializes")
	);
}
