# Own each Conversation on one Home Plane

Every Conversation has exactly one Home Plane whose `jetd` owns its authoritative state. Visa Runs execute on that Plane; No-Visa Runs keep their Harness there and may issue explicit, audited SSH operations against paired destination Planes without creating fleet synchronization. Changing the Visa Plane or No-Visa origin requires an explicit Plane transfer. Jet performs no continuous Conversation replication and never presents wall-clock merging as consensus.
