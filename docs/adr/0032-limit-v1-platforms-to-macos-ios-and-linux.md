# Limit v1 platforms to macOS, iOS, and Linux

V1 ships a native Swift GUI with bundled `jetd` on macOS, a remote-only native Swift GUI on iOS, and a Tauri GUI with bundled `jetd` on Linux. Windows packaging, daemon supervision, protocol transport validation, GUI release work, and performance qualification are deferred beyond v1. Portable core code should avoid unnecessary platform coupling, but Windows compatibility is not a release requirement and must not slow the first release.
