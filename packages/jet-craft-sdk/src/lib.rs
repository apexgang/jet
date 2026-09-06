//! SDK for building Jet Crafts.
//!
//! Crafts are out-of-process Harness adapters that speak the language-neutral
//! Craft protocol (ADR-0052). This crate is the stability seam they compile
//! against. See `docs/craft-protocol.md` for the language-neutral contract.

mod connection;
mod specification;

pub use connection::{CraftConnection, CraftError, CraftReceiver, CraftSender};
pub use jet_protocol::{
	CraftAction, CraftCommand, CraftEvent, Presentation, PresentationAction,
	PresentationBlock,
};
pub use specification::parse_specification;
