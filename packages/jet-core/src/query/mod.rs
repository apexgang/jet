//! Queries: read-only snapshots. Each snapshot and its journal cursor are
//! read in one consistent transaction (ADR-0092).

mod execute;

use crate::{
	ProjectId,
	account::AccountBindingList,
	audit::{AuditPage, AuditSequence},
	capability::{CapabilityObservation, CapabilitySnapshot},
	conversation::{
		ConversationId, ConversationList, ConversationSnapshot, PageCursor,
		import::ExternalConversationList,
	},
	event::{EventPage, EventSequence},
	filesystem::relative_path::RelativePath,
	pairing::PairingSnapshot,
	project::{PathGrant, ProjectList, ProjectPreview, entry::ProjectEntry},
	promotion::{PromotionDestination, PromotionPreview},
	search::{SearchResult, SearchTerms},
	setting::{SettingScope, SettingSelection, SettingSnapshot},
	status::PlaneStatus,
	workspace::WorkspaceId,
};

/// Read-only requests answered with a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
	/// Latest 100 durable Git operations, newest first.
	GitDeliveries {
		/// Owning Conversation.
		conversation_id: ConversationId,
	},
	/// Read an Auto-continue policy and its latest durable decision.
	AutoContinue {
		/// Scope to inspect.
		target: crate::AutoContinueTarget,
	},
	/// Inspect native source, components and requested access before a mutation.
	InspectExtension {
		/// Accepted Craft identity.
		craft_id: String,
		/// Exact native target.
		extension_id: String,
	},
	/// Discover native extensions through an accepted Craft.
	ExtensionCatalog {
		/// Craft identity.
		craft_id: String,
	},
	/// Inspect a staged native lifecycle operation.
	ExtensionChange {
		/// Durable operation identity.
		change_id: uuid::Uuid,
	},
	/// Inspect the exact remote action awaiting a destination review.
	RemoteToolReview {
		/// Paired installation that submitted it.
		client_id: crate::ClientId,
		/// Remote operation identity.
		operation_id: uuid::Uuid,
	},
	/// Read the durable result of one Utility request.
	Utility {
		/// Plane-assigned identity.
		job_id: uuid::Uuid,
	},
	/// Verify a third-party Craft source and return its exact consent surface.
	DiscoverCraft {
		/// Repository release or explicit Developer Mode source.
		source: crate::CraftSource,
	},
	/// Enabled schedules in one Conversation.
	ScheduledTasks {
		/// Owning Conversation.
		conversation_id: crate::ConversationId,
	},
	/// Read bounded UTF-8 content through a registered root.
	EditableFile {
		/// Registered Project or Workspace root.
		target: crate::FileTarget,
		/// Validated relative path.
		path: RelativePath,
	},
	/// Terminals belonging to one Workspace.
	WorkspaceTerminals {
		/// Workspace identity.
		workspace_id: crate::WorkspaceId,
	},
	/// Read a bounded chunk of a checkpoint patch Artifact.
	ChangeArtifact {
		/// Canonical SHA-256 content address.
		sha256: String,
		/// Byte offset, from zero through the Artifact length.
		offset: u64,
	},
	/// Compare observed Change checkpoints of a managed Run.
	ChangeDiff {
		/// Owning Run.
		run_id: crate::RunId,
		/// Boundaries to compare.
		scope: crate::DiffScope,
	},
	/// Continue changed-file metadata at an opaque, expiring snapshot cursor.
	NextChangeDiff {
		/// Cursor supplied by the preceding diff page.
		cursor: crate::PageCursor,
	},
	/// Authoritative input order and current execution state.
	TurnQueue {
		/// Conversation whose queue is read.
		conversation_id: ConversationId,
	},
	/// Bounded read-only metadata for interactive recovery decisions.
	OrphanedExecutions {
		/// Continue after the previous page.
		after: Option<crate::RunId>,
	},
	/// Read the durable execution projection of a managed Run.
	RunExecution {
		/// The Run to inspect.
		run_id: crate::RunId,
	},
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
	/// The Usage records this Plane holds for one scope, with the
	/// freshness and estimation each of them carries (ADR-0023).
	Usage {
		/// What the answer covers.
		selection: crate::UsageSelection,
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
	/// Events visible to a peer predating independent entity names.
	LegacyEvents {
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
	/// Every registered Project on the Plane (ADR-0025).
	Projects,
	/// What a Path grant would register, before it is made (ADR-0101).
	PreviewProject {
		/// The absolute path the user is about to grant.
		grant: PathGrant,
		/// Whether Git LFS is reported from the last observation of the
		/// Plane or a new one, taken now (ADR-0086).
		observation: CapabilityObservation,
	},
	/// What one path inside a Project names: the shape every ordinary file
	/// operation takes, a Project and a validated relative path
	/// (ADR-0101). Workspaces address files the same way once they exist
	/// (ADR-0025).
	ProjectEntry {
		/// The Project to resolve the path in.
		project_id: ProjectId,
		/// The path, relative to the Project's root.
		path: RelativePath,
	},
	/// What promoting a Workspace to a permanent checkout or branch of its
	/// Project would do, before it is done (ADR-0025).
	PreviewPromotion {
		/// The Workspace to promote.
		workspace_id: WorkspaceId,
		/// Where its changes would go.
		destination: PromotionDestination,
	},
	/// Bounded ranked hits over this Plane's human-visible Conversation
	/// content (ADR-0036). A GUI merges the answers of every Plane it is
	/// connected to.
	Search {
		/// The terms every hit must contain.
		terms: SearchTerms,
	},
	/// Search visible to a peer predating independent entity names.
	LegacySearch {
		/// The terms every hit must contain.
		terms: SearchTerms,
	},
	/// The Harness-native Conversations the Plane can see outside its
	/// management, and the imports it holds (ADR-0010).
	ExternalConversations,
}

/// Snapshots returned by [`Core::query`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
	/// Individually attributed Git outcomes.
	GitDeliveries(Vec<crate::GitDelivery>),
	/// Fenced Auto-continue policy and decision.
	AutoContinue(Box<crate::AutoContinueSnapshot>),
	/// Unconverted native inventory.
	ExtensionCatalog(crate::ExtensionCatalog),
	/// Durable lifecycle outcome.
	ExtensionChange(crate::ExtensionChange),
	/// Exact remote action awaiting a destination review.
	RemoteToolReview(crate::RemoteToolRequest),
	/// Durable Utility result.
	Utility(crate::UtilityJob),
	/// Verified proposal that performs no installation by itself.
	CraftInstallationPreview(Box<crate::CraftInstallationPreview>),
	/// Fenced schedule snapshot.
	ScheduledTasks(crate::ScheduledTasks),
	/// Bounded editable content and its exact file Revision.
	EditableFile(crate::EditableFile),
	/// Workspace terminal lifecycle snapshots.
	WorkspaceTerminals {
		/// Snapshot Event cursor.
		cursor: EventSequence,
		/// Retained terminal states.
		terminals: Vec<crate::WorkspaceTerminal>,
	},
	/// Bounded Artifact bytes.
	ChangeArtifact(crate::ChangeArtifactChunk),
	/// Immutable boundaries and their patch preview.
	ChangeDiff(Box<crate::ChangeDiff>),
	/// Bounded current Turn queue.
	TurnQueue(crate::TurnQueue),
	/// A bounded page of unsafe execution matches.
	OrphanedExecutions(crate::OrphanedExecutions),
	/// Lifecycle, activity, and Managed processes at one journal cursor.
	RunExecution(crate::RunExecution),
	/// Snapshot of the daemon's Plane status.
	Status(PlaneStatus),
	/// One page of Conversations on the Plane.
	Conversations(ConversationList),
	/// One Conversation with all of its Runs. Boxed: the Workspace it
	/// carries, with its promotion, outweighs every other snapshot.
	Conversation(Box<ConversationSnapshot>),
	/// What the Plane can do.
	Capabilities(CapabilitySnapshot),
	/// Every Account binding on the Plane.
	AccountBindings(AccountBindingList),
	/// What one Plane knows about Usage for the selected scope. Boxed: it
	/// carries the Plane's quota windows beside its per-Model totals.
	Usage(Box<crate::PlaneUsage>),
	/// Settings resolved for one scope.
	Settings(SettingSnapshot),
	/// One page of journal Events in sequence order.
	Events(EventPage),
	/// The Plane's Pairing as it stands.
	Pairing(PairingSnapshot),
	/// One page of the Security audit, oldest first.
	SecurityAudit(AuditPage),
	/// Every registered Project on the Plane.
	Projects(ProjectList),
	/// What a Path grant would register.
	ProjectPreview(ProjectPreview),
	/// What one path inside a Project names.
	ProjectEntry(ProjectEntry),
	/// What promoting a Workspace would do. Boxed: the preview carries
	/// two lists and six object names, far more than any other snapshot.
	PromotionPreview(Box<PromotionPreview>),
	/// Bounded ranked hits, best match first.
	Search(SearchResult),
	/// What the Plane can see outside its management, and what it holds.
	ExternalConversations(ExternalConversationList),
}
