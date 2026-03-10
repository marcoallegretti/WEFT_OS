#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all --check

if [ "$(uname -s)" = "Linux" ]; then
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
else
  cargo clippy --workspace --exclude weft-compositor --all-targets -- -D warnings
  cargo test --workspace --exclude weft-compositor
fi
