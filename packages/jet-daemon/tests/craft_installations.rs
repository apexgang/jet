//! Black-box third-party Craft installation conformance at the public Jet
//! protocol boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use jet_protocol::{
	CapabilityObservation, CraftSource, CraftTrust, SettingKey, SettingScope,
	SettingValue,
};
use pretty_assertions::assert_eq;
use sha2::{Digest, Sha256};
use support::{connect, start_jetd};
use uuid::Uuid;

#[tokio::test]
async fn developer_mode_installation_is_explicit_audited_and_persistent() {
	let dir = tempfile::tempdir().unwrap();
	let home = dir.path().join(".jet");
	let artifact = b"local third-party Craft";
	let sha256 = format!("{:x}", Sha256::digest(artifact));
	let artifact_name = format!(
		"jet-craft-demo-{}-{}",
		std::env::consts::OS,
		std::env::consts::ARCH
	);
	let artifact_path = dir.path().join(&artifact_name);
	let specification_path = dir.path().join("craft-spec.toml");
	tokio::fs::write(&artifact_path, artifact).await.unwrap();
	tokio::fs::write(
		&specification_path,
		format!(
			r#"schema = {{ major = 1, minor = 0 }}
id = "demo"
harness = "demo"
version = "1.0.0"
publisher = "Example Org"
broker_permissions = ["artifact_read"]
host_access = [{{ kind = "network", destination = "api.example.com" }}]
features = [{{ name = "turns", required = true }}]

[protocol]
family = "craft"
versions = [{{ major = 1, minor = 1 }}]
capabilities = ["runs"]

[[artifacts]]
name = "{artifact_name}"
operating_system = "{}"
architecture = "{}"
sha256 = "{sha256}"
"#,
			std::env::consts::OS,
			std::env::consts::ARCH,
		),
	)
	.await
	.unwrap();
	let client_id = Uuid::new_v4();
	let mut first = start_jetd(&home).await;
	let client = connect(&first, client_id).await;
	client
		.set_setting(
			Uuid::now_v7(),
			SettingKey::DeveloperMode,
			SettingScope::Plane,
			SettingValue::Flag(true),
		)
		.await
		.unwrap();
	let preview = client
		.discover_craft(CraftSource::Local {
			specification: specification_path.display().to_string(),
			artifact: artifact_path.display().to_string(),
		})
		.await
		.unwrap();
	assert_eq!(preview.confirmation.trust, CraftTrust::DeveloperSource);
	assert_eq!(preview.confirmation.artifact_sha256, sha256);
	let queued = client
		.install_craft(Uuid::now_v7(), preview.confirmation)
		.await
		.unwrap();
	assert_eq!(
		(queued.craft_id.as_str(), queued.version.as_str()),
		("demo", "1.0.0")
	);
	let capabilities = client
		.capabilities(CapabilityObservation::Fresh)
		.await
		.unwrap();
	assert!(
		capabilities
			.crafts
			.iter()
			.any(|craft| craft.craft_id == "demo")
	);
	let audit = client.security_audit_after(0).await.unwrap();
	assert!(
		audit
			.entries
			.iter()
			.any(|entry| entry.decision == "craft.installation_approved")
	);
	let updated_artifact = b"updated third-party Craft";
	let updated_digest = format!("{:x}", Sha256::digest(updated_artifact));
	tokio::fs::write(&artifact_path, updated_artifact)
		.await
		.unwrap();
	let specification = tokio::fs::read_to_string(&specification_path)
		.await
		.unwrap();
	tokio::fs::write(
		&specification_path,
		specification
			.replace("1.0.0", "2.0.0")
			.replace(&sha256, &updated_digest),
	)
	.await
	.unwrap();
	let update = client
		.discover_craft(CraftSource::Local {
			specification: specification_path.display().to_string(),
			artifact: artifact_path.display().to_string(),
		})
		.await
		.unwrap();
	let updated = client
		.install_craft(Uuid::now_v7(), update.confirmation)
		.await
		.unwrap();
	assert_eq!(
		(updated.version.as_str(), updated.artifact_sha256.as_str()),
		("2.0.0", updated_digest.as_str())
	);
	first.child.kill().await.unwrap();

	let second = start_jetd(&home).await;
	let client = connect(&second, client_id).await;
	let capabilities = client
		.capabilities(CapabilityObservation::Fresh)
		.await
		.unwrap();
	assert!(
		capabilities
			.crafts
			.iter()
			.any(|craft| craft.craft_id == "demo" && craft.version == "2.0.0")
	);
}
