//! On-demand power observations; no resident process or polling loop.
use serde::{Deserialize, Serialize};

/// Power information available from this operating system at admission time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerState {
	/// External power, with no reported low-power mode.
	Normal,
	/// Battery power or an enabled low-power mode.
	Constrained,
	/// Observation failed; admission uses the reduced budget.
	Unavailable,
}

/// Observe once, bounded to one second. Failure never enables excess work.
pub async fn observe_power() -> PowerState {
	tokio::time::timeout(std::time::Duration::from_secs(1), observe())
		.await
		.ok()
		.and_then(Result::ok)
		.unwrap_or(PowerState::Unavailable)
}

#[cfg(target_os = "macos")]
async fn observe() -> std::io::Result<PowerState> {
	async fn pmset(arguments: &[&str]) -> std::io::Result<String> {
		// ASVS 1.2.5: fixed system executable and argument arrays, never a shell.
		let output = tokio::process::Command::new("/usr/bin/pmset")
			.args(arguments)
			.kill_on_drop(true)
			.output()
			.await?;
		if !output.status.success() {
			return Err(std::io::Error::other("power observation failed"));
		}
		Ok(String::from_utf8_lossy(&output.stdout).into_owned())
	}
	let battery = pmset(&["-g", "batt"]).await?;
	let settings = pmset(&["-g"]).await?;
	if battery.contains("'Battery Power'")
		|| settings.lines().any(|line| {
			let mut fields = line.split_whitespace();
			fields.next() == Some("lowpowermode") && fields.next() == Some("1")
		}) {
		return Ok(PowerState::Constrained);
	}
	if battery.contains("'AC Power'") {
		Ok(PowerState::Normal)
	} else {
		Ok(PowerState::Unavailable)
	}
}

#[cfg(target_os = "linux")]
async fn observe() -> std::io::Result<PowerState> {
	let profile =
		tokio::fs::read_to_string("/sys/firmware/acpi/platform_profile").await;
	if profile
		.as_deref()
		.is_ok_and(|value| value.trim() == "low-power")
	{
		return Ok(PowerState::Constrained);
	}
	let mut supplies = tokio::fs::read_dir("/sys/class/power_supply").await?;
	let mut count = 0;
	while let Some(supply) = supplies.next_entry().await? {
		count += 1;
		if count > 64 {
			return Ok(PowerState::Unavailable);
		}
		let kind =
			tokio::fs::read_to_string(supply.path().join("type")).await?;
		if kind.trim() == "Battery" {
			let status =
				tokio::fs::read_to_string(supply.path().join("status")).await?;
			match status.trim() {
				"Discharging" | "Not charging" => {
					return Ok(PowerState::Constrained);
				}
				"Charging" | "Full" => {}
				_ => return Ok(PowerState::Unavailable),
			}
		}
	}
	Ok(PowerState::Normal)
}
