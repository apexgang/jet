//! Paired-client connection authentication, separate from SSH endpoint trust.

use crate::{
	Actor, ClientId, ClientPublicKey, Core, CoreError, ErrorCategory,
	PairedClientAccess, PairingSignature,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::watch;

// Fair admission: reads take one permit; Commands take all permits so a
// committed authority change publishes before any new request is admitted.
pub(crate) const AUTHORITY_READERS: u32 = 128;

pub(crate) fn invalidated_client(
	outcome: &crate::CommandOutcome,
) -> Option<ClientId> {
	use crate::CommandOutcome;
	match outcome {
		CommandOutcome::PairedClientRevoked { client_id } => Some(*client_id),
		CommandOutcome::PairingCompleted { client } => Some(client.client_id),
		CommandOutcome::PairedClientAccessSet { client } => match client.access
		{
			PairedClientAccess::Disabled => Some(client.client_id),
			PairedClientAccess::Enabled => None,
		},
		CommandOutcome::ConversationCreated(_)
		| CommandOutcome::RunCreated(_)
		| CommandOutcome::RunTransitioned(_)
		| CommandOutcome::SettingSet { .. }
		| CommandOutcome::SettingCleared { .. }
		| CommandOutcome::AccountBound(_)
		| CommandOutcome::AccountUnbound { .. }
		| CommandOutcome::PairingGateSet { .. }
		| CommandOutcome::PairingOpened { .. }
		| CommandOutcome::PairingClaimed { .. }
		| CommandOutcome::PairingConfirmed { .. }
		| CommandOutcome::AuditEpochBegun { .. }
		| CommandOutcome::ProjectRegistered(_)
		| CommandOutcome::WorkspacePromotionRecorded(_)
		| CommandOutcome::ConversationImported(_) => None,
	}
}

/// Revocable authority for one authenticated remote connection. It cannot
/// be constructed from a Client identity or restored from persisted Events.
#[derive(Debug, Clone)]
pub struct RemoteSession {
	client_id: ClientId,
	revoked: Arc<watch::Sender<bool>>,
}

impl PartialEq for RemoteSession {
	fn eq(&self, other: &Self) -> bool {
		Arc::ptr_eq(&self.revoked, &other.revoked)
	}
}
impl Eq for RemoteSession {}

impl RemoteSession {
	/// Identity whose signature established this connection.
	pub fn client_id(&self) -> ClientId {
		self.client_id
	}

	/// Resolves when this connection must close. Revocation is permanent for
	/// this connection even when the owner immediately enables the key again.
	pub async fn revoked(&self) {
		let _ = self.revoked.subscribe().wait_for(|revoked| *revoked).await;
	}

	pub(crate) fn authorize(&self) -> Result<(), CoreError> {
		if *self.revoked.borrow() {
			Err(unauthorized())
		} else {
			Ok(())
		}
	}
}

#[derive(Debug, Default)]
pub(crate) struct RemoteSessions(
	Mutex<HashMap<ClientId, Vec<Weak<watch::Sender<bool>>>>>,
);

impl RemoteSessions {
	pub(crate) fn authorize(
		&self,
		session: &RemoteSession,
	) -> Result<(), CoreError> {
		session.authorize()?;
		let sessions =
			self.0.lock().expect("remote authority registry poisoned");
		if sessions.get(&session.client_id).is_some_and(|connections| {
			connections.iter().any(|connection| {
				connection.ptr_eq(&Arc::downgrade(&session.revoked))
			})
		}) {
			Ok(())
		} else {
			Err(unauthorized())
		}
	}
	fn register(
		&self,
		client_id: ClientId,
	) -> Result<RemoteSession, CoreError> {
		let mut sessions =
			self.0.lock().expect("remote authority registry poisoned");
		sessions.retain(|_, connections| {
			connections.retain(|connection| connection.strong_count() > 0);
			!connections.is_empty()
		});
		if sessions.values().map(Vec::len).sum::<usize>() >= 128 {
			return Err(CoreError::unavailable(
				"connection.limit",
				"too many remote connections",
				"remote connection limit reached",
			));
		}
		let (revoked, _) = watch::channel(false);
		let revoked = Arc::new(revoked);
		sessions
			.entry(client_id)
			.or_default()
			.push(Arc::downgrade(&revoked));
		Ok(RemoteSession { client_id, revoked })
	}

	pub(crate) fn invalidate(&self, client_id: ClientId) {
		if let Some(connections) = self
			.0
			.lock()
			.expect("remote authority registry poisoned")
			.remove(&client_id)
		{
			for connection in connections
				.into_iter()
				.filter_map(|connection| connection.upgrade())
			{
				connection.send_replace(true);
			}
		}
	}
}

impl Drop for RemoteSessions {
	fn drop(&mut self) {
		let sessions = self
			.0
			.get_mut()
			.expect("remote authority registry poisoned");
		for connection in sessions.values().flatten().filter_map(Weak::upgrade)
		{
			connection.send_replace(true);
		}
	}
}

impl Core {
	/// Admits a No-Visa process under a live connection's authority. Remote
	/// tool adapters must first validate registered roots and tool permissions
	/// (issue #32); this owns the revocation race and process lifetime only.
	///
	/// # Errors
	/// Refuses revoked authority or an OS spawn failure. Visa Runs never use
	/// this connection-scoped operation seam.
	pub async fn spawn_no_visa(
		&self,
		session: &RemoteSession,
		command: &mut tokio::process::Command,
	) -> Result<jet_runtime::NoVisaOperation, CoreError> {
		let _access = self
			.remote_access
			.acquire()
			.await
			.expect("authority gate never closes");
		self.remote_sessions.authorize(session)?;
		let session = session.clone();
		jet_runtime::NoVisaOperation::spawn(command, async move {
			session.revoked().await
		})
		.map_err(|error| {
			CoreError::unavailable(
				"operation.spawn_failed",
				"the No-Visa operation could not start",
				error.to_string(),
			)
		})
	}

	/// Verifies a signature against this Plane's enabled Paired-client key.
	/// The trusted transport supplies its fresh, connection-bound transcript;
	/// it must accept exactly one proof before its handshake deadline.
	///
	/// # Errors
	/// Returns the same authorization refusal for unknown, disabled, and
	/// incorrect keys, or a store error if authority cannot be established.
	pub async fn authenticate_remote(
		&self,
		client_id: ClientId,
		transcript: &[u8],
		signature: PairingSignature,
	) -> Result<Actor, CoreError> {
		let _access = self
			.remote_access
			.acquire()
			.await
			.expect("authority gate never closes");
		let record = self
			.store
			.read(async |tx| tx.paired_client(client_id.0).await)
			.await?;
		let verified = record.is_some_and(|record| {
			record.access == PairedClientAccess::Enabled
				&& crate::pairing_identity::verifies(
					&ClientPublicKey {
						algorithm: record.key_algorithm,
						key: record.public_key,
					},
					transcript,
					&signature,
				)
		});
		// ASVS 16.2.1, 16.2.5: only attribution and outcome reach the audit;
		// neither challenge, signature, nor key material is recorded.
		let actor = Actor::InteractiveClient { client_id };
		self.store
			.write(async |tx| {
				crate::audit::record(
					tx,
					&actor,
					crate::audit::Decision {
						decision: crate::AuditDecision::ConnectionAuthenticated,
						subject: crate::audit::AuditSubject::PairedClient(
							client_id,
						),
						outcome: if verified {
							crate::AuditOutcome::Succeeded
						} else {
							crate::AuditOutcome::Denied
						},
					},
					self.now_unix_ms(),
				)
				.await
			})
			.await?;
		if !verified {
			return Err(unauthorized());
		}
		Ok(Actor::RemoteClient {
			session: self.remote_sessions.register(client_id)?,
		})
	}
}

pub(crate) fn unauthorized() -> CoreError {
	CoreError {
		category: ErrorCategory::Unauthorized,
		code: "connection.unauthorized".into(),
		message:
			"an enabled Paired client must prove this connection's challenge"
				.into(),
		retryable: false,
		detail: None,
		revision_conflict: None,
		recovery_actions: vec![],
	}
}
