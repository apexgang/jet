# Serialize atomic client commands

Every connected GUI client may submit work without acquiring an ownership lock, but messages and commands are atomic rather than character-interleaved. `jetd` assigns their authoritative order, records the originating client, and deduplicates retried commands so simultaneous multi-device control remains deterministic.
