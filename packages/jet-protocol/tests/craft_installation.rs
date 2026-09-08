//! Public wire compatibility for third-party Craft installation.

use jet_protocol::{
	BrokerPermission, ClientMessage, CommandRequest, CraftHostAccess,
	CraftInstallationConfirmation, CraftSource, CraftTrust, QueryRequest,
	decode_control, encode_control,
};
use pretty_assertions::assert_eq;

#[test]
fn discovery_and_exact_confirmation_round_trip_without_losing_authority() {
	let source = CraftSource::GitHubRelease {
		repository: "apex/jet-craft-demo".into(),
		tag: "v1.2.3".into(),
	};
	let confirmation = CraftInstallationConfirmation {
		source: source.clone(),
		repository: "https://github.com/apex/jet-craft-demo".into(),
		publisher_claim: "Example Org".into(),
		commit: "0123456789abcdef0123456789abcdef01234567".into(),
		artifact_sha256: "ab".repeat(32),
		broker_permissions: vec![BrokerPermission::ArtifactRead],
		host_access: vec![CraftHostAccess::Network {
			destination: "api.example.com".into(),
		}],
		trust: CraftTrust::SameUserExecutable,
	};
	for message in [
		ClientMessage::Query {
			id: 1,
			query: QueryRequest::DiscoverCraft { source },
			timeout_ms: None,
		},
		ClientMessage::Command {
			id: 2,
			command_id: uuid::Uuid::nil(),
			command: CommandRequest::InstallCraft { confirmation },
		},
	] {
		let encoded = encode_control(&message).unwrap();
		assert_eq!(decode_control::<ClientMessage>(&encoded).unwrap(), message);
	}
}

#[test]
fn discovery_accepts_the_shared_fixture_shape() {
	let payload = br#"{"kind":"query","id":5,"query":{"type":"discover_craft","source":{"type":"github_release","repository":"apex/jet-craft-demo","tag":"v1.2.3"}}}"#;
	decode_control::<ClientMessage>(payload).unwrap();
}

#[test]
fn unknown_confirmation_authority_is_rejected() {
	let payload = br#"{
		"kind":"command",
		"id":2,
		"command_id":"00000000-0000-0000-0000-000000000000",
		"command":{
			"type":"install_craft",
			"confirmation":{
				"source":{"type":"github_release","repository":"apex/jet-craft-demo","tag":"v1"},
				"repository":"https://github.com/apex/jet-craft-demo",
				"publisher_claim":"Example Org",
				"commit":"0123456789abcdef0123456789abcdef01234567",
				"artifact_sha256":"abababababababababababababababababababababababababababababababab",
				"broker_permissions":["future_permission"],
				"host_access":[],
				"trust":"same_user_executable"
			}
		}
	}"#;
	assert!(decode_control::<ClientMessage>(payload).is_err());
}
