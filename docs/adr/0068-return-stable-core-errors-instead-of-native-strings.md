# Return stable Core errors instead of native strings

Core Interfaces return stable categories such as invalid input, unauthorized, conflict, unavailable, incompatible, rate-limited, not found, Outcome unknown, and internal failure. Each Core error includes a domain code, retryability, a safe human message, and optional structured recovery actions. GUI clients never parse Rust, operating-system, SQLite, Git, SSH, or Harness error strings; redacted native detail remains available only in diagnostics.
