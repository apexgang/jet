//! Rust client for the Jet protocol.
//!
//! GUI clients and tests use this crate to talk to `jetd`; it depends only
//! on `jet-protocol` and never links the core or SQLite (ADR-0050,
//! ADR-0057).

mod connection;
mod handshake;
mod import_requests;
mod pairing_requests;
mod project_requests;
mod promotion_requests;
mod requests;
mod search_requests;
mod ssh;

pub use connection::{Client, ClientError};
pub use handshake::ClientIdentity;
pub use ssh::SshEndpoint;
