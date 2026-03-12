#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RESULT="${1:-"${REPO_ROOT}/infra/vm/weft-vm-result"}"

if [ ! -e "$RESULT" ]; then
    echo "error: $RESULT not found; run infra/vm/build.sh first" >&2
    exit 1
fi

VM_SCRIPT="${RESULT}/bin/run-weft-vm-vm"
if [ ! -x "$VM_SCRIPT" ]; then
    echo "error: VM script not found at $VM_SCRIPT" >&2
    exit 1
fi

exec "$VM_SCRIPT" "$@"
