//! Positive action validation at the trusted core (ADR-0012, ASVS 8.3.1).
//!
//! Opaque execution, including project builds/tests and Git hooks, cannot
//! be classified from a command name. Only absolute OS utilities with no
//! file, network, code-loading or policy authority are eligible here.
use serde::Deserialize;

use crate::ApprovalRequest;

/// Effect classes used to prevent equivalent workarounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
	WorkingDirectory,
	LiteralOutput,
	NoOp,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Shell {
	command: String,
	#[serde(default, rename = "threadId")]
	_thread_id: Option<String>,
	#[serde(default, rename = "turnId")]
	_turn_id: Option<String>,
	#[serde(default, rename = "itemId")]
	_item_id: Option<String>,
	#[serde(default)]
	cwd: Option<String>,
	#[serde(default, rename = "reason")]
	_reason: Option<String>,
	#[serde(default, rename = "description")]
	_description: Option<String>,
	#[serde(default)]
	timeout: Option<u32>,
}

pub(crate) fn operation(request: &ApprovalRequest) -> Option<Operation> {
	if !matches!(
		request.tool.as_str(),
		"Bash" | "item/commandExecution/requestApproval"
	) {
		return None;
	}
	// Unknown fields, duplicate keys and shell syntax never gain authority
	// from a reviewer. The absolute executable avoids PATH substitution.
	let shell: Shell = serde_json::from_str(&request.action).ok()?;
	if shell
		.command
		.bytes()
		.any(|byte| !byte.is_ascii_alphanumeric() && !b" /-_".contains(&byte))
	{
		return None;
	}
	let words: Vec<_> = shell.command.split_ascii_whitespace().collect();
	match words.as_slice() {
		["/bin/pwd"] => Some(Operation::WorkingDirectory),
		["/bin/echo", ..] => Some(Operation::LiteralOutput),
		["/usr/bin/true" | "/bin/true"] => Some(Operation::NoOp),
		_ => None,
	}
}

/// Correlation IDs identify the native attempt, not its execution authority.
/// All executable bytes and execution parameters must remain unchanged.
/// Bash requests retain their exact byte comparison.
pub(crate) fn same_action(
	first: &ApprovalRequest,
	second: &ApprovalRequest,
) -> bool {
	if first.tool != second.tool {
		return false;
	}
	if first.tool != "item/commandExecution/requestApproval" {
		return first.action == second.action;
	}
	match (
		serde_json::from_str::<Shell>(&first.action),
		serde_json::from_str::<Shell>(&second.action),
	) {
		(Ok(first), Ok(second)) => {
			first.command == second.command
				&& first.cwd == second.cwd
				&& first.timeout == second.timeout
		}
		_ => first.action == second.action,
	}
}
