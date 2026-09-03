# Rotate bounded Recovery snapshots

After the first meaningful committed change each day, before every schema migration, and before destructive maintenance, `jetd` creates and verifies a Recovery snapshot. It retains seven daily and four weekly snapshots plus the rollback snapshot required by the current and previous release pair. Disk-pressure cleanup may remove older snapshots but must preserve the newest verified snapshot and every snapshot still required for rollback.
