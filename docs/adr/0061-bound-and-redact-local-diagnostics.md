# Bound and redact local diagnostics

Diagnostic logs are separate from the Event journal and rotate at five files of five MiB per executable role. They exclude credentials, prompt bodies, terminal output, and other Conversation content by default; temporary debug logging requires explicit user activation. Crash reports and symbolized diagnostics remain local until a user exports them, and v1 sends no analytics or remote telemetry.
