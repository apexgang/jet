//! Translation between core domain types and versioned wire types
//! (ADR-0049). This is the only place the two vocabularies meet; its
//! Account binding, Capability, import, Pairing, Project, and Setting
//! parts sit in the submodules beside it.

mod user_input;
use user_input::{
	file_revision, file_revision_from_wire, file_target, file_target_from_wire,
};

mod error;
pub(crate) use error::error;

mod event;
use event::event;
pub(super) use event::{actor_of, unix_ms};

mod conversation;
use conversation::{
	conversation, conversation_list, conversation_snapshot,
	lifecycle_from_wire, plane_status, retention_from_wire, run,
	working_tree_request,
};

mod outcome;
pub(crate) use outcome::command_outcome;

mod command;
pub(crate) use command::command;

mod query;
pub(crate) use query::{query, query_result};

mod account;
mod audit;
mod capability;
mod checkpoint;
mod craft_installation;
mod execution_control;
pub(crate) mod extension;
mod import;
mod name;
mod pairing;
mod project;
mod promotion;
mod run;
mod schedule;
mod search;
mod setting;
mod terminal;
mod turn;
pub(crate) mod usage;
pub(crate) mod utility;

pub(crate) use capability::snapshot as capabilities;
pub(crate) use pairing::{client as paired_client, pending as pairing_pending};

#[cfg(test)]
mod test_support;

mod auto_continue;

mod git_delivery;
