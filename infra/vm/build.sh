#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${1:-"${REPO_ROOT}/infra/vm/weft-vm.qcow2"}"

if [ -f "$OUT" ]; then
    echo "error: $OUT already exists; remove it before rebuilding" >&2
    exit 1
fi

cd "$REPO_ROOT"

echo "building NixOS VM image..."
nix build .#nixosConfigurations.weft-vm.config.system.build.qcow2 \
    --out-link /tmp/weft-vm-result \
    --print-build-logs

SOURCE="$(readlink -f /tmp/weft-vm-result)"
if [ ! -f "$SOURCE" ]; then
    SOURCE="$(find /tmp/weft-vm-result -name '*.qcow2' | head -1)"
fi

if [ -z "$SOURCE" ]; then
    echo "error: could not locate .qcow2 in build output" >&2
    exit 1
fi

cp "$SOURCE" "$OUT"
rm -f /tmp/weft-vm-result
echo "image: $OUT"
