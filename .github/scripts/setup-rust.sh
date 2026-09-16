#!/usr/bin/env bash
set -euo pipefail

toolchain=$(python3 -c 'import tomllib; print(tomllib.load(open("packages/rust-toolchain.toml", "rb"))["toolchain"]["channel"])')
echo "RUSTUP_TOOLCHAIN=$toolchain" >> "$GITHUB_ENV"
rustup toolchain install "$toolchain" --profile minimal
if [[ "${1:?expected test or release}" == test ]]; then
  cat .github/ci/cargo.toml >> packages/.cargo/config.toml
fi
