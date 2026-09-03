# Connect every GUI through jetd

Every GUI, including the local macOS application, uses the same `jetd` client protocol instead of embedding `jet-core` through FFI. The Tauri application reuses `jet-client`; Swift applications use generated wire models with native transport code. This gives local and remote use one orchestration lifecycle, authorization path, reconnect behavior, and observable result.
