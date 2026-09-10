//! Authenticated, fenced Query execution over the Plane store.

use super::{Query, QueryResult};
use crate::{
	Actor, CORE_VERSION, Core, PlaneId,
	account::{AccountBindingList, AccountBindingStatus, CredentialState},
	audit::{AUDIT_PAGE_LIMIT, AuditEntry, AuditPage, AuditSequence},
	capability::CapabilityObservation,
	conversation::{
		ConversationId, ConversationList, ConversationSnapshot, PageCursor,
		import,
	},
	error::CoreError,
	event::EventSequence,
	pairing::{self, PairingSnapshot},
	project::{self, ProjectList, entry as project_entry},
	promotion, search,
	setting::{self, SettingScope, SettingSelection, SettingSnapshot},
	status::PlaneStatus,
	workspace::Workspace,
};
use jet_store::{ConversationPageStart, ReadTransaction};

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
			Query::RemoteToolReview {
				client_id,
				operation_id,
			} => self
				.store
				.read(async |tx| {
					crate::remote::review::pending(
						tx,
						client_id,
						operation_id,
						self.now_unix_ms(),
					)
					.await
				})
				.await
				.map(QueryResult::RemoteToolReview),
			Query::InspectExtension {
				craft_id,
				extension_id,
			} => crate::extension::work::inspect(self, &craft_id, &extension_id)
				.await
				.map(QueryResult::ExtensionCatalog),
			Query::ExtensionCatalog { craft_id } => {
				crate::extension::work::catalog(self, &craft_id)
					.await
					.map(QueryResult::ExtensionCatalog)
			}
			Query::ExtensionChange { change_id } => self
				.store
				.read(async |tx| {
					crate::extension::work::query(tx, change_id).await
				})
				.await
				.map(QueryResult::ExtensionChange),
			Query::DiscoverCraft { source } => {
				crate::craft::installation::discover(self, source)
					.await
					.map(Box::new)
					.map(QueryResult::CraftInstallationPreview)
			}
			Query::EditableFile { target, path } => {
				crate::user_input::read(self, target, path)
					.await
					.map(QueryResult::EditableFile)
			}
			Query::WorkspaceTerminals { workspace_id } => {
				self.store
					.read(async |tx| {
						if tx.workspace(workspace_id.0).await?.is_none() {
							return Err(crate::terminal::missing());
						}
						let terminals =
							tx.workspace_terminals(workspace_id.0).await?;
						Ok(QueryResult::WorkspaceTerminals {
							cursor: EventSequence(tx.event_cursor().await?),
							terminals: terminals
								.iter()
								.map(crate::terminal::snapshot)
								.collect::<Result<_, _>>()?,
						})
					})
					.await
			}
			Query::ChangeArtifact { sha256, offset } => {
				crate::checkpoint::change_artifact::read(
					self.run_home(),
					sha256,
					offset,
				)
				.await
				.map(QueryResult::ChangeArtifact)
			}
			Query::ChangeDiff { run_id, scope } => {
				crate::checkpoint::query::query(
					self,
					run_id,
					scope,
					crate::checkpoint::pages::Start::First,
				)
				.await
			}
			Query::NextChangeDiff { cursor } => {
				crate::checkpoint::query::next(self, cursor).await
			}
			Query::GitDeliveries { conversation_id } => self
				.store
				.read(async |tx| {
					crate::git_delivery::state::query(tx, conversation_id).await
				})
				.await
				.map(QueryResult::GitDeliveries),
			Query::Utility { job_id } => self
				.store
				.read(async |tx| crate::utility::work::query(tx, job_id).await)
				.await
				.map(QueryResult::Utility),
			Query::AutoContinue { target } => self
				.store
				.read(async |tx| {
					crate::auto_continue::snapshot(tx, target).await
				})
				.await
				.map(|s| QueryResult::AutoContinue(Box::new(s))),
			Query::ScheduledTasks { conversation_id } => self
				.store
				.read(async |tx| {
					crate::schedule::snapshot(tx, conversation_id).await
				})
				.await
				.map(QueryResult::ScheduledTasks),
			Query::TurnQueue { conversation_id } => self
				.store
				.read(async |tx| {
					crate::turn::queue::snapshot(tx, conversation_id).await
				})
				.await
				.map(QueryResult::TurnQueue),
			Query::OrphanedExecutions { after } => self
				.orphaned_executions(after)
				.await
				.map(QueryResult::OrphanedExecutions),
			Query::RunExecution { run_id } => self
				.store
				.read(async |tx| crate::run::state::snapshot(tx, run_id).await)
				.await
				.map(QueryResult::RunExecution),
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
			Query::Usage { selection } => {
				crate::usage::query::usage(self, selection).await
			}
			Query::Settings { scope, selection } => {
				settings(self, scope, selection).await
			}
			Query::Events { after } => {
				crate::event::query::events(self, after).await
			}
			Query::LegacyEvents { after } => {
				crate::event::query::legacy_events(self, after).await
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
			Query::PreviewProject { grant, observation } => {
				project::preview(self, actor, &grant, observation).await
			}
			Query::ProjectEntry { project_id, path } => {
				project_entry::entry(self, project_id, path).await
			}
			Query::PreviewPromotion {
				workspace_id,
				destination,
			} => {
				promotion::preview(self, actor, workspace_id, destination).await
			}
			Query::Search { terms } => search::query(self, &terms).await,
			Query::LegacySearch { terms } => {
				search::legacy_query(self, &terms).await
			}
			Query::ExternalConversations => {
				import::external_conversations(self).await
			}
			Query::Projects => {
				self.store
					.read(async |tx| {
						let cursor = EventSequence(tx.event_cursor().await?);
						let projects = tx.projects().await?;
						Ok(QueryResult::Projects(ProjectList {
							cursor,
							projects: projects
								.into_iter()
								.map(Into::into)
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
pub(super) async fn account_bindings(
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

pub(super) async fn settings(
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

pub(super) async fn first_conversations(
	core: &Core,
) -> Result<QueryResult, CoreError> {
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

pub(super) async fn next_conversations(
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

pub(super) async fn conversation(
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
	let workspace = match tx.workspace_of(conversation_id.0).await? {
		Some(record) => {
			let promotion = tx.latest_promotion(record.workspace_id).await?;
			let mut workspace = Workspace::from(record);
			workspace.promotion = promotion.map(Into::into);
			Some(workspace)
		}
		None => None,
	};
	let runs = tx.runs(conversation_id.0).await?;
	Ok(QueryResult::Conversation(Box::new(ConversationSnapshot {
		cursor,
		conversation: record.into(),
		workspace,
		runs: runs.into_iter().map(Into::into).collect(),
	})))
}
