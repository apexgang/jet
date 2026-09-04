//! Rust client for the Jet protocol.
//!
//! GUI clients and tests use this crate to talk to `jetd`; it depends only
//! on `jet-protocol` and never links the core or SQLite (ADR-0050,
//! ADR-0057).

mod connection;
mod requests;

pub use connection::{Client, ClientError};
