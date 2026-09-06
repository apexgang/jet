//! Specification compatibility and declaration validation at the public seam.
use jet_protocol::{CraftSpecification, decode_control};
use pretty_assertions::assert_eq;

#[test]
fn declarations_distinguish_features_permissions_and_host_access() {
	let bytes = br#"{"schema":{"major":1,"minor":0},"id":"fake","harness":"fake","protocol":{"family":"craft","versions":[{"major":1,"minor":0}]},"features":[{"name":"turns","required":true},{"name":"future","required":false}],"broker_permissions":["artifact_read"],"host_access":[{"kind":"executable","name":"fake-harness"}]}"#;
	let spec: CraftSpecification = decode_control(bytes).unwrap();
	assert_eq!(spec.enabled_features().unwrap(), vec!["turns".to_owned()]);
	let mut update = spec.clone();
	update.features[1].required = true;
	assert!(update.enabled_features().is_err());
	update = spec.clone();
	update.broker_permissions.clear();
	assert!(spec.requires_confirmation(&update));
	assert!(!update.requires_confirmation(&spec));
	update = spec.clone();
	update.host_access.clear();
	assert!(spec.requires_confirmation(&update));
	for invalid in [
		String::from_utf8(bytes.to_vec())
			.unwrap()
			.replace("artifact_read", "root_access"),
		String::from_utf8(bytes.to_vec())
			.unwrap()
			.replace("executable", "future_access"),
	] {
		assert!(
			decode_control::<CraftSpecification>(invalid.as_bytes()).is_err()
		);
	}
}
