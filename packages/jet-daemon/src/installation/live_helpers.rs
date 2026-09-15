//! The `jetfueld` helpers alive under one Plane (ADR-0088).
//!
//! Each helper publishes `descriptor.json` in its owner-only runtime
//! directory. A descriptor whose process is gone, or whose process no
//! longer matches the start the descriptor recorded, is not a live helper;
//! a descriptor that cannot be read safely is not one either.

use jet_runtime::JetHome;
use serde::{Deserialize, Serialize};
use std::io;

const RUNTIME_DIR: &str = "runtime";
const DESCRIPTOR_FILE: &str = "descriptor.json";

/// The fields Run and terminal descriptors share.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Descriptor {
	role: String,
	pid: u32,
	process_start: String,
	version: String,
}

/// One helper whose process is alive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LiveHelper {
	/// The runtime directory name, which is the execution identity.
	pub(crate) execution: String,
	/// `run` or `terminal`.
	pub(crate) role: String,
	/// The release version the helper was deployed from.
	pub(crate) version: String,
	/// The helper's process identifier.
	pub(crate) pid: u32,
}

/// Every live helper under `home`, sorted by execution.
pub(crate) fn live_helpers(home: &JetHome) -> io::Result<Vec<LiveHelper>> {
	let entries = match std::fs::read_dir(home.root().join(RUNTIME_DIR)) {
		Ok(entries) => entries,
		Err(error) if error.kind() == io::ErrorKind::NotFound => {
			return Ok(vec![]);
		}
		Err(error) => return Err(error),
	};
	let mut live = vec![];
	for entry in entries {
		let entry = entry?;
		let Ok(bytes) = jet_runtime::read_execution_file(
			&entry.path().join(DESCRIPTOR_FILE),
		) else {
			continue;
		};
		let Ok(descriptor) = serde_json::from_slice::<Descriptor>(&bytes)
		else {
			continue;
		};
		let identity = jet_runtime::execution_process_identity(descriptor.pid)?;
		if identity.as_ref() == Some(&descriptor.process_start) {
			live.push(LiveHelper {
				execution: entry.file_name().to_string_lossy().into_owned(),
				role: descriptor.role,
				version: descriptor.version,
				pid: descriptor.pid,
			});
		}
	}
	live.sort_by(|a, b| a.execution.cmp(&b.execution));
	Ok(live)
}
