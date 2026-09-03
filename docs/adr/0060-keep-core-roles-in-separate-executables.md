# Keep core roles in separate executables

`jetd`, `jetfueld`, and each bundled Craft remain separate executables instead of one multicall binary. Some compiled runtime code may be duplicated on disk, but the execution-preservation helper does not map database, GitHub, discovery, or Harness-adapter code, keeping its working set, permissions, restart behavior, and attack surface narrow.
