# Set performance budgets for the core store

On the release reference NVMe desktop, `jetd` must become ready within 150 ms with 10,000 Conversations and one million journal entries; representative 64-event or 256-KiB commits must complete within 10 ms at p99, sidebar and 500-block queries within 10 ms and 15 ms at p95, 10,000-event reconnect pages within 100 ms, and batched ingestion must sustain 10,000 small events per second. Rust throughput and integration benchmarks run across macOS and Linux, flagging regressions above 15 percent.
