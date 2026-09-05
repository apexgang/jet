//! Queries: read-only snapshots. Each snapshot and its journal cursor are
//! read in one consistent transaction (ADR-0092).

use jet_store::{ConversationPageStart, ReadTransaction};

use crate::account::{
	AccountBindingList, AccountBindingStatus, CredentialState,
};
use crate::audit::{AUDIT_PAGE_LIMIT, AuditEntry, AuditPage, AuditSequence};
use crate::capability::{CapabilityObservation, CapabilitySnapshot};
use crate::conversation::{
	ConversationId, ConversationList, ConversationSnapshot, PageCursor,
};
use crate::error::CoreError;
use crate::event::{EVENT_PAGE_LIMIT, Event, EventPage, EventSequence};
use crate::pairing::{self, PairingSnapshot};
use crate::setting::{self, SettingScope, SettingSelection, SettingSnapshot};
use crate::status::PlaneStatus;
use crate::{Actor, CORE_VERSION, Core, PlaneId};

/// Read-only requests answered with a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
	/// Snapshot of the daemon's Plane status.
	Status,
	/// First bounded page of Conversations on the Plane.
	Conversations,
	/// Legacy minor-0 snapshot containing every Conversation in one result.
	LegacyConversations,
	/// Continue a fenced Conversation keyset snapshot.
	NextConversations {
		/// Opaque token returned by the previous page.
		cursor: PageCursor,
	},
	/// One Conversation with all of its Runs.
	Conversation {
		/// The Conversation to read.
		conversation_id: ConversationId,
	},
	/// What the Plane can do (ADR-0086).
	Capabilities {
		/// Whether to report the last observation or take a new one.
		observation: CapabilityObservation,
	},
	/// Every Account binding on the Plane, with the state of the Credential
	/// each one resolves (ADR-0016, ADR-0076).
	AccountBindings {
		/// Whether the Credential states follow the last observation of the
		/// Plane or a new one, taken now. A GUI that has just unlocked the
		/// credential store asks for a new one (ADR-0086).
		observation: CapabilityObservation,
	},
	/// Settings resolved for one scope (ADR-0085).
	Settings {
		/// The scope to resolve for; its own values win over the Plane's.
		scope: SettingScope,
		/// Which Settings to resolve.
		selection: SettingSelection,
	},
	/// One page of journal Events strictly after a position, with the
	/// journal cursor the page was read at.
	Events {
		/// The position to resume after; zero for the whole journal.
		after: EventSequence,
	},
	/// The Plane's Pairing: whether it accepts new GUI clients (ADR-0017).
	Pairing,
	/// One page of the owner-only Security audit strictly after a position
	/// (ADR-0105).
	SecurityAudit {
		/// The position to resume after; zero for the whole audit.
		after: AuditSequence,
	},
}

/// Snapshots returned by [`Core::query`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
	/// Snapshot of the daemon's Plane status.
	Status(PlaneStatus),
	/// One page of Conversations on the Plane.
	Conversations(ConversationList),
	/// One Conversation with all of its Runs.
	Conversation(ConversationSnapshot),
	/// What the Plane can do.
	Capabilities(CapabilitySnapshot),
	/// Every Account binding on the Plane.
	AccountBindings(AccountBindingList),
	/// Settings resolved for one scope.
	Settings(SettingSnapshot),
	/// One page of journal Events in sequence order.
	Events(EventPage),
	/// The Plane's Pairing as it stands.
	Pairing(PairingSnapshot),
	/// One page of the Security audit, oldest first.
	SecurityAudit(AuditPage),
}

impl Core {
	/// Runs `query` on behalf of `actor` and returns its snapshot.
	///
	/// # Errors
	///
	/// Returns [`CoreError`] when the Actor is not authorized, the
	/// addressed entity does not exist, or the store cannot answer.
	pub async fn query(
		&self,
		actor: &Actor,
		query: Query,
	) -> Result<QueryResult, CoreError> {
		let _access = self
			.remote_access
			.acquire()
			.await
			.expect("authority gate never closes");
		actor.authorize(&self.remote_sessions)?;
		match query {
			Query::Status => {
				let security = *self.security.read().await;
				self.store
					.read(async |tx| {
						let plane = tx.plane().await?;
						let cursor = EventSequence(tx.event_cursor().await?);
						Ok(QueryResult::Status(PlaneStatus {
							cursor,
							plane_id: PlaneId(plane.plane_id),
							daemon_starts: plane.daemon_starts,
							started_at: self.started_at,
							core_version: CORE_VERSION,
							security,
						}))
					})
					.await
			}
			Query::Conversations => first_conversations(self).await,
			Query::LegacyConversations => {
				self.store
					.read(async |tx| {
						Ok(QueryResult::Conversations(ConversationList {
							cursor: EventSequence(tx.event_cursor().await?),
							conversations: tx
								.conversations()
								.await?
								.into_iter()
								.map(Into::into)
								.collect(),
							next_page: None,
						}))
					})
					.await
			}
			Query::NextConversations { cursor } => {
				next_conversations(self, &cursor).await
			}
			Query::Conversation { conversation_id } => {
				self.store
					.read(async |tx| conversation(tx, conversation_id).await)
					.await
			}
			Query::Capabilities { observation } => {
				Ok(QueryResult::Capabilities(match observation {
					CapabilityObservation::LastObserved => {
						self.capabilities.read().await.clone()
					}
					CapabilityObservation::Fresh => {
						self.observe_capabilities().await
					}
				}))
			}
			Query::AccountBindings { observation } => {
				account_bindings(self, observation).await
			}
			Query::Settings { scope, selection } => {
				settings(self, scope, selection).await
			}
			Query::Events { after } => {
				self.store
					.read(async |tx| {
						let (cursor, events) =
							tx.events_after(after.0, EVENT_PAGE_LIMIT).await?;
						Ok(QueryResult::Events(EventPage {
							cursor: EventSequence(cursor),
							events: events
								.into_iter()
								.map(Event::try_from)
								.collect::<Result<_, _>>()?,
						}))
					})
					.await
			}
			Query::Pairing => {
				let now_unix_ms = self.now_unix_ms();
				// ASVS 2.3.3: the gate, the offer, and the position that
				// fences them come from one SQLite snapshot.
				self.store
					.read(async |tx| {
						let cursor = EventSequence(tx.event_cursor().await?);
						let gate = tx.pairing_gate().await?;
						let offer = tx.pairing_offer().await?;
						let clients = tx.paired_clients().await?;
						Ok(QueryResult::Pairing(PairingSnapshot {
							cursor,
							gate,
							pending: offer.as_ref().map(|record| {
								pairing::pending(record, now_unix_ms)
							}),
							clients: clients
								.into_iter()
								.map(pairing::paired_client)
								.collect(),
						}))
					})
					.await
			}
			Query::SecurityAudit { after } => {
				// ASVS 2.3.3: the page and the position that fences it come
				// from one SQLite snapshot.
				self.store
					.read(async |tx| {
						let (cursor, records) =
							tx.audit_page(after.0, AUDIT_PAGE_LIMIT).await?;
						Ok(QueryResult::SecurityAudit(AuditPage {
							cursor: AuditSequence(cursor),
							entries: records
								.into_iter()
								.map(AuditEntry::from)
								.collect(),
						}))
					})
					.await
			}
		}
	}
}

/// Reads the bindings and pairs each with the state of its Credential.
///
/// The bindings, the daemon start that decides whether a session-only
/// Credential is still held, and the Event fence all come from one SQLite
/// snapshot (ASVS 2.3.3); the credential store's own state comes from the
/// observation `observation` selects.
async fn account_bindings(
	core: &Core,
	observation: CapabilityObservation,
) -> Result<QueryResult, CoreError> {
	let store = match observation {
		CapabilityObservation::LastObserved => {
			core.capabilities.read().await.credential_store
		}
		CapabilityObservation::Fresh => {
			core.observe_capabilities().await.credential_store
		}
	};
	core.store
		.read(async |tx| {
			let cursor = EventSequence(tx.event_cursor().await?);
			let daemon_start = tx.plane().await?.daemon_starts;
			let bindings = tx.account_bindings().await?;
			Ok(QueryResult::AccountBindings(AccountBindingList {
				cursor,
				bindings: bindings
					.into_iter()
					.map(|record| {
						let binding: crate::AccountBinding = record.into();
						AccountBindingStatus {
							credential_state: CredentialState::of(
								&binding.credential_reference,
								store,
								daemon_start,
							),
							binding,
						}
					})
					.collect(),
			}))
		})
		.await
}

async fn settings(
	core: &Core,
	scope: SettingScope,
	selection: SettingSelection,
) -> Result<QueryResult, CoreError> {
	let keys = selection.keys();
	// ASVS 2.3.3: the resolved values and their Event fence come from one
	// SQLite snapshot, so a Command committed between them cannot show up
	// in one and not the other.
	core.store
		.read(async |tx| {
			setting::require_subject(tx, scope).await?;
			let cursor = EventSequence(tx.event_cursor().await?);
			let stored = tx.settings_for_scope(scope.record()).await?;
			Ok(QueryResult::Settings(SettingSnapshot {
				cursor,
				scope,
				settings: setting::resolve(&keys, &stored),
			}))
		})
		.await
}

async fn first_conversations(core: &Core) -> Result<QueryResult, CoreError> {
	let now = core.now_unix_ms();
	// ASVS 2.3.3 and 15.4.2: the projection page and its Event fence are
	// read atomically from one SQLite snapshot.
	let (cursor, (conversations, next)) = core
		.store
		.read(async |tx| {
			let cursor = EventSequence(tx.event_cursor().await?);
			let page =
				tx.conversation_page(ConversationPageStart::First).await?;
			Ok::<_, CoreError>((cursor, page))
		})
		.await?;
	let deadline = core.conversation_pages.first_deadline(now);
	let next_page = core.conversation_pages.issue(next, cursor, deadline, now);
	Ok(QueryResult::Conversations(ConversationList {
		cursor,
		conversations: conversations.into_iter().map(Into::into).collect(),
		next_page,
	}))
}

async fn next_conversations(
	core: &Core,
	cursor: &PageCursor,
) -> Result<QueryResult, CoreError> {
	let now = core.now_unix_ms();
	let Some(state) = core.conversation_pages.resume(cursor, now) else {
		let current = core
			.store
			.read(async |tx| {
				Ok::<_, CoreError>(EventSequence(tx.event_cursor().await?))
			})
			.await?;
		return Err(CoreError::pagination_stale(current));
	};
	let (current, (conversations, next)) = core
		.store
		.read(async |tx| {
			let current = EventSequence(tx.event_cursor().await?);
			let page = tx
				.conversation_page(ConversationPageStart::After(state.after))
				.await?;
			Ok::<_, CoreError>((current, page))
		})
		.await?;
	if current != state.snapshot_revision {
		return Err(CoreError::pagination_stale(current));
	}
	let next_page = core.conversation_pages.issue(
		next,
		state.snapshot_revision,
		state.expires_at_unix_ms,
		now,
	);
	Ok(QueryResult::Conversations(ConversationList {
		cursor: state.snapshot_revision,
		conversations: conversations.into_iter().map(Into::into).collect(),
		next_page,
	}))
}

async fn conversation(
	tx: &mut ReadTransaction,
	conversation_id: ConversationId,
) -> Result<QueryResult, CoreError> {
	let Some(record) = tx.conversation(conversation_id.0).await? else {
		return Err(CoreError::not_found(
			"conversation.not_found",
			"the Conversation does not exist",
		));
	};
	let cursor = EventSequence(tx.event_cursor().await?);
	let runs = tx.runs(conversation_id.0).await?;
	Ok(QueryResult::Conversation(ConversationSnapshot {
		cursor,
		conversation: record.into(),
		runs: runs.into_iter().map(Into::into).collect(),
	}))
}
