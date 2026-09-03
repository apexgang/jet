# Disconnect slow clients instead of growing memory

Each GUI connection receives a bounded event window of at most 1,000 events or 2 MiB. A client that falls behind is disconnected with its last cursor and reconnects through normal snapshot and replay. Artifact and terminal byte streams use their own bounded channels and cannot make the semantic event queue grow without limit.
