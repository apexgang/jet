# Acknowledge control state only after durable commit

`jetd` issues a Durable acknowledgement only after the authoritative transaction for Commands, approvals, lifecycle changes, deletion, and Effects has committed with full SQLite durability. Replayable high-volume Harness output may commit in batches capped by sixty-four Events, 256 KiB, or fifty milliseconds because its Run-role `jetfueld` retains the native replay source. Jet advances the acknowledged helper source offset and Plane Event cursor only after commit, so a reconnect either observes the batch or safely requests it again.
