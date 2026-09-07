//! Explicit translation of execution control values into wire values.
use jet_core as core;
use jet_protocol as wire;

pub(super) fn control(value: core::RunControl) -> wire::RunControl {
	match value {
		core::RunControl::InterruptTurn => wire::RunControl::InterruptTurn,
		core::RunControl::StopRun => wire::RunControl::StopRun,
	}
}

pub(super) fn termination(value: core::RunTermination) -> wire::RunTermination {
	wire::RunTermination {
		control: control(value.control),
		stage: match value.stage {
			core::TerminationStage::NativeCancellation => {
				wire::TerminationStage::NativeCancellation
			}
			core::TerminationStage::Interrupt => {
				wire::TerminationStage::Interrupt
			}
			core::TerminationStage::Terminate => {
				wire::TerminationStage::Terminate
			}
			core::TerminationStage::Kill => wire::TerminationStage::Kill,
			core::TerminationStage::Unobserved => {
				wire::TerminationStage::Unobserved
			}
		},
	}
}
