//! Typed rows exchanged with the store. Enum variants carry their durable
//! column spelling, which also serves as their JSON spelling.

mod workspace;
pub use workspace::{
	NewUserEditIntent, UserEditIntentRecord, WorkingTreeRecord,
};

mod setting;
pub use setting::{SettingRecord, SettingScopeRecord};

mod event;
pub use event::{EventClass, EventRecord, NewEvent, VerifiedSnapshotCoverage};

mod effect;
pub use effect::{
	EffectKindRecord, EffectRecord, EffectSafetyRecord, EffectStateRecord,
	NewEffect,
};

mod command;
pub use command::{ActorRecord, CommandReceiptRecord, NewCommandReceipt};

mod run;
pub use run::{NewRun, RunLifecycle, RunRecord};

mod conversation;
pub(crate) use conversation::{ConversationOriginColumns, parse_name};
pub use conversation::{
	ConversationOriginRecord, ConversationPageKey, ConversationPageStart,
	ConversationRecord, NameRecord, NameSourceRecord, NewConversation,
	RetentionPolicy,
};

use crate::StoreError;
use uuid::Uuid;

/// Reports a column whose stored value no longer parses. Every conversion
/// failure inside the store is an integrity failure.
pub(crate) fn column_error(column: &str, message: String) -> StoreError {
	StoreError::Integrity(format!("column {column}: {message}"))
}

/// One fixed-width blob column, as the width its algorithm or hash fixes.
pub(crate) fn parse_bytes<const N: usize>(
	column: &str,
	bytes: Vec<u8>,
) -> Result<[u8; N], StoreError> {
	let length = bytes.len();
	bytes.try_into().map_err(|_| {
		column_error(column, format!("the value has {length} bytes"))
	})
}

pub(crate) fn parse_uuid(column: &str, text: &str) -> Result<Uuid, StoreError> {
	Uuid::parse_str(text)
		.map_err(|error| column_error(column, format!("not a UUID: {error}")))
}

pub(crate) fn parse_optional_uuid(
	column: &str,
	text: Option<&str>,
) -> Result<Option<Uuid>, StoreError> {
	text.map(|text| parse_uuid(column, text)).transpose()
}
