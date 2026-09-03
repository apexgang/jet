# Record Change checkpoints at turn boundaries

At every turn boundary, `jetd` records a lightweight immutable Change checkpoint containing the before-and-after commit identities, the uncommitted patch, changed-file metadata, and content-addressed artifact references. Checkpoints are created whether or not the Harness commits or rewrites Git history, allowing the GUI to present current, per-turn, final, and historical diffs without manufacturing Git commits. In No-Visa mode, checkpoints retain their Plane and Workspace identities so multi-Plane changes remain separately attributable.
