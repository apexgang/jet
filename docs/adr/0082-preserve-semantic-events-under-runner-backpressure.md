# Preserve semantic events under runner backpressure

Jet never silently drops semantic control events. Backpressure propagates through bounded channels; only raw terminal bytes and redundant progress updates may be truncated or coalesced, and Jet emits an explicit event describing the affected count or byte range so clients can render the loss honestly.
