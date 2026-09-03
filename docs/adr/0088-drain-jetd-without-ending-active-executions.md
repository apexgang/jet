# Drain jetd without ending active executions

During shutdown or update, `jetd` stops accepting mutating Commands, completes or rolls back its current transaction, records stream cursors and Effect state, and exits within ten seconds. Active Harness Runs and Workspace terminals remain alive under their version-matched `jetfueld` helpers, while incomplete Effects stay pending for reconciliation after restart.
