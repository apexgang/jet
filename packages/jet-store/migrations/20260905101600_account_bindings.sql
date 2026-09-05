-- Plane-local Account bindings and the opaque Credential references they
-- resolve through (ADR-0016, ADR-0076).
-- ASVS 2.2.1/2.2.3 and 14.1.4: the trusted storage layer allowlists every
-- Credential source, bounds every stored value, and has no column able to
-- hold secret material. Tokens, keys, and passwords stay in the platform
-- credential store or behind an external helper, never here.
CREATE TABLE account_bindings (
	binding_id TEXT PRIMARY KEY,
	provider TEXT NOT NULL CHECK (length(provider) <= 64),
	label TEXT NOT NULL CHECK (length(label) <= 128),
	provider_account TEXT CHECK (length(provider_account) <= 128),
	credential_source TEXT NOT NULL CHECK (
		credential_source IN (
			'platform_store', 'external_helper', 'harness_native',
			'session_only'
		)
	),
	credential_helper TEXT CHECK (length(credential_helper) <= 128),
	established_at_daemon_start INTEGER NOT NULL,
	created_at_unix_ms INTEGER NOT NULL,
	-- A helper name belongs to the external-helper source and to no other.
	CHECK (
		(credential_source = 'external_helper')
			= (credential_helper IS NOT NULL)
	)
);

-- Bindings group into one Provider account automatically only when the
-- Provider supplies a stable account identity, so that identity is unique
-- per Provider on this Plane (ADR-0016). Bindings without one are linked by
-- the user and may repeat.
CREATE UNIQUE INDEX account_bindings_provider_account
	ON account_bindings (provider, provider_account)
	WHERE provider_account IS NOT NULL;
