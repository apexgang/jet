//! The freedesktop Secret Service, spoken to over the session bus
//! (ADR-0076).
//!
//! Every call carries the `NoAutoStart` flag: a store that is installed
//! but not running is reported as unavailable rather than started, since
//! starting one can open its first-run or unlock dialog in the person's
//! session. Nothing here answers a Prompt, so a collection that needs one
//! is reported as locked. The probe item travels in a plain session: it is
//! a random value that lives for one round trip, and the session bus is a
//! socket in the user's own runtime directory.

use super::{
	KIND, PROBE_ACCOUNT, PROBE_LABEL, PROBE_SERVICE, STORE_TIMEOUT,
	probe_secret,
};
use crate::capability::{
	CredentialProbeStep, CredentialStoreStatus, CredentialStoreVerification,
};
use std::collections::HashMap;
use zbus::{
	Connection, Proxy,
	proxy::{CacheProperties, MethodFlags},
	zvariant::{Dict, ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

const SERVICE_NAME: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const DEFAULT_COLLECTION: &str = "/org/freedesktop/secrets/aliases/default";
const SERVICE_INTERFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_INTERFACE: &str = "org.freedesktop.Secret.Collection";
const ITEM_INTERFACE: &str = "org.freedesktop.Secret.Item";
const SESSION_INTERFACE: &str = "org.freedesktop.Secret.Session";
const PROMPT_INTERFACE: &str = "org.freedesktop.Secret.Prompt";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
/// The error a locked collection answers a write with.
const IS_LOCKED: &str = "org.freedesktop.Secret.Error.IsLocked";
/// The path a store answers where no Prompt is needed.
const NO_PROMPT: &str = "/";
/// The probe value is bytes with no meaning, and says so.
const PROBE_CONTENT_TYPE: &str = "application/octet-stream";

/// A Secret as the wire carries it (`(oayays)`): the session it is encoded
/// for, the encoding parameters, the value, and the value's content type.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

/// Where this Plane's session bus is, when its environment names one.
/// A `jetd` started without a session has neither the advertised address
/// nor the socket a session manager leaves in the runtime directory.
fn session_bus_address() -> Option<String> {
	if let Some(address) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS") {
		return address.into_string().ok();
	}
	let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
	let bus = std::path::Path::new(&runtime).join("bus");
	bus.exists().then(|| format!("unix:path={}", bus.display()))
}

pub(super) async fn observe() -> CredentialStoreStatus {
	match session_bus_address() {
		Some(address) => observe_at(&address).await,
		None => CredentialStoreStatus::Unavailable { kind: KIND },
	}
}

pub(super) async fn verify() -> CredentialStoreVerification {
	match session_bus_address() {
		Some(address) => verify_at(&address).await,
		None => CredentialStoreVerification::Unavailable { kind: KIND },
	}
}

/// Reads the default collection's `Locked` property over the bus at
/// `address`. A store that cannot be reached, is not running, or has no
/// default collection to keep an item in is unavailable.
async fn observe_at(address: &str) -> CredentialStoreStatus {
	match tokio::time::timeout(STORE_TIMEOUT, read_locked(address)).await {
		Ok(Ok(false)) => CredentialStoreStatus::Available { kind: KIND },
		Ok(Ok(true)) => CredentialStoreStatus::Locked { kind: KIND },
		Ok(Err(_)) | Err(_) => {
			CredentialStoreStatus::Unavailable { kind: KIND }
		}
	}
}

/// Creates, reads back, and deletes one probe item over the bus at
/// `address`.
async fn verify_at(address: &str) -> CredentialStoreVerification {
	match tokio::time::timeout(STORE_TIMEOUT, round_trip(address)).await {
		Ok(Ok(())) => CredentialStoreVerification::Verified { kind: KIND },
		Ok(Err(RoundTripFailure::Locked)) => {
			CredentialStoreVerification::Locked { kind: KIND }
		}
		Ok(Err(RoundTripFailure::Unavailable)) | Err(_) => {
			CredentialStoreVerification::Unavailable { kind: KIND }
		}
		Ok(Err(RoundTripFailure::Failed(step))) => {
			CredentialStoreVerification::Failed { kind: KIND, step }
		}
	}
}

/// Why a round trip stopped short.
enum RoundTripFailure {
	/// The store, or a default collection to keep an item in, cannot be
	/// reached.
	Unavailable,
	/// The collection is locked, or asked for a Prompt the probe does not
	/// answer.
	Locked,
	/// The store answered but did not complete the step.
	Failed(CredentialProbeStep),
}

async fn read_locked(address: &str) -> zbus::Result<bool> {
	let connection = connect(address).await?;
	let collection =
		proxy(&connection, DEFAULT_COLLECTION, PROPERTIES_INTERFACE).await?;
	let value: OwnedValue =
		call(&collection, "Get", &(COLLECTION_INTERFACE, "Locked")).await?;
	match &*value {
		Value::Bool(locked) => Ok(*locked),
		_ => Err(zbus::Error::Unsupported),
	}
}

async fn round_trip(address: &str) -> Result<(), RoundTripFailure> {
	let secret = probe_secret()
		.ok_or(RoundTripFailure::Failed(CredentialProbeStep::Create))?;
	let connection = connect(address)
		.await
		.map_err(|_| RoundTripFailure::Unavailable)?;
	let service = proxy(&connection, SERVICE_PATH, SERVICE_INTERFACE)
		.await
		.map_err(|_| RoundTripFailure::Unavailable)?;
	let (_, session): (OwnedValue, OwnedObjectPath) =
		call(&service, "OpenSession", &("plain", Value::from("")))
			.await
			.map_err(|_| RoundTripFailure::Unavailable)?;
	let collection =
		proxy(&connection, DEFAULT_COLLECTION, COLLECTION_INTERFACE)
			.await
			.map_err(|_| RoundTripFailure::Unavailable)?;

	let attributes: HashMap<&str, &str> =
		[("service", PROBE_SERVICE), ("account", PROBE_ACCOUNT)].into();
	let properties: HashMap<&str, Value<'_>> = [
		(
			"org.freedesktop.Secret.Item.Label",
			Value::from(PROBE_LABEL),
		),
		(
			"org.freedesktop.Secret.Item.Attributes",
			Value::from(Dict::from(attributes)),
		),
	]
	.into();
	let probe = (
		session.as_ref(),
		Vec::<u8>::new(),
		secret.clone(),
		PROBE_CONTENT_TYPE,
	);
	let (item, prompt): (OwnedObjectPath, OwnedObjectPath) =
		call(&collection, "CreateItem", &(properties, probe, true))
			.await
			.map_err(|error| match error {
				zbus::Error::MethodError(name, _, _) if name == IS_LOCKED => {
					RoundTripFailure::Locked
				}
				zbus::Error::MethodError(name, _, _)
					if points_nowhere(name.as_str()) =>
				{
					RoundTripFailure::Unavailable
				}
				_ => RoundTripFailure::Failed(CredentialProbeStep::Create),
			})?;
	// A store that wants a Prompt before it stores anything is asking for
	// the user, and the probe does not answer for them.
	if prompt.as_str() != NO_PROMPT {
		dismiss(&connection, prompt).await;
		return Err(RoundTripFailure::Locked);
	}

	let item = proxy(&connection, item.as_ref(), ITEM_INTERFACE)
		.await
		.map_err(|_| RoundTripFailure::Failed(CredentialProbeStep::Read))?;
	let read: zbus::Result<Secret> =
		call(&item, "GetSecret", &(session.as_ref(),)).await;
	// The item is deleted whatever the read said: a probe never outlives
	// its round trip.
	let deleted: zbus::Result<OwnedObjectPath> =
		call(&item, "Delete", &()).await;
	if let Ok(session) =
		proxy(&connection, session.as_ref(), SESSION_INTERFACE).await
	{
		let _closed: zbus::Result<()> = call(&session, "Close", &()).await;
	}

	if !matches!(&read, Ok((_, _, value, _)) if *value == secret) {
		return Err(RoundTripFailure::Failed(CredentialProbeStep::Read));
	}
	match deleted {
		Ok(prompt) if prompt.as_str() == NO_PROMPT => Ok(()),
		Ok(prompt) => {
			dismiss(&connection, prompt).await;
			Err(RoundTripFailure::Failed(CredentialProbeStep::Delete))
		}
		Err(_) => Err(RoundTripFailure::Failed(CredentialProbeStep::Delete)),
	}
}

/// Lets go of a Prompt the store offered, so it does not wait on a person
/// the probe never asks. Best effort: a Prompt that cannot be dismissed
/// changes nothing about the answer.
async fn dismiss(connection: &Connection, prompt: OwnedObjectPath) {
	if let Ok(prompt) =
		proxy(connection, prompt.as_ref(), PROMPT_INTERFACE).await
	{
		let _dismissed: zbus::Result<()> = call(&prompt, "Dismiss", &()).await;
	}
}

/// Whether a method error says the store, its default collection, or the
/// interface the probe speaks is not there.
fn points_nowhere(error: &str) -> bool {
	matches!(
		error,
		"org.freedesktop.DBus.Error.ServiceUnknown"
			| "org.freedesktop.DBus.Error.NameHasNoOwner"
			| "org.freedesktop.DBus.Error.UnknownObject"
			| "org.freedesktop.DBus.Error.UnknownInterface"
			| "org.freedesktop.DBus.Error.UnknownMethod"
	)
}

async fn connect(address: &str) -> zbus::Result<Connection> {
	zbus::connection::Builder::address(address)?.build().await
}

/// A proxy for one object of the store that caches nothing, so the only
/// messages on the bus are the calls the probe makes.
async fn proxy<'a, P>(
	connection: &Connection,
	path: P,
	interface: &'a str,
) -> zbus::Result<Proxy<'a>>
where
	P: TryInto<ObjectPath<'a>>,
	P::Error: Into<zbus::Error>,
{
	zbus::proxy::Builder::new(connection)
		.destination(SERVICE_NAME)?
		.path(path)?
		.interface(interface)?
		.cache_properties(CacheProperties::No)
		.build()
		.await
}

/// One call that never starts the store it is addressed to.
async fn call<R>(
	proxy: &Proxy<'_>,
	method: &str,
	body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> zbus::Result<R>
where
	R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
	proxy
		.call_with_flags(method, MethodFlags::NoAutoStart.into(), body)
		.await?
		.ok_or(zbus::Error::Unsupported)
}

#[cfg(test)]
mod tests {
	//! A Secret Service of the test's own, served over a private socket,
	//! so nothing here depends on the developer's keyring. The client
	//! side is exactly the production probe: it connects the way it
	//! connects to a session bus and speaks to the same names and paths.

	use super::{
		DEFAULT_COLLECTION, PROBE_CONTENT_TYPE, SERVICE_PATH, Secret,
		observe_at, verify_at,
	};
	use crate::capability::{
		CredentialProbeStep, CredentialStoreKind, CredentialStoreStatus,
		CredentialStoreVerification,
	};
	use pretty_assertions::assert_eq;
	use std::{
		collections::HashMap,
		path::Path,
		sync::{Arc, Mutex},
	};
	use zbus::{
		DBusError, fdo, interface,
		zvariant::{OwnedObjectPath, OwnedValue, Value},
	};

	const SESSION: &str = "/org/freedesktop/secrets/session/s1";
	const ITEM: &str = "/org/freedesktop/secrets/collection/login/1";
	const PROMPT: &str = "/org/freedesktop/secrets/prompt/p1";
	const KIND: CredentialStoreKind = CredentialStoreKind::SecretService;

	/// How the served store behaves.
	#[derive(Clone, Copy)]
	struct Store {
		/// The default collection is locked.
		locked: bool,
		/// Reading an item back gives the value that was stored.
		faithful: bool,
		/// Deleting an item completes without a Prompt.
		deletes: bool,
	}

	const OPEN: Store = Store {
		locked: false,
		faithful: true,
		deletes: true,
	};

	/// What the store holds: at most the one probe item.
	type Held = Arc<Mutex<Option<Vec<u8>>>>;

	fn subdir(dir: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
		let path = dir.path().join(name);
		std::fs::create_dir(&path).unwrap();
		path
	}

	fn path(path: &str) -> OwnedObjectPath {
		OwnedObjectPath::try_from(path).unwrap()
	}

	/// The message bus itself, enough of it for a client to say hello.
	struct Bus;

	#[interface(name = "org.freedesktop.DBus")]
	impl Bus {
		fn hello(&self) -> String {
			":1.7".into()
		}
	}

	struct Service;

	#[interface(name = "org.freedesktop.Secret.Service")]
	impl Service {
		fn open_session(
			&self,
			_algorithm: &str,
			_input: Value<'_>,
		) -> (OwnedValue, OwnedObjectPath) {
			(Value::from("").try_into().unwrap(), path(SESSION))
		}
	}

	struct Session;

	#[interface(name = "org.freedesktop.Secret.Session")]
	impl Session {
		fn close(&self) {}
	}

	#[derive(DBusError, Debug)]
	#[zbus(prefix = "org.freedesktop.Secret.Error")]
	enum SecretError {
		#[zbus(error)]
		ZBus(zbus::Error),
		IsLocked,
	}

	struct Collection {
		store: Store,
		held: Held,
	}

	#[interface(name = "org.freedesktop.Secret.Collection")]
	impl Collection {
		#[zbus(property)]
		fn locked(&self) -> bool {
			self.store.locked
		}

		fn create_item(
			&self,
			_properties: HashMap<String, OwnedValue>,
			secret: Secret,
			_replace: bool,
		) -> Result<(OwnedObjectPath, OwnedObjectPath), SecretError> {
			if self.store.locked {
				return Err(SecretError::IsLocked);
			}
			*self.held.lock().unwrap() = Some(secret.2);
			Ok((path(ITEM), path("/")))
		}
	}

	struct Item {
		store: Store,
		held: Held,
	}

	#[interface(name = "org.freedesktop.Secret.Item")]
	impl Item {
		fn get_secret(&self, session: OwnedObjectPath) -> fdo::Result<Secret> {
			let held = self.held.lock().unwrap().clone().ok_or_else(|| {
				fdo::Error::UnknownObject("nothing is held".into())
			})?;
			let value = if self.store.faithful {
				held
			} else {
				b"something else entirely".to_vec()
			};
			Ok((session, Vec::new(), value, PROBE_CONTENT_TYPE.into()))
		}

		fn delete(&self) -> OwnedObjectPath {
			if self.store.deletes {
				*self.held.lock().unwrap() = None;
				path("/")
			} else {
				path(PROMPT)
			}
		}
	}

	/// Serves `store` on a socket under `dir` for as long as the test's
	/// runtime lives, and returns the bus address a client connects to.
	async fn serve(dir: &Path, store: Store) -> (String, Held) {
		let socket = dir.join("bus");
		let listener = tokio::net::UnixListener::bind(&socket).unwrap();
		let held: Held = Arc::default();
		let served = Arc::clone(&held);
		tokio::spawn(async move {
			let mut connections = Vec::new();
			while let Ok((stream, _)) = listener.accept().await {
				let connection = zbus::connection::Builder::unix_stream(stream)
					.server(zbus::Guid::generate())
					.unwrap()
					.p2p()
					.serve_at("/org/freedesktop/DBus", Bus)
					.unwrap()
					.serve_at(SERVICE_PATH, Service)
					.unwrap()
					.serve_at(SESSION, Session)
					.unwrap()
					.serve_at(
						DEFAULT_COLLECTION,
						Collection {
							store,
							held: Arc::clone(&served),
						},
					)
					.unwrap()
					.serve_at(
						ITEM,
						Item {
							store,
							held: Arc::clone(&served),
						},
					)
					.unwrap()
					.build()
					.await
					.unwrap();
				connections.push(connection);
			}
		});
		(format!("unix:path={}", socket.display()), held)
	}

	/// The `Locked` property is the observation: an open collection is an
	/// available store and a locked one is a locked store, and neither
	/// observation writes anything.
	#[tokio::test]
	async fn the_default_collections_lock_state_is_the_observation() {
		let dir = tempfile::tempdir().unwrap();
		let (open, open_held) = serve(&subdir(&dir, "open"), OPEN).await;
		let (locked, locked_held) = serve(
			&subdir(&dir, "locked"),
			Store {
				locked: true,
				..OPEN
			},
		)
		.await;

		let seen_open = observe_at(&open).await;
		let seen_locked = observe_at(&locked).await;

		assert_eq!(
			(
				seen_open,
				seen_locked,
				open_held.lock().unwrap().clone(),
				locked_held.lock().unwrap().clone()
			),
			(
				CredentialStoreStatus::Available { kind: KIND },
				CredentialStoreStatus::Locked { kind: KIND },
				None,
				None
			)
		);
	}

	/// A bus nobody answers on is a Plane without a credential store, and
	/// the probe says so instead of waiting for one.
	#[tokio::test]
	async fn a_bus_nobody_answers_on_is_an_unavailable_store() {
		let dir = tempfile::tempdir().unwrap();
		let nobody = format!("unix:path={}", dir.path().join("bus").display());

		let observed = observe_at(&nobody).await;
		let verified = verify_at(&nobody).await;

		assert_eq!(
			(observed, verified),
			(
				CredentialStoreStatus::Unavailable { kind: KIND },
				CredentialStoreVerification::Unavailable { kind: KIND }
			)
		);
	}

	/// The round trip creates, reads back, and deletes, and leaves the
	/// store as it found it (ADR-0076).
	#[tokio::test]
	async fn an_open_store_round_trips_the_probe_and_keeps_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let (address, held) = serve(dir.path(), OPEN).await;

		let verified = verify_at(&address).await;

		assert_eq!(
			(verified, held.lock().unwrap().clone()),
			(CredentialStoreVerification::Verified { kind: KIND }, None)
		);
	}

	/// A locked collection refuses the write, and the probe reports the
	/// lock rather than answering the Prompt that would unlock it.
	#[tokio::test]
	async fn a_locked_store_is_reported_rather_than_unlocked() {
		let dir = tempfile::tempdir().unwrap();
		let (address, held) = serve(
			dir.path(),
			Store {
				locked: true,
				..OPEN
			},
		)
		.await;

		let verified = verify_at(&address).await;

		assert_eq!(
			(verified, held.lock().unwrap().clone()),
			(CredentialStoreVerification::Locked { kind: KIND }, None)
		);
	}

	/// A store that hands back something other than what it was given has
	/// not held the Credential. The probe item is still deleted.
	#[tokio::test]
	async fn a_store_that_returns_a_different_value_fails_the_read() {
		let dir = tempfile::tempdir().unwrap();
		let (address, held) = serve(
			dir.path(),
			Store {
				faithful: false,
				..OPEN
			},
		)
		.await;

		let verified = verify_at(&address).await;

		assert_eq!(
			(verified, held.lock().unwrap().clone()),
			(
				CredentialStoreVerification::Failed {
					kind: KIND,
					step: CredentialProbeStep::Read
				},
				None
			)
		);
	}

	/// A store that wants a Prompt before it deletes has not completed the
	/// round trip, and the probe does not answer the Prompt for the user.
	#[tokio::test]
	async fn a_delete_that_needs_a_prompt_fails_the_delete() {
		let dir = tempfile::tempdir().unwrap();
		let (address, _held) = serve(
			dir.path(),
			Store {
				deletes: false,
				..OPEN
			},
		)
		.await;

		let verified = verify_at(&address).await;

		assert_eq!(
			verified,
			CredentialStoreVerification::Failed {
				kind: KIND,
				step: CredentialProbeStep::Delete
			}
		);
	}
}
