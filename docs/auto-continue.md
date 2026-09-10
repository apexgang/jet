# Auto-continue

Auto-continue admits bounded retry input after both a measured, exhausted Provider quota window and structured `waiting_for_quota` activity are committed for a Run (ADR-0033). Text output cannot authorize a retry.

Jet 1.34 exposes `set_auto_continue` and the `auto_continue` query through the authenticated client API. A target is an Account binding or Conversation. Policies are `off`, or `retry` with `delay_ms`, `max_delay_ms`, `max_retries`, and an exact `message`. Delay bounds are 1–86,400,000 ms, retry bounds are 1–100, and messages contain 1–8192 bytes of nonblank text.

A Conversation policy is a one-shot override for one exhaustion episode. Selection falls back to the Home Plane's Account-binding default, then off. The query returns the configured policy; for a Conversation, this is the unconsumed override or off. Its retry record separately retains the selected policy and scope, triggering Usage condition, observed time, due time, count, and outcome. Changes also appear in the durable Event journal.

Provider reset times take precedence over fallback delays. Without a reset time, delays double after each attempted retry up to `max_delay_ms`. Exhausted windows are considered independently; a later blocking reset postpones the existing pending input without increasing its count. Successful work followed by a new scheduled input begins a new episode.

Only one Auto-continue entry is pending at a time. Replacement and disabling leave user and Scheduled-task entries intact. New user input cancels Auto-continue while retaining the quota wait. A full queue defers admission without advancing the retry count; later worker ticks reconsider it. Admitted retries survive daemon restart and retain their due time.

Delivery retains the Account binding, Provider, native Conversation, Craft, Harness, Model, and Plane. An existing connection must still match the admitted selection. A new Run requires native resume and an explicit Model observation under Craft 1.10. If the accepted Craft cannot enforce that selection, the input stays pending. The bundled Claude Craft supports this resume; the Codex Craft currently supports retries only on its matching live connection. Automatic Handoff is not enabled by this policy.

Policy changes enter the content-free Security audit and are refused in Security-degraded mode. Queues with quota timing or pinned retry input use a versioned storage envelope. Older backends refuse that queue rather than dispatching input without its safeguards; upgrading restores access to the unchanged queue.
