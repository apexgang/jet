# Execution control

Issue [#23](https://github.com/apexgang/jet/issues/23) implements ADR-0083 and
ADR-0095 through Jet protocol minor 19, Craft 1.4, and Helper 1.2.

`interrupt_turn` and `stop_run` each take a `run_id`. They are separate
Commands with separate outcomes: interrupting ends the turn the Run is
working on and leaves the Run able to take the next one; stopping ends the
whole execution, including the native processes it owns. Both answer with
`run_control_accepted`, carrying the Run and the request. Acceptance is
durable, not an outcome: what actually happened arrives as Events.

```json
{"type":"interrupt_turn","run_id":"<uuid>"}
{"type":"stop_run","run_id":"<uuid>"}
```

Interrupting requires a Run that is executing a claimed turn; a Run with
nothing running, or one already stopping, is refused with
`run.no_active_turn`, and a Run that has ended with `run.not_controllable`.
Stopping is accepted from any live lifecycle and moves an active Run to
`stopping`, so no further input is claimed while it ends. Repeating a
request drives the same durable decision again and asking to stop a Run
that was only being interrupted replaces the weaker request; how often a
client asks never chooses how hard Jet stops the work.

## How the work actually ends

Interruption prefers the Harness's own cancellation. When the pinned Craft
negotiated Craft 1.4, the host sends `interrupt` naming the identity that
turn was delivered under, and the Craft answers with a `turn_ended`
boundary carrying `interrupted`. The turn settles as `canceled`, the Run
stays `active`, and the queue moves on to the input waiting behind it.

Everything else escalates. jetd asks the Run's own authenticated helper
instance, over Helper 1.2, to deliver one signal to the native process
group: interrupt, then terminate, then kill, waiting for the execution's
own end between steps so a Harness that handles the signal is never
escalated past. Signalling ends no helper and releases no source, so the
native exit still reaches Core through the ordinary spool and the Run's
partial output, Conversation history, and Workspace changes all survive.
A Run that ends under an admitted request ends `canceled`, whatever status
the OS reported.

The `run_execution` Query carries `termination` once a request settles, and
the journal records `run.control_requested` and `run.terminated`. Its
`stage` is exactly the step that ended the work: `native_cancellation`,
`interrupt`, `terminate`, `kill`, or `unobserved` when every signal was
delivered and the end was never seen. The last three are forced
terminations. Nothing is recorded when the execution outlived the request
on its own: no Run that stopped by itself is described as having been
stopped.

An escalation interrupted by a daemon restart is observed, never continued
blindly, because a signal already delivered may have ended work whose
outcome is not yet visible. A later explicit Command asks again.

## Request cancellation is not execution control

A Query accepts an optional `timeout_ms`, a relative bound of 1 to 60000
milliseconds from arrival, refused outside that range with
`request.invalid_timeout` and answered with `request.timed_out` when it
expires. A Query is a non-durable read, so abandoning one changes nothing
on the Plane.

Commands take no bound at all. Once a Command is durably accepted, neither
a transport-level cancellation nor a lost client reverses it: its Effect
still dispatches and its identity still answers with the original receipt.
Withdrawing queued input reaches queued input only, and never the turn a
Run is executing. Changing live work is exactly what Interrupt turn and
Stop Run are for.

## Validation

`just test -p jet-daemon --test execution_control`, run from `packages/`,
drives the real daemon, Craft, helper, and native processes: a natively
cancelled turn whose Run keeps working, a Harness that ignores every signal
it may ignore until it is killed, and the request bounds a client may and
may not set. The escalation case asserts that the output the Harness
produced before the stop, and the file it wrote, are still there afterwards.
