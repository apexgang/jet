//! Wire form of registered Projects (ADR-0025, ADR-0101).
//!
//! A Project is registered through an explicit Path grant: the one request
//! in this protocol that carries an absolute path. Every other file
//! operation names a Project and a relative path.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::Actor;

/// One registered Project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
	/// Durable identity.
	pub project_id: Uuid,
	/// The canonical absolute root of its working tree, as the Plane
	/// resolved the granted path.
	pub root: String,
	/// The interactive Actor whose Path grant registered it.
	pub registered_by: Actor,
	/// When it was registered, in signed Unix milliseconds.
	pub registered_at_unix_ms: i64,
}

/// Every registered Project on one Plane, fenced by a journal cursor
/// (ADR-0092).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectList {
	/// Newest Event sequence visible when the snapshot was read, carried as
	/// a decimal string (ADR-0089).
	#[serde(with = "crate::decimal")]
	pub cursor: u64,
	/// The Projects in the order they were registered.
	pub projects: Vec<Project>,
}
