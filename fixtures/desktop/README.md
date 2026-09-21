# Desktop presentation fixtures

`presentation-v1.json` is the shared, platform-neutral fixture corpus for the
macOS and Linux desktop clients. It covers the required shell states before
either client has a live transport.

The corpus is presentation data only. It cannot authorize a Command, stand in
for a Plane snapshot, or imply that an unsupported action succeeded. Each
scenario names the Query, Command, stable error, restart metadata, or backend
dependency that produces it.

The Swift app decodes and validates the corpus through
`DesktopFixtureCorpus`. The decoder rejects unknown format versions, missing
state coverage, duplicate identifiers, non-canonical cursors, impossible Run
state combinations, and queues over the protocol limit.

Do not put credentials, prompts from real users, terminal output, private paths,
or production identifiers in these fixtures.
