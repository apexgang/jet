//! Endpoint and host trust through the actual system SSH argument parser.
use jet_client::SshEndpoint;
use pretty_assertions::assert_eq;

#[test]
fn endpoints_cannot_inject_options_or_shell_source() {
	for endpoint in [
		"-oProxyCommand=touch /tmp/jet",
		"host;id",
		"$(id)",
		"host\nother",
		"user@",
		"@host",
	] {
		assert!(SshEndpoint::new(endpoint).is_err(), "accepted {endpoint:?}");
	}
}

#[test]
fn user_ssh_configuration_cannot_weaken_endpoint_authentication() {
	let dir = tempfile::tempdir().unwrap();
	let config = dir.path().join("config");
	std::fs::write(&config, "Host jet-test\n HostName example.invalid\n User configured-user\n StrictHostKeyChecking no\n ControlMaster auto\n ControlPath /tmp/jet-unsafe-master\n UserKnownHostsFile /tmp/jet-test-known-hosts\n").unwrap();
	let endpoint = SshEndpoint::new("jet-test").unwrap();
	let command = endpoint.command();
	let output = std::process::Command::new(command.get_program())
		.args(["-G", "-F"])
		.arg(config)
		.args(command.get_args())
		.output()
		.unwrap();
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	let settings = String::from_utf8(output.stdout).unwrap();
	let selected: Vec<_> = settings
		.lines()
		.filter(|line| {
			[
				"hostname ",
				"user ",
				"stricthostkeychecking ",
				"controlpath ",
				"userknownhostsfile ",
			]
			.iter()
			.any(|key| line.starts_with(key))
		})
		.collect();
	assert_eq!(
		selected,
		vec![
			"user configured-user",
			"hostname example.invalid",
			"stricthostkeychecking true",
			"userknownhostsfile /tmp/jet-test-known-hosts"
		]
	);
}
