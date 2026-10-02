//! Drops the unwinder's lookup table from the released Linux `jetd`.
//!
//! `jetd` aborts on panic (ADR-0059), so nothing unwinds inside the process.
//! `.eh_frame_hdr` and its `PT_GNU_EH_FRAME` segment serve only in-process
//! stack walks, which in `jetd` means `RUST_BACKTRACE` and `std::backtrace`
//! captures; those now end after the panic message. `.eh_frame` stays, so
//! gdb, eu-stack, and systemd-coredump still walk a core dump, with or without
//! the crash-symbol artifact. Debug builds keep the table so backtraces keep
//! their frames during development. macOS links with ld64 and is unaffected.

fn main() {
	println!("cargo:rerun-if-changed=build.rs");
	let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
	let profile = std::env::var("PROFILE").unwrap_or_default();
	if target_os == "linux" && profile == "release" {
		println!("cargo:rustc-link-arg-bin=jetd=-Wl,--no-eh-frame-hdr");
	}
}
