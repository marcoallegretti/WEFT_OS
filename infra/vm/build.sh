#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${1:-"${REPO_ROOT}/infra/vm/weft-vm-result"}"

if [ -e "$OUT" ]; then
    echo "error: $OUT already exists; remove it before rebuilding" >&2
    exit 1
fi

cd "$REPO_ROOT"

echo "building NixOS VM..."
nix build .#nixosConfigurations.weft-vm.config.system.build.vm \
    --out-link "$OUT" \
    --print-build-logs

echo "VM script: $OUT/bin/run-weft-vm-vm"
