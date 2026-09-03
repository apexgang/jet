# Keep database migrations rollback-compatible

Database migrations are forward, transactional, and preceded by a verified Recovery snapshot. Schema evolution follows expand-then-contract so the current and previous Jet releases can open the store; destructive removal waits until the rollback window has passed. A failed migration leaves the prior database untouched and the new `jetd` refuses Commands rather than running against partially migrated state.
