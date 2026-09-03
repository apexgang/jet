# Use one local and remote client protocol

GUI clients communicate with `jetd` through one versioned, bidirectional, sequenced protocol using local IPC on desktop and SSH standard input/output remotely. Desktop distributions bundle `jetd`, while v1 mobile distributions contain only a GUI client; remote and local clients receive the same structured conversations, artifacts, and diffs without requiring a public Jet network listener.
