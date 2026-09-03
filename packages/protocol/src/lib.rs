//! Versioned Jet protocol wire types and bounded framing.
//!
//! This crate owns the wire representation shared by `jetd` and every Jet
//! client. It knows nothing about the core domain model: `jetd` translates
//! between domain types and these DTOs at the transport seam (ADR-0049).
//!
//! The v1 protocol is a small length-prefixed envelope carrying strict JSON
//! control frames and raw binary data frames (ADR-0089). Every connection
//! starts with a fixed preface and a restricted handshake (ADR-0090).

mod control;
mod conversation;
mod event;
mod frame;
mod handshake;
mod message;

pub use control::{
	ControlError, MAX_NESTING_DEPTH, decode_control, encode_control,
};
pub use conversation::{
	CommandRequest, CommandResponse, ConflictState, Conversation,
	ConversationList, ConversationSnapshot, Retention, RevisionConflict, Run,
	RunLifecycle,
};
pub use event::{Actor, Event};
pub use frame::{
	Frame, FrameError, FrameKind, FrameLimits, FrameReader, FrameWriter,
	MAX_CONTROL_FRAME, MAX_DATA_FRAME,
};
pub use handshake::{
	CODEC_JSON_V1, ClientHello, PREFACE, PROTOCOL_VERSION, ServerHello,
	VersionRange,
};
pub use message::{
	ClientMessage, ErrorCategory, PlaneStatus, QueryRequest, QueryResponse,
	RequestId, ServerMessage, WireError,
};
