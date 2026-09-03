# Render adapters through presentation blocks

Each Jet Craft implements its harness-native protocol directly and retains complete native events, while also emitting platform-neutral presentation blocks and actions understood by every GUI client. This avoids reducing harness behavior to a shared domain model while ensuring a newly installed adapter appears and remains usable in existing Swift and Tauri GUIs without supplying executable UI code.
