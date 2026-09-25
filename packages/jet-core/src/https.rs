//! The one place the core starts an HTTPS client.
//!
//! reqwest is linked with `rustls-no-provider`, so TLS runs on rustls with
//! the ring crypto provider and the platform certificate verifier instead of
//! reqwest's aws-lc-rs default (ADR-0054). Building a client before a
//! process-wide provider exists panics, and `jetd` aborts on panic, so `jetd`
//! installs the provider before its runtime starts, and every client in the
//! core still starts from [`client_builder`].

use std::sync::Once;

/// Makes rustls with ring the process-wide TLS provider.
///
/// `jetd` calls this first, before its runtime can spawn work that builds an
/// HTTPS client by any path. [`client_builder`] calls it too, for tests and
/// any other process that links the core. Later calls do nothing.
pub fn install_tls_provider() {
	static PROVIDER: Once = Once::new();
	PROVIDER.call_once(|| {
		// An error means a provider was installed first; reqwest uses it.
		let _ = rustls::crypto::ring::default_provider().install_default();
	});
}

/// A client builder whose TLS provider is already installed.
#[expect(
	clippy::disallowed_methods,
	reason = "the one sanctioned reqwest client constructor"
)]
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
	install_tls_provider();
	reqwest::Client::builder()
}

#[cfg(test)]
mod tests {
	use super::{client_builder, install_tls_provider};
	use rustls::crypto::{CryptoProvider, ring};

	#[test]
	fn a_client_builds_on_the_installed_ring_provider() {
		assert!(client_builder().https_only(true).build().is_ok());
		assert!(CryptoProvider::get_default().is_some());
	}

	#[test]
	fn installing_makes_ring_the_provider_every_client_constructor_finds() {
		install_tls_provider();
		install_tls_provider();
		let installed =
			CryptoProvider::get_default().expect("a provider is installed");
		let ring = ring::default_provider();
		let suites = |provider: &CryptoProvider| {
			provider
				.cipher_suites
				.iter()
				.map(|suite| suite.suite())
				.collect::<Vec<_>>()
		};
		let groups = |provider: &CryptoProvider| {
			provider
				.kx_groups
				.iter()
				.map(|group| group.name())
				.collect::<Vec<_>>()
		};
		assert_eq!(suites(installed), suites(&ring));
		assert_eq!(groups(installed), groups(&ring));
		// Without a provider reqwest's `Default` constructors panic, and clippy
		// cannot disallow them (clippy.toml); once it is installed they build.
		assert!(reqwest::ClientBuilder::default().build().is_ok());
		drop(reqwest::Client::default());
	}
}
