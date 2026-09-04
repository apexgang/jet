//! Rebuilds the crate when a migration is added.
//!
//! `sqlx::migrate!` emits an `include_str!` per migration file, so editing
//! an existing migration already forces a rebuild. Nothing tracks a *new*
//! file, so without this the daemon would ship an embedded migration set
//! that silently omits it.

fn main() {
	println!("cargo:rerun-if-changed=migrations");
}
