//! Persisted Setting scopes and values.

use super::{column_error, parse_uuid};
use crate::StoreError;
use uuid::Uuid;

/// Where one Setting value is stored (ADR-0085). A resolution reads the
/// Plane's values and the values of the scope it addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingScopeRecord {
	/// Values that apply everywhere on the Plane.
	Plane,
	/// Values that apply inside one registered Project.
	Project {
		/// The Project the values apply to.
		project_id: Uuid,
	},
	/// Values that apply inside one Conversation.
	Conversation {
		/// The Conversation the values apply to.
		conversation_id: Uuid,
	},
}

impl SettingScopeRecord {
	pub(crate) fn columns(self) -> (&'static str, Option<Uuid>) {
		match self {
			Self::Plane => ("plane", None),
			Self::Project { project_id } => ("project", Some(project_id)),
			Self::Conversation { conversation_id } => {
				("conversation", Some(conversation_id))
			}
		}
	}

	pub(crate) fn parse(
		scope: &str,
		scope_id: Option<&str>,
	) -> Result<Self, StoreError> {
		match (scope, scope_id) {
			("plane", None) => Ok(Self::Plane),
			("project", Some(id)) => Ok(Self::Project {
				project_id: parse_uuid("scope_id", id)?,
			}),
			("conversation", Some(id)) => Ok(Self::Conversation {
				conversation_id: parse_uuid("scope_id", id)?,
			}),
			(scope, _) => Err(column_error(
				"scope",
				format!("unknown or incomplete Setting scope {scope:?}"),
			)),
		}
	}
}

/// One Setting value as one scope stores it. The core owns the key
/// vocabulary and the value encoding; the store keeps both as bounded text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRecord {
	/// Durable key spelling such as `git.auto_commit`.
	pub key: String,
	/// The scope that stores this value.
	pub scope: SettingScopeRecord,
	/// Encoded value, bounded by the store.
	pub value: String,
	/// When the value was last written.
	pub updated_at_unix_ms: i64,
}
