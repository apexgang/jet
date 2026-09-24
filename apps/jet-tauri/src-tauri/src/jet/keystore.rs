//! This computer's Ed25519 client identity for remote Planes (ADR-0017).
//!
//! ADR-0076 governs where the private seed lives: only in the OS Secret
//! Service, after a create/read/delete probe proved the store works, or in
//! process memory for an explicit session-only pairing. There is never a file
//! fallback, the seed is never exported, and the webview never receives the
//! seed, a proof or a signature. The durable seed is created lazily at the
//! first claim, so local-only use never touches the keyring.
//!
//! The identity is unlocked before ssh starts: the server nonce is valid for
//! 10 s and the handshake times out after 15 s, so a keyring prompt inside
//! `sign()` would always lose. `HandshakeIdentity` therefore holds an already
//! loaded key for exactly one handshake or one pairing signature and wipes it
//! when dropped.
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{errors::PublicError, pairing_transcript::ValidatedTranscript};

/// The prefix `jet_protocol::connection_signing_bytes` puts on every remote
/// login transcript. `sign()` refuses anything else (domain separation from
/// pairing transcripts, which are signed only through `sign_pairing`).
const CONNECTION_DOMAIN: &[u8] = b"jet.connection.v1\0ed25519\0";
/// The login transcript is a domain, one encoded hello and a 32-byte nonce.
const MAXIMUM_CONNECTION_TRANSCRIPT: usize = 4096;

pub(crate) type Seed = Zeroizing<[u8; 32]>;
pub(crate) type StoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, KeyStoreError>> + Send + 'a>>;

/// Where a remote Plane's pairing key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Credential {
    /// In Secret Service; survives restarts.
    Durable,
    /// In process memory only; ends when Jet quits (ADR-0076).
    Session,
}

impl Credential {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::Session => "session",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyStoreError {
    /// No Secret Service on the bus, or a create/read/delete step failed.
    Unavailable,
    /// The unlock prompt was dismissed or timed out.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Locked,
    /// This build has no supported secure store (every non-Linux build).
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Unsupported,
}

impl KeyStoreError {
    pub(crate) fn public(self) -> PublicError {
        match self {
            Self::Unavailable => PublicError::unavailable(
                "identity.secret_store_unavailable",
                "Jet couldn't use a system keyring for this computer's pairing key.",
                true,
            ),
            Self::Locked => PublicError::unavailable(
                "identity.secret_store_locked",
                "Unlock your keyring, then try again.",
                true,
            ),
            Self::Unsupported => PublicError::unavailable(
                "identity.unsupported_platform",
                "This computer has no supported secure storage for a pairing key.",
                false,
            ),
        }
    }

    fn key_state(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Locked => "locked",
            Self::Unsupported => "unsupported",
        }
    }
}

pub(crate) fn key_missing() -> PublicError {
    PublicError::conflict(
        "identity.key_missing",
        "This computer's pairing key is missing. Pair again.",
    )
}

pub(crate) fn session_ended() -> PublicError {
    PublicError::conflict(
        "identity.session_ended",
        "This computer paired for one session only. Pair again to reconnect.",
    )
}

/// A store for the raw 32-byte seed, keyed by this installation's client ID.
pub(crate) trait KeyStore: Send + Sync {
    /// ADR-0076: create, find, read back and delete a throwaway item.
    fn probe(&self) -> StoreFuture<'_, ()>;
    /// Whether a seed exists, without unlocking anything.
    fn contains(&self, client_id: Uuid) -> StoreFuture<'_, bool>;
    /// The seed, unlocking the store (with its prompt) if needed.
    fn load(&self, client_id: Uuid) -> StoreFuture<'_, Option<Seed>>;
    fn save<'a>(&'a self, client_id: Uuid, seed: &'a Seed) -> StoreFuture<'a, ()>;
}

/// Process-memory seeds for explicit session-only pairing (and tests). Never
/// written anywhere; dropped (and wiped) when Jet quits.
#[derive(Default)]
pub(crate) struct SessionKeyStore {
    seeds: Mutex<HashMap<Uuid, Seed>>,
}

impl KeyStore for SessionKeyStore {
    fn probe(&self) -> StoreFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn contains(&self, client_id: Uuid) -> StoreFuture<'_, bool> {
        let found = self
            .seeds
            .lock()
            .map(|seeds| seeds.contains_key(&client_id))
            .map_err(|_| KeyStoreError::Unavailable);
        Box::pin(async move { found })
    }

    fn load(&self, client_id: Uuid) -> StoreFuture<'_, Option<Seed>> {
        let seed = self
            .seeds
            .lock()
            .map(|seeds| seeds.get(&client_id).map(|seed| Zeroizing::new(**seed)))
            .map_err(|_| KeyStoreError::Unavailable);
        Box::pin(async move { seed })
    }

    fn save<'a>(&'a self, client_id: Uuid, seed: &'a Seed) -> StoreFuture<'a, ()> {
        let saved = self
            .seeds
            .lock()
            .map(|mut seeds| {
                seeds.insert(client_id, Zeroizing::new(**seed));
            })
            .map_err(|_| KeyStoreError::Unavailable);
        Box::pin(async move { saved })
    }
}

impl fmt::Debug for SessionKeyStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionKeyStore { .. }")
    }
}

/// The durable store for this platform.
pub(crate) fn platform_store() -> Arc<dyn KeyStore> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(secret_service_store::SecretServiceKeyStore)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Arc::new(UnsupportedKeyStore)
    }
}

#[cfg(not(target_os = "linux"))]
struct UnsupportedKeyStore;

#[cfg(not(target_os = "linux"))]
impl KeyStore for UnsupportedKeyStore {
    fn probe(&self) -> StoreFuture<'_, ()> {
        Box::pin(async { Err(KeyStoreError::Unsupported) })
    }
    fn contains(&self, _: Uuid) -> StoreFuture<'_, bool> {
        Box::pin(async { Err(KeyStoreError::Unsupported) })
    }
    fn load(&self, _: Uuid) -> StoreFuture<'_, Option<Seed>> {
        Box::pin(async { Err(KeyStoreError::Unsupported) })
    }
    fn save<'a>(&'a self, _: Uuid, _: &'a Seed) -> StoreFuture<'a, ()> {
        Box::pin(async { Err(KeyStoreError::Unsupported) })
    }
}

#[cfg(target_os = "linux")]
mod secret_service_store {
    use std::{collections::HashMap, future::Future, time::Duration};

    use secret_service::{EncryptionType, Error, SecretService};
    use uuid::Uuid;
    use zeroize::Zeroizing;

    use super::{same_bytes, KeyStore, KeyStoreError, Seed, StoreFuture};

    const APPLICATION: &str = "me.heeka.jet-tauri";
    const CONTENT_TYPE: &str = "application/octet-stream";
    /// A D-Bus round trip that needs no human.
    const BUS_LIMIT: Duration = Duration::from_secs(10);
    /// A step that can show the keyring's unlock prompt.
    const PROMPT_LIMIT: Duration = Duration::from_secs(60);

    /// Freedesktop Secret Service (GNOME Keyring, KWallet, KeePassXC) over
    /// D-Bus with an encrypted session.
    pub(super) struct SecretServiceKeyStore;

    fn map(error: Error) -> KeyStoreError {
        match error {
            Error::Locked | Error::Prompt | Error::PromptDisconnected => KeyStoreError::Locked,
            // No service, no default collection, a D-Bus or crypto failure.
            _ => KeyStoreError::Unavailable,
        }
    }

    async fn bus<T>(step: impl Future<Output = Result<T, Error>>) -> Result<T, KeyStoreError> {
        tokio::time::timeout(BUS_LIMIT, step)
            .await
            .map_err(|_| KeyStoreError::Unavailable)?
            .map_err(map)
    }

    /// A dismissed or timed-out prompt is a locked store, not a missing one.
    async fn prompt<T>(step: impl Future<Output = Result<T, Error>>) -> Result<T, KeyStoreError> {
        tokio::time::timeout(PROMPT_LIMIT, step)
            .await
            .map_err(|_| KeyStoreError::Locked)?
            .map_err(map)
    }

    fn identity_attributes(client_id: &str) -> HashMap<&str, &str> {
        HashMap::from([
            ("application", APPLICATION),
            ("jet-purpose", "client-identity"),
            ("jet-client-id", client_id),
            ("jet-key-algorithm", "ed25519"),
        ])
    }

    async fn connect() -> Result<SecretService<'static>, KeyStoreError> {
        bus(SecretService::connect(EncryptionType::Dh)).await
    }

    impl KeyStore for SecretServiceKeyStore {
        fn probe(&self) -> StoreFuture<'_, ()> {
            Box::pin(async {
                let service = connect().await?;
                let collection = bus(service.get_default_collection()).await?;
                prompt(collection.ensure_unlocked()).await?;
                let mut secret = Zeroizing::new([0u8; 16]);
                getrandom::fill(&mut secret[..]).map_err(|_| KeyStoreError::Unavailable)?;
                let probe_id = Uuid::new_v4().to_string();
                let attributes = HashMap::from([
                    ("application", APPLICATION),
                    ("jet-purpose", "probe"),
                    ("jet-probe-id", probe_id.as_str()),
                ]);
                let item = prompt(collection.create_item(
                    "Jet secure storage check",
                    attributes.clone(),
                    &secret[..],
                    true,
                    CONTENT_TYPE,
                ))
                .await?;
                let read_back = async {
                    let found = bus(service.search_items(attributes)).await?;
                    let found = found.unlocked.first().ok_or(KeyStoreError::Unavailable)?;
                    let read = Zeroizing::new(bus(found.get_secret()).await?);
                    if same_bytes(&read, &secret[..]) {
                        Ok(())
                    } else {
                        Err(KeyStoreError::Unavailable)
                    }
                }
                .await;
                // Every step must succeed, including removing the probe.
                let deleted = prompt(item.delete()).await;
                read_back?;
                deleted.map_err(|_| KeyStoreError::Unavailable)
            })
        }

        fn contains(&self, client_id: Uuid) -> StoreFuture<'_, bool> {
            Box::pin(async move {
                let service = connect().await?;
                let client_id = client_id.to_string();
                let found = bus(service.search_items(identity_attributes(&client_id))).await?;
                Ok(!found.unlocked.is_empty() || !found.locked.is_empty())
            })
        }

        fn load(&self, client_id: Uuid) -> StoreFuture<'_, Option<Seed>> {
            Box::pin(async move {
                let service = connect().await?;
                let client_id = client_id.to_string();
                let found = bus(service.search_items(identity_attributes(&client_id))).await?;
                let item = match (found.unlocked.first(), found.locked.first()) {
                    (Some(item), _) => item,
                    (None, Some(item)) => {
                        prompt(item.unlock()).await?;
                        item
                    }
                    (None, None) => return Ok(None),
                };
                // The returned buffer is wiped as soon as the seed is copied.
                let secret = Zeroizing::new(prompt(item.get_secret()).await?);
                let seed: [u8; 32] = secret
                    .as_slice()
                    .try_into()
                    .map_err(|_| KeyStoreError::Unavailable)?;
                Ok(Some(Zeroizing::new(seed)))
            })
        }

        fn save<'a>(&'a self, client_id: Uuid, seed: &'a Seed) -> StoreFuture<'a, ()> {
            Box::pin(async move {
                let service = connect().await?;
                let collection = bus(service.get_default_collection()).await?;
                prompt(collection.ensure_unlocked()).await?;
                let client_id = client_id.to_string();
                prompt(collection.create_item(
                    "Jet client identity",
                    identity_attributes(&client_id),
                    &seed[..],
                    true,
                    CONTENT_TYPE,
                ))
                .await?;
                Ok(())
            })
        }
    }
}

/// Constant-time equality for secrets of equal length.
pub(crate) fn same_bytes(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// First 16 hex characters of SHA-256 over a public key, grouped in fours.
/// Only public bytes are hashed.
pub(crate) fn fingerprint(key: &[u8; 32]) -> String {
    let digest = Sha256::digest(key);
    digest[..8]
        .chunks(2)
        .map(|pair| format!("{:02x}{:02x}", pair[0], pair[1]))
        .collect::<Vec<_>>()
        .join(" ")
}

fn public_of(seed: &Seed) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

fn generate_seed() -> Result<Seed, PublicError> {
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut seed[..]).map_err(|_| KeyStoreError::Unavailable.public())?;
    Ok(seed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Durable {
    /// Nothing has asked the store yet.
    Unknown,
    NotCreated,
    Present,
    Failed(KeyStoreError),
}

/// Public facts only: the durable store's last known state and public keys.
struct KeyState {
    durable: Durable,
    durable_public: Option<[u8; 32]>,
    session_public: Option<[u8; 32]>,
}

/// The identity key state shown in the webview. No key material.
pub(crate) struct KeyView {
    pub(crate) key: &'static str,
    pub(crate) fingerprint: Option<String>,
}

/// This installation's pairing keys: the durable store, the session store,
/// a cached probe success and cached public keys.
pub(crate) struct IdentityKeys {
    durable: Arc<dyn KeyStore>,
    session: SessionKeyStore,
    probed: AtomicBool,
    state: Mutex<KeyState>,
}

impl fmt::Debug for IdentityKeys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("IdentityKeys { .. }")
    }
}

impl IdentityKeys {
    pub(crate) fn new(durable: Arc<dyn KeyStore>) -> Self {
        Self {
            durable,
            session: SessionKeyStore::default(),
            probed: AtomicBool::new(false),
            state: Mutex::new(KeyState {
                durable: Durable::Unknown,
                durable_public: None,
                session_public: None,
            }),
        }
    }

    fn record(&self, change: impl FnOnce(&mut KeyState)) {
        if let Ok(mut state) = self.state.lock() {
            change(&mut state);
        }
    }

    fn failed(&self, error: KeyStoreError) -> PublicError {
        self.record(|state| state.durable = Durable::Failed(error));
        error.public()
    }

    /// ADR-0076 probe. A success is cached for the process; a failure is not,
    /// so **Check again** re-runs it.
    pub(crate) async fn probe(&self) -> Result<(), PublicError> {
        if self.probed.load(Ordering::Acquire) {
            return Ok(());
        }
        match self.durable.probe().await {
            Ok(()) => {
                self.probed.store(true, Ordering::Release);
                self.record(|state| {
                    if let Durable::Failed(_) | Durable::Unknown = state.durable {
                        state.durable = Durable::NotCreated;
                    }
                });
                Ok(())
            }
            Err(error) => Err(self.failed(error)),
        }
    }

    /// Whether a seed exists for `credential`, without unlocking anything.
    pub(crate) async fn has_seed(&self, client_id: Uuid, credential: Credential) -> bool {
        match credential {
            Credential::Session => self.session.contains(client_id).await.unwrap_or(false),
            Credential::Durable => match self.durable.contains(client_id).await {
                Ok(found) => {
                    self.record(|state| {
                        state.durable = if found {
                            Durable::Present
                        } else {
                            Durable::NotCreated
                        };
                    });
                    found
                }
                Err(error) => {
                    self.failed(error);
                    false
                }
            },
        }
    }

    /// The public key for `credential`, creating its seed first if needed.
    /// A durable seed is created only after a successful probe.
    pub(crate) async fn ensure_public(
        &self,
        client_id: Uuid,
        credential: Credential,
    ) -> Result<[u8; 32], PublicError> {
        match credential {
            Credential::Session => {
                let seed = match self.session.load(client_id).await {
                    Ok(Some(seed)) => seed,
                    _ => {
                        let seed = generate_seed()?;
                        self.session
                            .save(client_id, &seed)
                            .await
                            .map_err(KeyStoreError::public)?;
                        seed
                    }
                };
                let public = public_of(&seed);
                self.record(|state| state.session_public = Some(public));
                Ok(public)
            }
            Credential::Durable => {
                let loaded = self
                    .durable
                    .load(client_id)
                    .await
                    .map_err(|error| self.failed(error))?;
                let seed = match loaded {
                    Some(seed) => seed,
                    None => {
                        self.probe().await?;
                        let seed = generate_seed()?;
                        self.durable
                            .save(client_id, &seed)
                            .await
                            .map_err(|error| self.failed(error))?;
                        seed
                    }
                };
                let public = public_of(&seed);
                self.record(|state| {
                    state.durable = Durable::Present;
                    state.durable_public = Some(public);
                });
                Ok(public)
            }
        }
    }

    /// Loads the seed (showing the keyring prompt if needed) before any ssh
    /// process starts, and returns a key scoped to one handshake.
    pub(crate) async fn unlock_for_handshake(
        &self,
        client_id: Uuid,
        credential: Credential,
    ) -> Result<HandshakeIdentity, PublicError> {
        let seed = match credential {
            Credential::Session => self
                .session
                .load(client_id)
                .await
                .ok()
                .flatten()
                .ok_or_else(session_ended)?,
            Credential::Durable => match self
                .durable
                .load(client_id)
                .await
                .map_err(|error| self.failed(error))?
            {
                Some(seed) => seed,
                None => {
                    self.record(|state| state.durable = Durable::NotCreated);
                    return Err(key_missing());
                }
            },
        };
        let key = SigningKey::from_bytes(&seed);
        let public = key.verifying_key().to_bytes();
        self.record(|state| match credential {
            Credential::Session => state.session_public = Some(public),
            Credential::Durable => {
                state.durable = Durable::Present;
                state.durable_public = Some(public);
            }
        });
        Ok(HandshakeIdentity { client_id, key })
    }

    /// Key state and fingerprint for the webview.
    pub(crate) fn view(&self) -> KeyView {
        let Ok(state) = self.state.lock() else {
            return KeyView {
                key: "unknown",
                fingerprint: None,
            };
        };
        let (key, public) = match (state.durable, state.session_public) {
            (Durable::Present, _) => ("present", state.durable_public),
            (_, Some(session)) => ("session_only", Some(session)),
            (Durable::NotCreated, None) => ("not_created", None),
            (Durable::Failed(error), None) => (error.key_state(), None),
            (Durable::Unknown, None) => ("unknown", None),
        };
        KeyView {
            key,
            fingerprint: public.as_ref().map(fingerprint),
        }
    }
}

/// A key loaded for exactly one remote login or one pairing signature. The
/// `SigningKey` zeroizes itself on drop.
pub(crate) struct HandshakeIdentity {
    client_id: Uuid,
    key: SigningKey,
}

impl fmt::Debug for HandshakeIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HandshakeIdentity")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl HandshakeIdentity {
    pub(crate) fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// Signs only a remote-login transcript. A pairing transcript, or any
    /// other message a hostile endpoint might want signed, is refused.
    fn sign_connection(&self, transcript: &[u8]) -> io::Result<[u8; 64]> {
        if transcript.len() > MAXIMUM_CONNECTION_TRANSCRIPT
            || !transcript.starts_with(CONNECTION_DOMAIN)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refused to sign a non-connection transcript",
            ));
        }
        Ok(self.key.sign(transcript).to_bytes())
    }

    /// Signs a pairing transcript that `pairing_transcript::validate` has
    /// already checked against this installation and the claimed offer.
    pub(crate) fn sign_pairing(&self, transcript: &ValidatedTranscript) -> [u8; 64] {
        self.key.sign(transcript.bytes()).to_bytes()
    }
}

impl jet_client::ClientIdentity for HandshakeIdentity {
    fn client_id(&self) -> Uuid {
        self.client_id
    }

    fn sign(&self, transcript: &[u8]) -> impl Future<Output = io::Result<[u8; 64]>> + Send {
        // No I/O here: the key was unlocked before ssh started.
        std::future::ready(self.sign_connection(transcript))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::AtomicUsize;

    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use jet_client::ClientIdentity;

    use super::*;

    /// Which step of the probe a fake store fails at.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Fail {
        Nothing,
        Create,
        Read,
        Delete,
        Locked,
    }

    /// A durable-store fake that counts every access and can fail each probe
    /// step, the way Secret Service reports them.
    pub(crate) struct CountingStore {
        pub(crate) inner: SessionKeyStore,
        pub(crate) fail: Mutex<Fail>,
        pub(crate) accesses: AtomicUsize,
    }

    impl CountingStore {
        pub(crate) fn new(fail: Fail) -> Self {
            Self {
                inner: SessionKeyStore::default(),
                fail: Mutex::new(fail),
                accesses: AtomicUsize::new(0),
            }
        }

        fn fail(&self) -> Fail {
            *self.fail.lock().unwrap()
        }

        pub(crate) fn count(&self) -> usize {
            self.accesses.load(Ordering::SeqCst)
        }
    }

    impl KeyStore for CountingStore {
        fn probe(&self) -> StoreFuture<'_, ()> {
            self.accesses.fetch_add(1, Ordering::SeqCst);
            let result = match self.fail() {
                Fail::Nothing => Ok(()),
                // Each of create, read-back and delete failing is the same
                // outcome for the user: the store cannot be trusted.
                Fail::Create | Fail::Read | Fail::Delete => Err(KeyStoreError::Unavailable),
                Fail::Locked => Err(KeyStoreError::Locked),
            };
            Box::pin(async move { result })
        }
        fn contains(&self, client_id: Uuid) -> StoreFuture<'_, bool> {
            self.accesses.fetch_add(1, Ordering::SeqCst);
            self.inner.contains(client_id)
        }
        fn load(&self, client_id: Uuid) -> StoreFuture<'_, Option<Seed>> {
            self.accesses.fetch_add(1, Ordering::SeqCst);
            if self.fail() == Fail::Locked {
                return Box::pin(async { Err(KeyStoreError::Locked) });
            }
            self.inner.load(client_id)
        }
        fn save<'a>(&'a self, client_id: Uuid, seed: &'a Seed) -> StoreFuture<'a, ()> {
            self.accesses.fetch_add(1, Ordering::SeqCst);
            self.inner.save(client_id, seed)
        }
    }

    const CLIENT: Uuid = Uuid::from_u128(7);

    #[tokio::test]
    async fn session_store_creates_loads_and_probes() {
        let store = SessionKeyStore::default();
        store.probe().await.unwrap();
        assert!(!store.contains(CLIENT).await.unwrap());
        assert!(store.load(CLIENT).await.unwrap().is_none());
        let seed = Zeroizing::new([9u8; 32]);
        store.save(CLIENT, &seed).await.unwrap();
        assert!(store.contains(CLIENT).await.unwrap());
        assert_eq!(*store.load(CLIENT).await.unwrap().unwrap(), [9u8; 32]);
        assert_eq!(format!("{store:?}"), "SessionKeyStore { .. }");
    }

    #[tokio::test]
    async fn a_probe_failure_at_any_step_means_the_store_is_unavailable() {
        for fail in [Fail::Create, Fail::Read, Fail::Delete] {
            let keys = IdentityKeys::new(Arc::new(CountingStore::new(fail)));
            let error = keys.probe().await.unwrap_err();
            assert_eq!(error.code, "identity.secret_store_unavailable");
            assert!(error.retryable);
            assert_eq!(keys.view().key, "unavailable");
            // A failure is not cached: the next probe asks the store again.
            let error = keys
                .ensure_public(CLIENT, Credential::Durable)
                .await
                .unwrap_err();
            assert_eq!(error.code, "identity.secret_store_unavailable");
        }
        let keys = IdentityKeys::new(Arc::new(CountingStore::new(Fail::Locked)));
        assert_eq!(
            keys.probe().await.unwrap_err().code,
            "identity.secret_store_locked"
        );
        assert_eq!(keys.view().key, "locked");
    }

    #[tokio::test]
    async fn a_durable_seed_is_created_only_after_a_successful_probe() {
        let store = Arc::new(CountingStore::new(Fail::Nothing));
        let keys = IdentityKeys::new(store.clone());
        assert_eq!(keys.view().key, "unknown");
        assert!(!keys.has_seed(CLIENT, Credential::Durable).await);
        assert_eq!(keys.view().key, "not_created");
        let error = keys
            .unlock_for_handshake(CLIENT, Credential::Durable)
            .await
            .unwrap_err();
        assert_eq!(error.code, "identity.key_missing");

        let public = keys
            .ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        let view = keys.view();
        assert_eq!(view.key, "present");
        assert_eq!(view.fingerprint.as_deref(), Some(&*fingerprint(&public)));
        assert_eq!(
            keys.ensure_public(CLIENT, Credential::Durable)
                .await
                .unwrap(),
            public,
            "the seed is created once"
        );
        let identity = keys
            .unlock_for_handshake(CLIENT, Credential::Durable)
            .await
            .unwrap();
        assert_eq!(identity.public_key(), public);
        // The probe ran once and its success is cached.
        let before = store.count();
        keys.probe().await.unwrap();
        assert_eq!(store.count(), before);
    }

    #[tokio::test]
    async fn session_only_keys_never_touch_the_durable_store() {
        let store = Arc::new(CountingStore::new(Fail::Create));
        let keys = IdentityKeys::new(store.clone());
        let error = keys
            .unlock_for_handshake(CLIENT, Credential::Session)
            .await
            .unwrap_err();
        assert_eq!(error.code, "identity.session_ended");
        let public = keys
            .ensure_public(CLIENT, Credential::Session)
            .await
            .unwrap();
        assert_eq!(keys.view().key, "session_only");
        assert!(keys.has_seed(CLIENT, Credential::Session).await);
        let identity = keys
            .unlock_for_handshake(CLIENT, Credential::Session)
            .await
            .unwrap();
        assert_eq!(identity.public_key(), public);
        assert_eq!(store.count(), 0);
    }

    #[tokio::test]
    async fn handshake_signing_is_domain_separated_bounded_and_offline() {
        let store = Arc::new(CountingStore::new(Fail::Nothing));
        let keys = IdentityKeys::new(store.clone());
        keys.ensure_public(CLIENT, Credential::Durable)
            .await
            .unwrap();
        let identity = keys
            .unlock_for_handshake(CLIENT, Credential::Durable)
            .await
            .unwrap();
        let accesses = store.count();

        let mut transcript = CONNECTION_DOMAIN.to_vec();
        transcript.extend([1u8; 64]);
        let signature = identity.sign(&transcript).await.unwrap();
        let verifying = VerifyingKey::from_bytes(&identity.public_key()).unwrap();
        verifying
            .verify(&transcript, &Signature::from_bytes(&signature))
            .unwrap();

        let mut pairing = b"jet.pairing.transcript.v1\0".to_vec();
        pairing.extend([0u8; 120]);
        assert!(identity.sign(&pairing).await.is_err());
        let mut oversized = CONNECTION_DOMAIN.to_vec();
        oversized.resize(MAXIMUM_CONNECTION_TRANSCRIPT + 1, 0);
        assert!(identity.sign(&oversized).await.is_err());
        assert!(identity.sign(b"").await.is_err());
        assert_eq!(store.count(), accesses, "signing performs no store access");
        assert!(!format!("{identity:?}").contains("key"));
    }

    #[test]
    fn fingerprints_are_sixteen_grouped_hex_characters_of_the_key_digest() {
        // SHA-256 of 32 zero bytes starts 66687aadf862bd77.
        assert_eq!(fingerprint(&[0; 32]), "6668 7aad f862 bd77");
        assert_ne!(fingerprint(&[1; 32]), fingerprint(&[0; 32]));
    }

    /// Manual check against the desktop session's real Secret Service. It
    /// creates, reads back and deletes one probe item (and may show the
    /// keyring's unlock prompt), so it never runs in the automated gates.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "touches the desktop session's Secret Service; run by hand"]
    async fn the_real_secret_service_probe_round_trips() {
        platform_store().probe().await.unwrap();
    }

    #[test]
    fn constant_time_equality_compares_length_and_content() {
        assert!(same_bytes(b"12345678", b"12345678"));
        assert!(!same_bytes(b"12345678", b"12345679"));
        assert!(!same_bytes(b"1234567", b"12345678"));
    }
}
