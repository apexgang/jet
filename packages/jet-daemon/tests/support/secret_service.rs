//! An open Secret Service served over a private socket, so a Plane under
//! test has a credential store without touching the developer's keyring.
//! It speaks enough of the session bus for `jetd` to say hello, reports
//! its default collection unlocked, and round-trips one item.

use std::{
	collections::HashMap,
	path::Path,
	sync::{Arc, Mutex},
};
use zbus::{
	fdo, interface,
	zvariant::{OwnedObjectPath, OwnedValue, Value},
};

const SESSION: &str = "/org/freedesktop/secrets/session/s1";
const ITEM: &str = "/org/freedesktop/secrets/collection/login/1";

/// A Secret as the wire carries it: session, parameters, value, content
/// type.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);
type Held = Arc<Mutex<Option<Vec<u8>>>>;

fn path(path: &str) -> OwnedObjectPath {
	OwnedObjectPath::try_from(path).unwrap()
}

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

struct Collection(Held);

#[interface(name = "org.freedesktop.Secret.Collection")]
impl Collection {
	#[zbus(property)]
	fn locked(&self) -> bool {
		false
	}

	fn create_item(
		&self,
		_properties: HashMap<String, OwnedValue>,
		secret: Secret,
		_replace: bool,
	) -> (OwnedObjectPath, OwnedObjectPath) {
		*self.0.lock().unwrap() = Some(secret.2);
		(path(ITEM), path("/"))
	}
}

struct Item(Held);

#[interface(name = "org.freedesktop.Secret.Item")]
impl Item {
	fn get_secret(&self, session: OwnedObjectPath) -> fdo::Result<Secret> {
		let value = self.0.lock().unwrap().clone().ok_or_else(|| {
			fdo::Error::UnknownObject("nothing is held".into())
		})?;
		Ok((
			session,
			Vec::new(),
			value,
			"application/octet-stream".into(),
		))
	}

	fn delete(&self) -> OwnedObjectPath {
		*self.0.lock().unwrap() = None;
		path("/")
	}
}

/// Serves the store at `socket` for as long as the test's runtime lives,
/// and returns the bus address to hand `jetd`. A `jetd` restarted over the
/// same home gets a fresh store at the same address.
pub async fn serve(socket: &Path) -> String {
	let _ = std::fs::remove_file(socket);
	let listener = tokio::net::UnixListener::bind(socket).unwrap();
	let held: Held = Arc::default();
	tokio::spawn(async move {
		let mut connections = Vec::new();
		while let Ok((stream, _)) = listener.accept().await {
			let connection = zbus::connection::Builder::unix_stream(stream)
				.server(zbus::Guid::generate())
				.unwrap()
				.p2p()
				.serve_at("/org/freedesktop/DBus", Bus)
				.unwrap()
				.serve_at("/org/freedesktop/secrets", Service)
				.unwrap()
				.serve_at(SESSION, Session)
				.unwrap()
				.serve_at(
					"/org/freedesktop/secrets/aliases/default",
					Collection(Arc::clone(&held)),
				)
				.unwrap()
				.serve_at(ITEM, Item(Arc::clone(&held)))
				.unwrap()
				.build()
				.await
				.unwrap();
			connections.push(connection);
		}
	});
	format!("unix:path={}", socket.display())
}
