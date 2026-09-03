# Defer application-level database encryption

V1 protects Conversation state with owner-only filesystem permissions and relies on FileVault or Linux full-disk encryption for encryption at rest. Credentials remain outside the database in platform credential storage. Jet does not link SQLCipher in v1 because its binary, migration, recovery, and key-loss costs are not justified without an enterprise requirement; application-level store encryption remains a future compatible design concern.
