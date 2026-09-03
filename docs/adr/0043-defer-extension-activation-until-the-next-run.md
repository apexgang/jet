# Defer extension activation until the next Run

Installing, updating, disabling, or removing a Harness extension takes effect for subsequent Runs by default and never silently mutates an active Run. A Jet Craft update similarly stages a new artifact digest for new Runs while active Runs remain pinned to their current digest. The GUI may offer Reload now for a Harness extension only when the responsible Craft reports native safe-reload support, and it must disclose context invalidation, prompt-cache cost, restart requirements, or partial activation before the user confirms.
