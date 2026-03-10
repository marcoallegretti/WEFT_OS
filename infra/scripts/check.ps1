$ErrorActionPreference = 'Stop'

cargo fmt --all --check
cargo clippy --workspace --exclude weft-compositor --all-targets -- -D warnings
cargo test --workspace --exclude weft-compositor
