# Retain Command deduplication results for thirty days

For thirty days, `jetd` retains each Actor's Command identity, request digest, authoritative result, and any referenced Effect identity. An identical retry returns the original result, while reuse with different content is a conflict. An identity older than the retention window is rejected as expired rather than executed again, preventing delayed retries from duplicating external work.
