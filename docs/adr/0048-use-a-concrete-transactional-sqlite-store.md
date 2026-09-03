# Use a concrete transactional SQLite store

`jet-store` exposes one concrete transactional store Interface backed by SQLite and content-addressed artifacts. SQL, migrations, FTS, compaction, snapshots, journal sequencing, and artifact bookkeeping remain inside its Implementation. Tests use real temporary SQLite databases rather than a family of generic per-entity repository traits; another storage Adapter will be introduced only if a second implementation actually exists.
