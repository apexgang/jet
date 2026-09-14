# Plane transfer

Issue #51 implements ADR-0070 in the core: moving one Conversation's Home
Plane (ADR-0062) through prepare, relinquish, and commit phases, with an
Authority fence kept outside SQLite the way the Deletion ledger of
ADR-0102 is. The Commands use the core binary interface, like [Recovery
bundles](recovery-bundles.md): the bundle is bounded bytes in process, and
the JSON Command transport does not carry it. A GUI transport that streams
the bundle between two authenticated Plane connections is separate work.
The Transfer tombstone's Jet Trash reason, `plane_transfer`, is on the wire
behind protocol minor 42.

## Authority and epochs

Every Conversation carries an authority and an epoch. A Conversation
created on a Plane is `home` there in epoch 1. A transfer retires the
source's epoch and opens the next one on the target; a Conversation moved
twice is in epoch 3 wherever it ends up. The epoch is what the fence
names, so a Conversation that later comes back to a Plane it left is not
the one that Plane fenced.

| Authority | Meaning | New work |
|-----------|---------|----------|
| `home` | this Plane owns the Conversation | admitted, unless a source transfer is prepared |
| `prepared` | a validated import, waiting for its source to relinquish | refused |
| `relinquished` | the Transfer tombstone left after relinquishing | refused |

Run creation and admission, turn admission from every source, dispatch of
queued turns, schedule firing, and renaming all ask the same question
first. A refused Command
answers `conversation.not_authoritative` or, for a source that has prepared
a transfer, `conversation.transfer_prepared`; a schedule or a dispatch
simply waits, and fires once the transfer is committed on the new Home
Plane or aborted on the old one.

## The phases

**Prepare**, on the source: `prepare_plane_transfer` names the
Conversation and the target Plane. It refuses a Conversation that is not
this Plane's own (`conversation.not_authoritative`), a target that is this
Plane (`transfer.same_plane`), live work, a Run that has not ended or a
queued turn (`transfer.live_work`), and a transfer already prepared to
another target (`transfer.in_progress`), and one in Jet Trash
(`transfer.trashed`); while it stays prepared, forgetting or deleting the
Conversation here is refused too (`conversation.transfer_prepared`). It records the transfer with the
bundle's SHA-256, which freezes the Conversation, and returns the transfer
and the bundle. The bundle is never receipted: a second prepare of the
same Conversation to the same target reads the frozen Conversation again,
produces the same bytes, and checks their hash against the recorded one;
`transfer.bundle_drift` means something changed underneath and the
transfer should be aborted and prepared anew, and `transfer.stale` means
the Conversation moved between the read and the record and the prepare
should simply be repeated.

**Import**, on the target: `import_plane_transfer` carries the bundle and,
when the Conversation worked in a Project on its source, the Project on
this Plane it continues in; it is refused without one
(`transfer.project_required`). The bundle is checked before anything is
written: its format, every bound, that it was prepared for this Plane
(`transfer.wrong_plane`), that every Event names a Run it carries, that
every Run has ended, and that every Artifact reference has its payload
exactly; anything else is `transfer.invalid_bundle`. Payloads are
published under their hashes through the same verification and budget as
an uploaded Artifact. One transaction then records the Conversation under
its own identity as `prepared` in the next epoch, with its name, its Runs,
its transcript, its queued turns, its schedules, its Conversation-scoped
Settings, and its Artifact references, and the transfer itself. The
Conversation continues in the named Project's Local checkout; a managed
Workspace is not created for it, and its origin is recorded as new, with
provenance in the transfer record and the journal. A Conversation this
Plane relinquished earlier gives up its tombstone to the Conversation
coming back in a later epoch;
one it holds in any other authority refuses the import
(`transfer.conversation_exists`). The same bundle imported again answers
the same transfer.

**Relinquish**, on the source: `relinquish_plane_transfer` names the
transfer and the bundle hash the target imported, and refuses any other
hash (`transfer.bundle_mismatch`). In one transaction the Conversation
becomes `relinquished`, the transfer settles, the content enters Jet Trash
as a Transfer tombstone that expires after thirty days, and the Authority
fence is appended to its ledger before the commit, so no relinquishment is
acknowledged that a restored snapshot could undo. The reply is the fence.
Relinquishing again answers the same fence.

**Commit**, on the target: `commit_plane_transfer` carries the transfer,
the bundle hash, and the fence. The fence must name exactly this transfer,
this Conversation, the epoch being retired, this Plane as its target, and
the source the bundle was prepared from (`transfer.fence_invalid`). The
Prepared transfer becomes `home` in the next epoch, the transfer settles,
and schedules
and queued turns resume. Committing again answers again.

**Abort**, on the source: `abort_plane_transfer` forgets a transfer this
Plane prepared and has not relinquished, and the Conversation takes new
work again. After relinquishing there is nothing to abort
(`transfer.relinquished`). A target's Prepared transfer is not aborted; it
is forgotten through Jet Trash like any Conversation.

Every step is recorded in the journal as `conversation.transfer_prepared`,
`conversation.transfer_imported`, `conversation.transfer_relinquished`,
`conversation.transfer_committed`, or `conversation.transfer_aborted`, and
in the Security audit as `transfer.prepared`, `transfer.imported`,
`transfer.relinquished` with destructive risk, `transfer.committed`, or
`transfer.aborted`.

## The bundle

The bundle is the same bounded container a Recovery bundle uses, with its
own magic so neither is read as the other, holding `transfer.json` and one
`artifacts/<sha256>` component per referenced payload, 256 MiB in all. It
carries the Conversation, its retention, name, and creation time; its Runs
with their terminal lifecycle and names; its transcript Events, the
`turn.input`, `run.output`, and `artifact.published` kinds, with the Client
that recorded each; its queued turns and schedules as stored; its
Conversation-scoped Settings; and its Artifact references. Account
bindings, credentials, native Harness session files, execution plans,
Workspace directories, and authorization Events stay on the source, as
they do in a portable Recovery snapshot (ADR-0074). The target follows the
existing native-resume rules from there: no native session travels.

## Authority fences

Before the source commits a relinquishment, `jetd` appends the fence to
`~/.jet/recovery/plane.sqlite3.fences`, an owner-only text file beside the
Deletion ledger with the same shape: a format line, the Plane, and one line
per fence carrying its sequence, the time, the Conversation, the retired
epoch, the transfer, the target Plane, and a SHA-256 link folding the line
before it. Its durable head lives in `plane.sqlite3.fences.head`, and the
store counts the fences it has applied in its Plane row, so a snapshot
says which fences it predates and a ledger gone missing is noticed.

Every open of an authoritative store reapplies the fences: a Conversation
still claiming a fenced epoch as `home` is set to `relinquished`. That is
what a restored snapshot from before the transfer meets, and what a crash
between the fence and its commit leaves for the next open to finish; a
relinquish retried after such a crash settles the transfer and leaves the
tombstone without raising the fence a second time.
Fences that cannot be trusted, because the file or its head is missing or
altered, refuse a snapshot restoration and any new relinquishment, as a
corrupt Deletion ledger does. The fence is content-free and permanent: the
tombstone expires, the fence does not.

## The Transfer tombstone

The source keeps the relinquished Conversation's content in Jet Trash under
the `plane_transfer` reason for thirty days, for recent recovery and
explanation, then deletes it through the Deletion ledger like any expired
entry. It cannot be restored (`retention.transferred`): the fence keeps it
from becoming a Home Plane again. A peer at a protocol minor below 42 is
refused a Trash page holding one rather than shown a reason it would
misread.

## What is not here

The optional Workspace snapshot of ADR-0070 is not selected by this
interface; the target continues in a Local checkout of the mapped Project.
A GUI transport for the bundle, and a Query over a Plane's transfers, are
separate work.
