# Use narrow internal platform seams

`jet-runtime` provides small internal Interfaces only where macOS, Linux, and test behavior genuinely varies: process and PTY hosting, credential storage, power monitoring, and sound playback. Each has real platform and test Adapters. Portable Git and filesystem behavior remains concrete rather than being hidden behind broad mock-oriented traits, and no giant platform Interface is exposed to callers.
