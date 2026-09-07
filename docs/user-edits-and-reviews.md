# User edits and structured reviews

Issue #26 implements ADR-0041 through Jet protocol minor 19. Direct file
changes are authenticated Commands, never Harness observations, and submitted
inline comments enter the existing Turn queue as one user Turn.

## Direct edits

An editable file is addressed by a `FileTarget`—either a registered Project or
a Workspace—and a validated path relative to that root. Absolute paths, parent
traversal, platform-dependent separators, control characters, and symbolic
links that leave the registered root are refused before file access. Editable
paths that pass through an in-root symbolic link are also refused so an atomic
replacement cannot silently change which path receives the write.

The `editable_file` Query returns at most 128 KiB of UTF-8 content and an exact
`FileRevision`: the Git blob object and regular-file mode. A missing file has
no content, mode `000000`, and an all-zero object of the repository's object
length. Direct editing does not create parent directories and does not expose
binary or oversized files through this text boundary.

`apply_user_edit` carries the complete replacement content and the Revision the
editor read. Jet revalidates the registered root and Revision immediately before
an atomic same-directory replacement. A write-ahead intent commits before the
replacement; if Jet stops before the Actor Event, evidence, and Command receipt
commit, daemon startup reconciles the observed before/after state and finishes
that same Command. If recovery finds the requested bytes already present, it
finishes the Event and receipt but conservatively omits change evidence because
the durable state alone cannot prove who wrote those bytes. Existing permissions
are preserved; a new file starts non-executable. A stale Revision returns
`user_edit.stale_revision` and a structured `refresh_file` recovery action with
the authoritative Revision. Exact Command retries return the original durable
result after restart.

Each actual change writes a `user_edit.applied` Event attributed to the
authenticated interactive client, with no Harness origin. If a managed Run is
tracking that Project or Workspace, the same Command records exact before/after
change evidence with `user_edit` origin. Evidence written while a turn is active
belongs to that turn; evidence written between turns belongs to the gap before
the next checkpoint, or to the terminal snapshot when the Run ends first. Current,
aggregate, and Final diffs can therefore attribute the complete file transition
to that client; gaps or later external overwrites retain the existing
external-or-unknown behavior.

## Review submissions

`submit_review` accepts one to 128 ordered comments. Every comment preserves a
validated relative file path, a one-based line number, and 1–8192 bytes of UTF-8
text. The whole rendered prompt must fit the existing 65,536-byte Turn limit.
Its structured Event encoding must fit the journal's 65,536-byte bound as well.
The GUI's draft remains local until this Command succeeds.

The batch receives one Turn identity, sequence, authenticated client, and `user`
source. It follows the same admission order, queue limits, execution, withdrawal,
receipt, and restart rules as other user input. The queue persists the structured
batch, `review.submitted` preserves it in the Event journal, and `turn.input`
preserves the coherent text delivered to the Harness.

Focused conformance tests are `just test -p jet-daemon --test user_input` and
`just test -p jet-core checkpoint_tests::a_direct_edit_records_user_evidence_for_the_active_turn`,
run from `packages/`.
