# Enforce per-architecture core size budgets

Stripped release executables must remain at or below twelve MiB for `jetd`, three MiB for `jetfueld`, and six MiB for each bundled Jet Craft, with the complete Rust executable payload capped at thirty MiB per architecture. Universal macOS artifacts are evaluated per architecture slice. Release checks fail when a limit is exceeded or when an increase above five percent lacks an explicit reviewed justification; debug symbols and GUI assets are measured separately.
