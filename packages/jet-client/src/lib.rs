//! Rust client for the Jet protocol.
//!
//! GUI clients and tests use this crate to talk to `jetd`; it depends only
//! on `jet-protocol` and never links the core or SQLite (ADR-0050,
//! ADR-0057).

mod checkpoint_requests;
mod connection;
mod craft_installation_requests;
mod fork_requests;
mod handoff_requests;
mod handshake;
mod import_requests;
mod name_requests;
mod pairing_requests;
mod project_requests;
mod promotion_requests;
mod requests;
mod search_requests;
mod ssh;
mod turn_requests;
mod user_input_requests;
mod visa_requests;

pub use connection::{Client, ClientError};
pub use handshake::ClientIdentity;
pub use ssh::SshEndpoint;

mod utility_requests;

mod extension_requests;
