# Separate request cancellation from execution control

Queries and non-durable operations accept relative millisecond timeouts and cooperative request cancellation. Once a Command or Effect is durably accepted, transport cancellation or client disconnection cannot reverse it; changing active work requires an explicit Interrupt turn or Stop Run Command. This prevents an ambiguous lost response from being mistaken for a successful rollback.
