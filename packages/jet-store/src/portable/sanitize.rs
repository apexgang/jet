//! Removes Plane-local authority and authentication from a private SQLite copy.
use crate::StoreError;
use sqlx::SqliteConnection;

pub(super) async fn sanitize(
	connection: &mut SqliteConnection,
) -> Result<(), StoreError> {
	super::validate::columns(connection).await?;
	// ASVS 14.2.4: an explicit schema allowlist fails closed when storage grows.
	// sqlite_master is inspected before trusting a portable schema, as at Store::open.
	let tables: Vec<String> = sqlx::query_scalar(
		"SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
	)
	.fetch_all(&mut *connection)
	.await?;
	for name in tables {
		if !TABLES.contains(&name.as_str()) {
			return Err(StoreError::Integrity(
				"unsupported portable snapshot schema".into(),
			));
		}
	}
	sqlx::query!("DELETE FROM account_bindings")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM audit_epochs")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM audit_state")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM auto_continue_policies")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM autodelete_rules")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM change_checkpoints")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM command_receipts")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM conversation_fork_launches")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM conversation_trash")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM craft_disables")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM craft_installation_plans")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM craft_revocations")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM effects")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM event_journal_state")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM execution_resolutions")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM extension_changes")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM git_branches")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM git_deliveries")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM git_drafts")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM imported_conversations")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM orphaned_executions")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM paired_clients")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM pairing_gate")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM pairing_offers")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM plane_transfers")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM plane")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM projects")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM remote_operations")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM run_executions")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM search_index_state")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM search_name_index_state")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM security_audit")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM turn_queues")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM usage_aggregates")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM usage_dirty_hours")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM usage_observations")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM usage_provider_reach")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM usage_quota_snapshots")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM user_edit_intents")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM utility_jobs")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM workspace_promotion_conflicts")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM workspace_promotions")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM workspace_terminals")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM workspaces")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM search_documents")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM search_name_documents")
		.execute(&mut *connection)
		.await?;
	sqlx::query!("DELETE FROM settings WHERE key NOT IN ('storage.disposable_mib', 'git.auto_branch', 'git.auto_push', 'git.auto_draft_pull_request', 'git.branch_prefix', 'utility.automatic_naming', 'git.auto_commit', 'git.message_instructions', 'retention.trash_grace_days', 'security.audit_retention_days', 'utility.git_text', 'utility.content_consent', 'utility.autodelete_compilation', 'craft.developer_mode', 'review.automatic', 'review.cross_provider_consent', 'artifact.max_mib', 'artifact.run_mib', 'energy.concurrency', 'energy.low_power_concurrency', 'energy.constrained', 'energy.foreground_override') OR scope = 'project'").execute(&mut *connection).await?;
	// Only transcript and payload content is portable. Account, trust, execution
	// plans, native-resume tokens, and authorization Events never leave the Plane.
	sqlx::query!("DELETE FROM events WHERE conversation_id IS NULL OR kind NOT IN ('turn.input', 'run.output', 'artifact.published')").execute(&mut *connection).await?;
	// Rebuild the portable payload fields, dropping internal Actor/provenance
	// annotations rather than trying to enumerate every secret metadata key.
	sqlx::query!("UPDATE events SET payload = CASE kind WHEN 'turn.input' THEN json_object('text', json_extract(payload, '$.text'), 'turn_id', json_extract(payload, '$.turn_id')) WHEN 'run.output' THEN json_object('native_json', json_extract(payload, '$.native_json'), 'presentation_json', json_extract(payload, '$.presentation_json')) WHEN 'artifact.published' THEN json_object('artifact', json_object('sha256', json_extract(payload, '$.artifact.sha256'), 'size', json_extract(payload, '$.artifact.size'))) END").execute(&mut *connection).await?;
	sqlx::query!("UPDATE events SET actor_kind = 'recovery', actor_id = NULL")
		.execute(&mut *connection)
		.await?;
	// A portable copy claims no authority: its Conversations get fresh
	// identities on import, in a first epoch of their own (ADR-0070).
	sqlx::query!("UPDATE conversations SET working_tree = 'none', project_id = NULL, import_id = NULL, fork_source_conversation_id = NULL, fork_source_run_id = NULL, fork_checkpoint_turn = NULL, authority = 'home', authority_epoch = 1").execute(&mut *connection).await?;
	// Schedules survive as inert metadata, without the Client that authorized them.
	sqlx::query!("UPDATE scheduled_tasks SET next_due_unix_ms = 9223372036854775807, state = json_set(json_remove(state, '$.authorized_by', '$.next'), '$.enabled', json('false'))").execute(&mut *connection).await?;
	Ok(())
}
const TABLES: &[&str] = &[
	"_sqlx_migrations",
	"account_bindings",
	"artifact_references",
	"audit_epochs",
	"audit_state",
	"auto_continue_policies",
	"autodelete_rules",
	"change_checkpoints",
	"command_receipts",
	"conversation_fork_launches",
	"conversation_trash",
	"conversations",
	"craft_disables",
	"craft_installation_plans",
	"craft_revocations",
	"effects",
	"event_journal_state",
	"events",
	"execution_resolutions",
	"extension_changes",
	"git_branches",
	"git_deliveries",
	"git_drafts",
	"imported_conversations",
	"orphaned_executions",
	"paired_clients",
	"pairing_gate",
	"pairing_offers",
	"plane",
	"plane_transfers",
	"projects",
	"remote_operations",
	"run_executions",
	"runs",
	"scheduled_tasks",
	"search_documents",
	"search_documents_config",
	"search_documents_content",
	"search_documents_data",
	"search_documents_docsize",
	"search_documents_idx",
	"search_index_state",
	"search_name_documents",
	"search_name_documents_config",
	"search_name_documents_content",
	"search_name_documents_data",
	"search_name_documents_docsize",
	"search_name_documents_idx",
	"search_name_index_state",
	"security_audit",
	"settings",
	"sqlite_sequence",
	"turn_queues",
	"usage_aggregates",
	"usage_dirty_hours",
	"usage_observations",
	"usage_provider_reach",
	"usage_quota_snapshots",
	"user_edit_intents",
	"utility_jobs",
	"workspace_promotion_conflicts",
	"workspace_promotions",
	"workspace_terminals",
	"workspaces",
];
