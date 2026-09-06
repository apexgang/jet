//! Independent version compatibility through the public negotiation API.
use jet_protocol::{
	Negotiation, ProtocolFamily, ProtocolOffer, ProtocolVersion, decode_control,
};
use pretty_assertions::assert_eq;

#[test]
fn families_negotiate_independently_and_restarts_keep_the_execution_version() {
	for family in ["client", "craft", "helper", "specification"] {
		let local: ProtocolOffer = decode_control(format!(r#"{{"family":"{family}","versions":[{{"major":2,"minor":3}},{{"major":1,"minor":7}}],"capabilities":["actions","resume"]}}"#).as_bytes()).unwrap();
		let peer: ProtocolOffer = decode_control(format!(r#"{{"family":"{family}","versions":[{{"major":2,"minor":1}},{{"major":1,"minor":5}}],"capabilities":["resume","future"]}}"#).as_bytes()).unwrap();
		let selected =
			local.negotiate(&peer, Negotiation::NewExecution).unwrap();
		assert_eq!(
			(selected.version, selected.capabilities),
			(
				ProtocolVersion { major: 2, minor: 1 },
				vec!["resume".to_owned()]
			)
		);
		let pinned = ProtocolVersion { major: 1, minor: 4 };
		assert_eq!(
			local
				.negotiate(&peer, Negotiation::Resume(pinned))
				.unwrap()
				.version,
			pinned
		);
		let incompatible = ProtocolOffer {
			versions: vec![ProtocolVersion { major: 2, minor: 3 }],
			..peer
		};
		assert!(
			local
				.negotiate(&incompatible, Negotiation::Resume(pinned))
				.is_err()
		);
		let wrong_family = ProtocolOffer {
			family: ProtocolFamily::Craft,
			..local.clone()
		};
		if family != "craft" {
			assert!(
				local
					.negotiate(&wrong_family, Negotiation::NewExecution)
					.is_err()
			);
		}
	}
}

#[test]
fn ambiguous_or_unsupported_offers_never_select_a_version() {
	let local: ProtocolOffer = decode_control(
		br#"{"family":"craft","versions":[{"major":1,"minor":0}]}"#,
	)
	.unwrap();
	for versions in [
		"[]",
		r#"[{"major":0,"minor":0}]"#,
		r#"[{"major":1,"minor":0},{"major":1,"minor":1}]"#,
		r#"[{"major":9,"minor":0}]"#,
	] {
		let peer: ProtocolOffer = decode_control(
			format!(r#"{{"family":"craft","versions":{versions}}}"#).as_bytes(),
		)
		.unwrap();
		assert!(local.negotiate(&peer, Negotiation::NewExecution).is_err());
	}
}
