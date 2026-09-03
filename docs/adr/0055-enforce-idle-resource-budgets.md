# Enforce idle resource budgets

On fixed macOS and Linux reference machines, an idle `jetd` with ten thousand Conversations may use at most thirty-five MiB of resident memory, a `jetfueld` helper at most eight MiB excluding its child, and an idle Jet Craft at most fifteen MiB. The whole idle core must average below 0.2 percent CPU over five minutes. Core modules use filesystem events and scheduled deadlines instead of periodic repository scans, and resource measurements accompany the existing latency and throughput benchmarks.
