//! Typed wire Queries and their replies.

use super::{EventPage, PlaneStatus};
use crate::{
	account::AccountBindingList,
	audit::SecurityAudit,
	capability::{CapabilityObservation, CapabilitySnapshot},
	conversation::{
		ConversationList, ConversationSnapshot, PageCursor,
		import::ExternalConversationList,
		promotion::{PromotionDestination, PromotionPreview},
	},
	pairing::PairingSnapshot,
	project::{ProjectEntry, ProjectList, ProjectPreview},
	search::SearchResult,
	setting::{SettingScope, SettingSelection, SettingSnapshot},
	usage::{PlaneUsage, UsageSelection},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Queries a client may run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueryRequest {
	/// Latest 100 durable Git operations, newest first.
	GitDeliveries {
		/// Owning Conversation.
		conversation_id: Uuid,
	},
	/// Inspect an Auto-continue policy and retry (Jet 1.34).
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
	/// Discover extensions in native sources through the responsible Craft.
	ExtensionCatalog {
		/// Accepted Craft identity.
		craft_id: String,
	},
	/// Inspect a staged extension mutation.
	ExtensionChange {
		/// Durable operation identity.
		change_id: Uuid,
	},
	/// Inspect the exact remote action awaiting a destination review.
	RemoteToolReview {
		/// Paired installation that submitted it.
		client_id: Uuid,
		/// Remote operation identity.
		operation_id: uuid::Uuid,
	},
	/// Read a durable Utility result.
	Utility {
		/// Plane-assigned job identity.
		job_id: Uuid,
	},
	/// Verify one third-party Craft source and return its exact consent surface.
	DiscoverCraft {
		/// Repository release or explicit Developer Mode source.
		source: crate::CraftSource,
	},
	/// Enabled schedules in one Conversation.
	ScheduledTasks {
		/// Owning Conversation.
		conversation_id: Uuid,
	},
	/// Read bounded UTF-8 file content through a registered root.
	EditableFile {
		/// Registered Project or Workspace root.
		target: crate::FileTarget,
		/// Path relative to that root.
		path: String,
	},
	/// All retained terminals of one Workspace (at most 64).
	WorkspaceTerminals {
		/// Registered Workspace identity.
		workspace_id: Uuid,
	},
	/// Read a bounded chunk of a checkpoint patch Artifact.
	ChangeArtifact {
		/// Canonical SHA-256 content address.
		sha256: String,
		/// Byte offset in decimal-string form.
		#[serde(with = "crate::transport::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		offset: u64,
	},
	/// Compare observed Change checkpoints.
	ChangeDiff {
		/// Owning Run.
		run_id: Uuid,
		/// Boundaries to compare.
		scope: crate::DiffScope,
	},
	/// Continue changed-file metadata at an opaque, expiring snapshot cursor.
	NextChangeDiff {
		/// Cursor supplied by the preceding diff page.
		cursor: crate::PageCursor,
	},
	/// Current queue, with a position given by vector order.
	TurnQueue {
		/// Conversation whose queue is read.
		conversation_id: Uuid,
	},
	/// Bounded Orphaned-execution metadata for interactive decisions.
	OrphanedExecutions {
		/// Continue after a previous page.
		#[serde(default)]
		after: Option<Uuid>,
	},
	/// Read the durable execution state of a managed Run.
	RunExecution {
		/// Run identity.
		run_id: Uuid,
	},
	/// Snapshot of the Plane's daemon status.
	Status,
	/// First bounded page of Conversations on the Plane.
	Conversations,
	/// Continue a fenced Conversation keyset snapshot.
	NextConversations {
		/// Opaque token returned by the previous page.
		cursor: PageCursor,
	},
	/// One Conversation with all of its Runs.
	Conversation {
		/// The Conversation to read.
		conversation_id: Uuid,
	},
	/// What the Plane can do.
	Capabilities {
		/// Whether to report the last observation or take a new one.
		observation: CapabilityObservation,
	},
	/// Every Account binding on the Plane, with the state of the Credential
	/// each one resolves.
	AccountBindings {
		/// Whether the Credential states follow the last observation of the
		/// Plane or a new one, taken now.
		observation: CapabilityObservation,
	},
	/// The Usage records the Plane holds for one scope, with the freshness
	/// and estimation each of them carries.
	Usage {
		/// What the answer covers.
		selection: UsageSelection,
	},
	/// Settings resolved for one scope.
	Settings {
		/// The scope to resolve for; its own values win over the Plane's.
		scope: SettingScope,
		/// Which Settings to resolve.
		selection: SettingSelection,
	},
	/// A page of journal Events strictly after a sequence.
	Events {
		/// The sequence to resume after, carried as a decimal string
		/// (ADR-0089); `"0"` for the whole journal.
		#[serde(with = "crate::transport::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		after: u64,
	},
	/// The Plane's Pairing: whether it accepts new GUI clients.
	Pairing,
	/// A page of the owner-only Security audit strictly after a position.
	SecurityAudit {
		/// The position to resume after, carried as a decimal string
		/// (ADR-0089); `"0"` for the whole audit.
		#[serde(with = "crate::transport::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		after: u64,
	},
	/// Every registered Project on the Plane.
	Projects,
	/// What registering the Git working tree at an absolute path would
	/// record, before the Path grant is made: the directory it resolves to
	/// and what the Plane's Git says about it.
	PreviewProject {
		/// The absolute path the user is about to grant.
		path: String,
		/// Whether Git LFS is reported from the last observation of the
		/// Plane or a new one, taken now.
		observation: CapabilityObservation,
	},
	/// What one path inside a Project names. This is how every ordinary
	/// file operation addresses a file: a Project and a path relative to
	/// its root, which the Plane validates before touching the filesystem.
	ProjectEntry {
		/// The Project to resolve the path in.
		project_id: Uuid,
		/// The path relative to the Project's root, with `/` between its
		/// components.
		path: String,
	},
	/// What promoting a Workspace to a permanent checkout or branch of its
	/// Project would do, before it is done. The answer binds what it
	/// compared, and a promotion carries that binding back.
	PreviewPromotion {
		/// The Workspace to promote.
		workspace_id: Uuid,
		/// Where its changes would go.
		destination: PromotionDestination,
	},
	/// Bounded ranked hits over this Plane's human-visible Conversation
	/// content. A GUI merges the answers of every Plane it is connected
	/// to.
	Search {
		/// At most 256 characters, whose whitespace-separated terms every
		/// hit must contain, sixteen at most.
		text: String,
	},
	/// The Harness-native Conversations the Plane can see outside its
	/// management, placed against its Projects, and the imports it holds.
	ExternalConversations,
}

/// Query snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueryResponse {
	/// Individually attributed Git outcomes.
	GitDeliveries {
		/// Newest operations, bounded to 100.
		deliveries: Vec<crate::GitDelivery>,
	},
	/// Durable Auto-continue policy and decision.
	AutoContinue(crate::AutoContinueSnapshot),
	/// Unconverted native catalog and metadata.
	ExtensionCatalog(crate::ExtensionCatalog),
	/// Deferred native mutation outcome.
	ExtensionChange(crate::ExtensionChange),
	/// Exact remote action awaiting a destination review.
	RemoteToolReview(crate::RemoteToolRequest),
	/// Attributed Utility result, with no execution authority.
	Utility(crate::UtilityJob),
	/// Verified immutable Craft installation proposal.
	CraftInstallationPreview(crate::CraftInstallationPreview),
	/// Fenced schedule snapshot.
	ScheduledTasks(crate::ScheduledTasks),
	/// Bounded editable content and its exact file Revision.
	EditableFile(crate::EditableFile),
	/// Terminal lifecycle snapshots, separate from Runs.
	WorkspaceTerminals {
		/// Snapshot Event cursor.
		#[serde(with = "crate::transport::decimal")]
		#[cfg_attr(feature = "schema", schemars(with = "crate::Decimal"))]
		cursor: u64,
		/// Retained Workspace terminals.
		terminals: Vec<crate::WorkspaceTerminal>,
	},
	/// Bounded Artifact bytes.
	ChangeArtifact(crate::ChangeArtifactChunk),
	/// Immutable boundaries and patch preview.
	ChangeDiff(Box<crate::ChangeDiff>),
	/// Fenced Turn queue snapshot.
	TurnQueue(crate::TurnQueue),
	/// Read-only recovery candidates.
	OrphanedExecutions(crate::OrphanedExecutions),
	/// Durable lifecycle, activity, and Managed processes of a Run.
	RunExecution(crate::RunExecution),
	/// Snapshot of the Plane's daemon status.
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
	Usage(Box<PlaneUsage>),
	/// Settings resolved for one scope.
	Settings(SettingSnapshot),
	/// One page of journal Events in sequence order.
	Events(EventPage),
	/// The Plane's Pairing as it stands.
	Pairing(PairingSnapshot),
	/// One page of the Security audit, oldest first.
	SecurityAudit(SecurityAudit),
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
