# Project removal

Issue #54 implements the Project-removal half of ADR-0011 through Jet
protocol minor 41, and joins it to the Deletion ledger of ADR-0102 and
the Security audit of ADR-0105.

Unregistering a Project is not implemented in this core; removal is. It
is the one Command that destroys a directory the user owns, so it is
made in two phases, by an interactive user, and by nobody else.

## The preview

`preview_project_removal` reads what removing one Project would meet and
lose, and answers a **binding** the removal carries back:

| Field | Meaning |
|-------|---------|
| `root` | the canonical root, the directory the removal takes |
| `live_runs` | Runs of its Conversations that have not ended |
| `schedules` | Scheduled tasks of its Conversations |
| `dirty_files` | lines of `git status`: changed files and untracked entries, lost with it |
| `unpushed_commits` | commits no remote branch holds, lost with it |
| `workspaces` | the Workspaces created from it, removed with it |
| `actor` | the Client the preview was shown to |

Beside the binding the preview discloses `disk_use_bytes`, which is not
bound because a repository's size moves on its own; `obstacles`, what
refuses the removal today; and `permanent_removal_warning`, the text a
permanent removal acknowledges. The store is read, the checkout is
inspected with Git outside any transaction, and the live work is read
last, so the binding is as fresh as it can be. Conversations in a
Workspace and in the Local checkout both count.

## What refuses a removal

| Obstacle | Refusal |
|----------|---------|
| `live_runs`, `schedules` | `project.live_work` |
| `filesystem_root`, `user_home`, `jet_home` | `project.protected_root` |
| `contains_project` | `project.contains_project` |

`jet_home` covers a root that is Jet's home, holds it, or lies inside
it: everything under `~/.jet` is Jet's, the Workspaces included. A Project
that holds another registered Project is refused until the inner one is
removed, so nothing is taken that another registration still names.

## The Command

`remove_project` carries the binding, the Project directory's own name
typed by the user, and where the directory goes. Before its transaction
opens, the Plane computes the preview again and refuses:

| Code | When |
|------|------|
| `project.removal_unbound` | the binding was shown to another Client |
| `project.not_found` | the Project is no longer registered |
| `project.removal_stale` | the Project has moved past the binding |
| `project.name_mismatch` | the typed name is not the directory's own |
| `project.warning_unacknowledged` | a permanent removal's warning is not word for word |

Inside the transaction the live work is read once more, so work admitted
since the preparation still refuses it. The removal then takes the
Project's Workspaces and what hangs off them, detaches its Conversations
into no Project, keeps everything else they own, removes the
registration, appends the identity to the Deletion ledger so a restored
snapshot cannot bring it back, appends `project.removed` to the journal,
and records `project.removed` in the Security audit as a destructive
decision. Reapplying the ledger at open runs the same statements for a
`project` line.

The directory is disposed of inside the same transaction, after the
audit record and before the commit: a directory the Plane cannot dispose
of leaves the registration in place and the Command refused, and the
Plane's one writer waits while it moves. A root that is already gone,
removed by hand or by an attempt the Plane could not commit, needs
nothing and counts as deleted, so its registration can still be
removed. The Workspace directories follow after the commit's own work,
best effort, the way a deleted Conversation's does.

## Where the directory goes

`system_trash` moves the directory to the system Trash, from where the
user can bring it back. Where the Plane has no Trash for the directory
the Command answers `project.trash_unavailable` and removes nothing.
`permanent` deletes it for good and is the second authorization: it
carries the `permanent_removal_warning` the preview published, word for
word. The Plane cannot tell whether a Trash exists without trying, so a
client asks for the Trash first and offers the permanent removal only
against that refusal.

## Who can ask

Both Actors this core knows are interactive, and the removal's
admission matches them exhaustively, so a Harness, Craft, Scheduled-task,
Utility, or automatic Actor added later fails to compile there rather
than being admitted. No sweep constructs the Command, and it exists in
no Craft or remote-tool contract.

## Tests

`just test -p jet-store project` covers the counts, the cascade, and the
ledger; `just test -p jet-core removal` covers the preview, the
refusals, the obstacles, a removal over a real repository with a
Workspace, and the system Trash on Linux, purging the entry it made;
`just test -p jet-daemon --test projects` drives a real daemon through a
preview and a permanent removal.
