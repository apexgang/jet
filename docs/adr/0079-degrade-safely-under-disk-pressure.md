# Degrade safely under disk pressure

Jet reserves the greater of 2 GiB or 5% free space and applies a configurable 5 GiB budget to disposable Artifacts and caches. Under pressure, `jetd` may collect only eligible caches, unreferenced Artifacts, and expired data; it never removes pinned, active, dirty, or unpushed work. Reads, export, and cleanup remain available, while new Runs and large writes are rejected until the reserve recovers.
