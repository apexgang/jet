//! Owner-only execution descriptors and OS process-start evidence (ADR-0004).
use sha2::{Digest, Sha256};
use std::{
	fs,
	io::{self, Read},
	os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
	path::Path,
};

/// Reads one bounded owner-only regular file without following its final symlink.
///
/// # Errors
/// Rejects unsafe ownership, permissions, type, or size, and filesystem failures.
pub fn read_execution_file(path: &Path) -> io::Result<Vec<u8>> {
	validate_execution_directory(path.parent().ok_or_else(invalid)?)?;
	// ASVS 5.3.2, 15.4.2: validate the opened object, not a prior path lookup.
	let file = fs::OpenOptions::new()
		.read(true)
		.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
		.open(path)?;
	let metadata = file.metadata()?;
	if !metadata.is_file() || !owner_only(&metadata) {
		return Err(invalid());
	}
	let mut bytes = Vec::new();
	file.take(65_537).read_to_end(&mut bytes)?;
	if bytes.len() > 65_536 {
		return Err(invalid());
	}
	Ok(bytes)
}

/// Validates an existing owner-only runtime directory.
///
/// # Errors
/// Rejects symlinks, foreign ownership, public access, and missing directories.
pub fn validate_execution_directory(path: &Path) -> io::Result<()> {
	let metadata = fs::symlink_metadata(path)?;
	if !metadata.is_dir() || !owner_only(&metadata) {
		return Err(invalid());
	}
	Ok(())
}

fn owner_only(metadata: &fs::Metadata) -> bool {
	metadata.uid() == rustix::process::geteuid().as_raw()
		&& metadata.permissions().mode() & 0o077 == 0
}

/// Captures a kernel-observed process start and executable name. `None` proves
/// the process is gone; probe failures never count as death.
///
/// # Errors
/// Returns an error if the process cannot be inspected reliably.
pub fn execution_process_identity(pid: u32) -> io::Result<Option<String>> {
	let pid = rustix::process::Pid::from_raw(
		i32::try_from(pid).map_err(|_| invalid())?,
	)
	.ok_or_else(invalid)?;
	match rustix::process::test_kill_process(pid) {
		Err(rustix::io::Errno::SRCH) => return Ok(None),
		Err(error) => return Err(error.into()),
		Ok(()) => {}
	}
	// ASVS 1.2.5: fixed program and flags, with a typed positive PID.
	let output = std::process::Command::new("/bin/ps")
		.env("LC_ALL", "C")
		.args([
			"-p",
			&pid.as_raw_nonzero().get().to_string(),
			"-o",
			"stat=",
			"-o",
			"lstart=",
			"-o",
			"comm=",
		])
		.output()?;
	if !output.status.success() || output.stdout.is_empty() {
		return match rustix::process::test_kill_process(pid) {
			Err(rustix::io::Errno::SRCH) => Ok(None),
			Ok(()) | Err(_) => Err(invalid()),
		};
	}
	let text = String::from_utf8(output.stdout).map_err(|_| invalid())?;
	let (status, identity) = text
		.trim_start()
		.split_once(char::is_whitespace)
		.ok_or_else(invalid)?;
	if status.starts_with('Z') {
		return Ok(None);
	}
	Ok(Some(identity.trim_start().to_owned()))
}

/// SHA-256 identity of a deployed executable, with bounded reads.
///
/// # Errors
/// Rejects non-regular or oversized files and unreadable artifacts.
pub fn execution_digest(path: &Path) -> io::Result<String> {
	let mut file = fs::OpenOptions::new()
		.read(true)
		.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
		.open(path)?;
	if !file.metadata()?.is_file() || file.metadata()?.len() > 64 * 1024 * 1024
	{
		return Err(invalid());
	}
	let mut digest = Sha256::new();
	let mut buffer = [0; 8192];
	let mut size = 0;
	loop {
		let count = file.read(&mut buffer)?;
		if count == 0 {
			break;
		}
		size += count;
		if size > 64 * 1024 * 1024 {
			return Err(invalid());
		}
		digest.update(&buffer[..count]);
	}
	// ASVS 11.4.1: execution identity uses a collision-resistant artifact digest.
	Ok(format!("{:x}", digest.finalize()))
}
fn invalid() -> io::Error {
	io::Error::other("execution identity is unavailable or unsafe")
}

/// Independent identity of the current OS boot, suitable for an accepted Run pin.
/// Unlike a descriptor or PID, a changed boot proves an earlier execution ended.
///
/// # Errors
/// Returns an error when the kernel boot identity cannot be read reliably.
pub fn execution_boot_identity() -> io::Result<String> {
	#[cfg(target_os = "linux")]
	{
		let value = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
		let id = value.trim();
		if id.len() != 36
			|| !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
		{
			return Err(invalid());
		}
		Ok(format!("linux:{id}"))
	}
	#[cfg(target_os = "macos")]
	{
		let output = std::process::Command::new("/usr/sbin/sysctl")
			.env("LC_ALL", "C")
			.args(["-n", "kern.bootsessionuuid"])
			.output()?;
		if !output.status.success() {
			return Err(invalid());
		}
		let text = String::from_utf8(output.stdout).map_err(|_| invalid())?;
		let id = text.trim();
		if id.len() != 36
			|| !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
		{
			return Err(invalid());
		}
		// Kernel boot-session identity remains stable across wall-clock changes.
		Ok(format!("macos:{id}"))
	}
}
