# Retention, forgetting, and Jet Trash

Issue #53 implements ADR-0001, ADR-0011, and ADR-0015 through Jet
protocol minor 39, and joins Conversation deletion to the Deletion ledger
of ADR-0102 and the Security audit of ADR-0105.

A Conversation is retained by default. Nothing Jet does on its own removes
a retained Conversation, and nothing removes any Conversation at once:
every path stages the Conversation in Jet Trash with its reason recorded,
and the deletion itself happens when the grace period ends, unless the
Conversation is restored first.

## Three ways in

| Path | Who | What is refused | Reason recorded |
|------|-----|-----------------|-----------------|
| `forget_conversation` | an interactive or Paired client | a live Run or a queued turn (`retention.live_work`) | `manual_forget` |
| `delete_conversation_everywhere` | an interactive or Paired client | a Run still launching (`retention.run_starting`) | `delete_everywhere` |
| the retention sweep | the Plane, on the policy `forget_after_final_run` | any protection below | `automatic_forget` |
| the Autodelete sweep | the Plane, on an approved [Autodelete rule](autodelete.md) | any protection below | `autodelete_rule`, or `autodelete_everywhere` |

Forgetting removes what Jet owns about the Conversation and leaves the
Harness's own history where it is. Deleting everywhere additionally stops
the active Run through the same escalation a Stop Run request uses
(ADR-0083), cancels every queued turn, and records that the Harness's
native history is to be requested too when the grace period ends. No
bundled Craft offers native deletion in this release, so that request
has nowhere to go yet; the reason is recorded so a later Craft can act on
it, and the Harness's history stays.

Each Command is a Guarded Security decision: refused while the Plane
cannot vouch for its audit (ADR-0105), and recorded as
`conversation.forgotten`, `conversation.deletion_authorized`, or
`conversation.restored` with the Conversation as target. Staging and
restoring also append `conversation.trashed` and `conversation.restored`
Events to the Conversation's journal. Asking twice answers
`retention.already_trashed`; restoring what is not staged answers
`retention.not_trashed`.

## What protects a Conversation

The `retention_preview` Query reads, in this order, whatever protects a
Conversation today:

| Protection | Meaning |
|------------|---------|
| `active_run` | a Run has not ended |
| `pending_turn` | a turn is queued or claimed |
| `enabled_schedule` | a Scheduled task will queue more turns |
| `dirty_workspace` | the Workspace has uncommitted changes or untracked files |
| `unpushed_work` | the Workspace has commits past its base that no remote branch contains |
| `unresolved_effect` | an Effect of one of its Runs, or a Git delivery, is pending or in flight |

Automatic forgetting waits for all of them. Manual forgetting refuses only
the first two: the rest are the owner's to weigh, and the preview shows
them. The Workspace is inspected with Git outside any store transaction;
a Workspace whose directory is gone has nothing to lose, and one Git
cannot inspect counts as protected for that sweep and answers
`retention.workspace_unreadable` in the preview. A Conversation in the
Project's Local checkout has no managed Workspace and no Workspace
protection: that checkout is the user's.

ADR-0001 also names a pin in a shared or private Conversation layout as a
protection. Layouts and pins are not implemented in this core, so nothing
can be pinned yet; the protection joins this list with them.

The preview also discloses how many Security-audit records name the
Conversation, and whether it is already staged.

## The sweep

The daemon's maintenance loop runs the retention sweep on every wakeup,
after schedules and recovery. It is idle unless something is due: the
loop's deadline includes the soonest Trash expiry. The sweep does nothing
while the Plane is in Recovery mode or its Security audit is degraded.

Staging pages through every Conversation with the policy
`forget_after_final_run` that has had at least one Run and is not staged. The store is read once
to decide and again inside the transaction that stages, so work admitted
in between is not forgotten. The sweep records `conversation.forgotten`
under the audit actor `retention`, which needs minor 39 to read; a client
on an older minor is answered `audit.actor_incompatible` for a page that
includes one, the same way Craft revocations are gated.

Expiry deletes every staged Conversation whose grace period has ended,
taking the day's Recovery snapshot first if none exists yet (ADR-0097).
Nothing refuses new work on a staged Conversation, so a Run, a queued
turn, or an Effect still in flight at expiry holds the deletion: the
entry stays expired and the next sweep asks again once the work has
ended. Restoring is the way to keep the Conversation.
One transaction removes the Conversation's Runs, Workspace row,
promotions, terminals, checkpoints, Artifact references, Usage records,
Git deliveries, schedules, queue, fork launches, search documents, and
journal Events; appends the identity to the Deletion ledger so a restored
snapshot cannot bring it back (ADR-0102); records `conversation.deleted`;
and then clears the identity from every audit record about the
Conversation, that one included, leaving only the opaque reference they
were chained over (ADR-0105). After the commit the Workspace directory is
removed. Artifact payloads are not touched: with their references gone
they are unreferenced, and collection takes them after its grace. The
Harness's own history stays.

Removing journal Events moves the journal's replay floor past them, so a
cursor from before the deletion is answered with an expiration and takes
a fresh snapshot rather than being replayed across the gap (ADR-0078).
Restoring a database snapshot reapplies the ledger, and a `conversation`
line removes the same rows again. Removing a Project is its own
two-phase operation, described in [Project removal](project-removal.md).

## The grace period

`retention.trash_grace_days` is a Plane-scoped Setting with a built-in
default of 30 days and a floor of one day. Changing it is recorded as
`policy.trash_grace_changed` or `policy.trash_grace_cleared`. The grace
period is fixed when a Conversation is staged; a later change moves
nothing already in the Trash.

## Wire surface

Minor 39 adds the Commands `forget_conversation`,
`delete_conversation_everywhere`, and `restore_conversation`, with results
`conversation_trashed` and `conversation_restored`; the Queries
`conversation_trash` and `retention_preview`; the Setting
`retention.trash_grace_days`; and the audit actor `retention`. Older
peers cannot admit any of them.

## Tests

`just test -p jet-store trash` covers the rows, the protection queries,
the cascade, and the ledger; `just test -p jet-core retention` covers the
Commands, the protections, the Workspace inspection over a real
repository, and the sweep through staging and deletion; `just test -p
jet-daemon --test retention` drives a real daemon through forgetting,
previewing, and restoring.
