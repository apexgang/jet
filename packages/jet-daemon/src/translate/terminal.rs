//! Domain terminal state to versioned wire DTOs.
pub(super) fn snapshot(
	value: jet_core::WorkspaceTerminal,
) -> jet_protocol::WorkspaceTerminal {
	use jet_core::TerminalState as Domain;
	use jet_protocol::TerminalState as Wire;
	jet_protocol::WorkspaceTerminal {
		terminal_id: value.terminal_id.0,
		workspace_id: value.workspace_id.0,
		state: match value.state {
			Domain::Opening => Wire::Opening,
			Domain::Open => Wire::Open,
			Domain::Closing => Wire::Closing,
			Domain::Closed => Wire::Closed,
			Domain::Unavailable => Wire::Unavailable,
		},
	}
}
