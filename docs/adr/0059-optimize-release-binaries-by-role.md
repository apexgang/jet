# Optimize release binaries by role

Release profiles optimize `jetd` for balanced speed and size with thin link-time optimization, while `jetfueld` and bundled Crafts prioritize size with full link-time optimization. Shipped executables strip symbols into separate crash-symbol artifacts and abort on panic; library Interfaces return typed errors. Dependency default features are disabled unless explicitly justified, and both performance and size gates must pass without trading one away invisibly.
