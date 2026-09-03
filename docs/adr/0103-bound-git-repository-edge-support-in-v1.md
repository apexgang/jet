# Bound Git repository edge support in v1

V1 accepts ordinary non-bare Git repositories and existing linked worktrees as Projects. It preserves sparse-checkout configuration, represents submodules only by their Git-link commits without recursively managing dirty submodule state, and treats nested repositories as opaque directories. Git LFS behavior is available only when the external capability is detected and reported; Jet does not bundle it. Bare repositories are rejected because they cannot provide the working tree required by Runs, diffs, and Change checkpoints.
