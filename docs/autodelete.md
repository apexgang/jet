# Autodelete rules

Issue #55 implements ADR-0015 through Jet protocol minor 40, on top of the
Utility work of ADR-0099 and the Jet Trash of [Retention](retention.md).

An Autodelete rule starts as natural language. The Utility model compiles
it once into one bounded predicate, whole days of inactivity, and that is
the last the model has to do with it: the rule the sweep evaluates is the
stored interpretation its owner saw and approved, and nothing the model
produced ever becomes a Command. A compilation that was unavailable,
ambiguous, out of schema, or refused leaves the rule refused. It shows no
interpretation, selects nothing, and cannot be approved.

## Life of a rule

| Command | What it does | Leaves the rule |
|---------|--------------|-----------------|
| `compile_autodelete_rule` | admits one `autodelete` Utility job for the prompt; a second call on the same `rule_id` is a source edit | compiling, then the job's draft or refused |
| `set_autodelete_rule_inactive_days` | sets the interpretation by hand, keeping the source | a draft of exactly that inactivity |
| `approve_autodelete_rule` | approves the interpretation shown, named in the Command | approved |
| `authorize_autodelete_everywhere` | separately authorizes native deletion of matches | approved, scope `everywhere` |
| `delete_autodelete_rule` | removes the rule | gone; what it staged stays in Jet Trash |

Either edit, of the source or of the interpretation, clears approval and
the delete-everywhere authorization together: the owner approved one
reading, and a different reading is a different rule. For the same reason
the approval names the inactivity the owner was shown; a draft that reads
differently by then, because another client edited or recompiled it,
answers `autodelete.interpretation_changed` instead of being approved
unseen. Approving a rule still compiling answers `autodelete.compiling`; a refused one,
`autodelete.refused`; one already approved, `autodelete.already_approved`.
Authorizing an unapproved rule answers `autodelete.not_approved`. The
inactivity is 1 to 36500 days, the Utility schema's own bound; anything
else answers `autodelete.inactive_days_out_of_range`. The prompt is 1 to
4096 UTF-8 bytes and answers `utility.input_limit` otherwise. The rule
carries its Utility job's identity, so the Provider, binding, Model, and
policy that compiled it are readable through the `utility` Query.

Each Command is a Guarded Security decision recorded against the rule as
`autodelete.rule_compiled`, `autodelete.rule_edited`,
`autodelete.rule_approved`, `autodelete.everywhere_authorized`, or
`autodelete.rule_deleted`. Approval is elevated risk; authorizing native
deletion is destructive.

## What a rule sees

The `autodelete_rules` Query lists every rule with its state and, for one
that has an interpretation, up to 32 candidate matches: Conversations not
in Jet Trash, with no Run still running, whose creation or latest Run's end
is at least that many days ago. Each candidate lists what protects it
today, in the retention preview's order. An empty list means the sweep
would stage it. A Workspace Git cannot inspect is left out, as the sweep
leaves it alone. A pin in a Conversation layout will protect a candidate
too when layouts arrive; nothing can be pinned yet.

## The sweep

The daemon's maintenance loop runs the Autodelete sweep on every wakeup,
after the retention sweep, and its deadline includes the moment the next
Conversation not yet idle long enough reaches an approved rule's
inactivity. A Conversation already past the cutoff that the sweep
declines to stage sets no deadline. The sweep
does nothing while the Plane is in Recovery mode or its Security audit is
degraded. For each approved rule it pages through the idle Conversations
and stages each one that nothing protects, through the same two-read
staging the retention policy uses; inside the staging transaction it reads
the rule and the Conversation's idleness again, so a rule edited or
deleted mid-sweep, or a Run that came and went, is respected. The reason recorded is
`autodelete_rule`, or `autodelete_everywhere` for a rule authorized to
delete everywhere, and the grace period is the Plane's
`retention.trash_grace_days`, 30 days unless changed. The audit records
`conversation.forgotten` or `conversation.deletion_authorized` under the
`retention` actor. Expiry, restoring, and the Deletion ledger are the
retention sweep's, unchanged; native deletion has no Craft to go to in this
release and is recorded for one that can.

## Wire surface

Minor 40 adds the five Commands, the results `autodelete_rule_recorded`
and `autodelete_rule_deleted`, the `autodelete_rules` Query, and the two
Trash reasons. Older peers cannot admit the Commands or the Query, and a
Trash page or retention preview that carries one of the new reasons
answers them `retention.reason_incompatible` rather than a reason they
would misread.

## Tests

`just test -p jet-store autodelete` covers the rows and the inactivity
query; `just test -p jet-core autodelete` covers the state machine, the
fail-closed compilation, and the sweep; `just test -p jet-daemon --test
autodelete` drives a real daemon through a refused compilation, a hand
edit, approval, authorization, and removal.
