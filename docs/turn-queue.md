# Turn queue

Issue #22 implements ADR-0024 and ADR-0037 through Jet protocol minor 16.

`submit_turn` takes `conversation_id`, `prompt`, and an optional `source`
(`user`, `schedule`, or `auto_continue`; the default is `user`). Its durable
`turn_admitted` result includes the Plane-assigned `turn_id`, Conversation
`sequence`, authenticated `client_id`, source, state, and eventual `run_id`.
The source chooses a coalescing policy; it never supplies Actor authority.
Scheduled-task timing and Auto-continue policies feed this admission boundary;
this feature does not implement their timers or policy evaluation.

Each new admission receives a strictly increasing Conversation sequence. User
turns are never replaced. Each background source has one replaceable **pending**
slot, and its replacement receives a fresh identity and sequence. New user
input cancels pending Auto-continue. Claimed work is never coalesced. The next
turn is always the remaining entry with the lowest sequence.

`withdraw_turn` takes `conversation_id` and `turn_id`. Only the authenticated
client that admitted a still-queued user turn may withdraw it. Withdrawal is
separate from interruption and Run termination. Exact Command retries return
their original admission or withdrawal receipt, including after restart.

The `turn_queue` Query (also `Client::turn_queue`) returns a bounded snapshot
and its Event cursor. Vector order is queue position, with any claimed turn
first. Sequences and cursors cross the wire as decimal strings. `turn.changed`
Events preserve admission, claim, replacement, cancellation, withdrawal,
completion, failure, and uncertain-outcome metadata. `turn.input` Events retain
the original prompt in segments of at most 8192 UTF-8 bytes; concatenate a
turn's segments in Event order. All segments, queue changes, and the receipt
commit atomically. Prompt text never enters diagnostics or the Security audit.

Admission accepts 1–65536 UTF-8 bytes per prompt. A queue holds at most 128
unsettled entries and 1 MiB of prompt text. Replacement candidates are excluded
when evaluating these bounds; a refused admission changes neither the queue
nor its Events. Larger input requires the separate Artifact workflow.

The first `start_run` selects and pins the Craft. Its prompt enters this same
queue, so previously admitted input keeps its earlier position. Before that
selection, input may wait durably without a Run. Later pending work starts a
new Run using the accepted pin when no Run is live; it revalidates the working
roots and Craft before launch. New Runs refresh OS boot evidence and explicitly
resume the last recorded native Conversation with the pinned Craft protocol.
Existing managed Runs supply the pin after an upgrade, too. An unavailable
artifact or unsupported resume keeps the input pending. A busy Local checkout preserves pending work
until execution becomes available. Background work never chooses a different
Craft implicitly.

Within a Run, delivery requires a durable claim, an explicit matching Craft
completion of the preceding input, and a fully committed native source
boundary. Activity such as waiting for user input or approval does not release
the turn. Frame reads remain intact while queue wakeups send input through the
independent writer. Restart reattaches the existing execution and reconciles
source; it never redelivers a claimed input. A lost connection or unverified
recovered claim is observable as `outcome_unknown` until completion or proven
Run termination settles it. A completion for another identity cannot advance
the queue. Unfinished input is recorded as failed when its Run ends, while
pending entries remain available for later Runs. Proven process death releases
an incomplete source prefix without acknowledging unavailable source or
discarding already committed semantic Events.

The focused conformance suite is `just test -p jet-daemon --test turns`, run
from `packages/`. It uses public Commands, Queries, and Events, a real SQLite
store, daemon restart, and controlled Craft/helper/Harness subprocesses. The
implementation crosses protocol, transactional storage, and native execution
boundaries: admission alone would preserve input without executing it, so
those parts belong to the same feature. Queue policy, dispatch, and later-Run
admission live in separate modules for review.
