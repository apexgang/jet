# Diagnostic log

Issue #56 implements ADR-0061 for `jetd`. Each executable role keeps its
own Diagnostic log under `~/.jet/diagnostics`, owner-only, as one JSON
record per line: `jetd.log` is live and `jetd.log.1` through `jetd.log.4`
are the rotated generations. A file rotates at 5 MiB and the fifth
generation is dropped, so one role never holds more than 25 MiB. The log
is separate from the Event journal and from the Security audit
(ADR-0105); nothing in it feeds either, a failed diagnostics write never
refuses a Command, and a daemon that cannot open its log serves without
one and says so once on standard error.

Nothing is sent anywhere. There is no analytics, telemetry, automatic
upload, crash reporting, or audit forwarding; the files stay under the Jet
home until the user copies them.

## What a record may hold

A record is a `level`, a `component`, a static `message`, and typed
`fields`. Messages are string literals in the executable, so a record
cannot be assembled from a request, a prompt, or an Event. Fields are
limited to counts, identifiers, stable error codes such as
`store.unavailable`, and one `failure` field: what an error said about
itself. Failure text is the only prose the log accepts, and it is scrubbed
before it is kept:

- Line breaks and control characters collapse to spaces, and the text is
  cut at 256 bytes with a `[truncated]` marker. A large native payload,
  terminal transcript, or file body never reaches the file whole.
- Everything from the first `{` or `[` on is replaced with
  `[payload redacted]`, because raw native envelopes are what carry
  prompts and outputs.
- Spans of 32 bytes or more inside double quotes or backticks become
  `"[redacted]"`. Apostrophes are left alone because prose is full of
  them.
- A URL keeps only its scheme, host, and path. User information, query
  and fragment are dropped, which is where an authentication callback
  carries its `code` and `state`, and a path segment that looks like key
  material is dropped too.
- A value after `token`, `secret`, `password`, `key`, `auth`, `cookie`,
  `credential`, `session`, `signature`, or `otp` is replaced, whether it
  follows `=` or `:` or stands as the next word, and so is the word after
  `Bearer`, `Basic`, or `Token`.
- Any word of 20 or more base64, hex, URL-safe, or dotted characters
  mixing letters and digits is replaced, except UUIDs, which identify
  things. A path with a digit in it is lost the same way.

Identifiers and codes pass through a stricter filter that keeps only
letters, digits, `.`, `_`, and `-`.

The scrub is structural, so short plain prose passes: a short unquoted
sentence of a prompt would survive inside the 256 bytes. What keeps such
prose out is the other half of the design: `jetd` feeds the failure field
only Core errors, whose text is a stable code and a constant message
(ADR-0068), and never a Harness event, a terminal chunk, or a file. The
purely numeric secret, such as a pairing code, is not recognised either
unless it follows one of the labels above; none is ever passed.

## What stays bounded under load

Two budgets sit in front of the file ring. A token bucket admits 200
records at once and 20 per second sustained; what does not fit is counted
and reported as one `records suppressed by budget` record once the bucket
admits again. A record identical to the one before it, by level,
component, message, and code, is collapsed for up to a minute: the first
is written, the repeats are counted, and one `earlier record repeated`
record reports the count when something else is written. A failure that
recurs every second therefore costs one line a minute.

## Levels and debug activation

`error` marks a failure the daemon could not carry on from, `warn` one it
recovered from or will retry, and `info` an expected event worth a line:
start, stop, snapshots, and what retention forgot. `debug` records are
dropped unless the daemon was started with `jetd serve --debug-log`; that
flag applies to that process alone and is never remembered.

Refused Commands and failed remote authentication or pairing are always
recorded at `warn` with their stable code and nothing of the request. Any
other Command failure, a Usage query included, is a `debug` record.

## What `jetd` records today

Process start and stop, read-only Recovery mode, rejected hellos and
connections from other users, refused Commands and authentication,
worker failures across Craft reconciliation and installation, extension
changes, schedules, terminals, executions, Workspace promotions,
retention, Autodelete, Recovery snapshots, Utility work, and Git delivery. `jetfueld` and the
Craft executables do not write a Diagnostic log yet: their configuration
carries no Jet home, and adding one is a wire-contract change.
