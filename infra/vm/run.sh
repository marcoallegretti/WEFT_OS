#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE="${1:-"${REPO_ROOT}/infra/vm/weft-vm.qcow2"}"

if [ ! -f "$IMAGE" ]; then
    echo "error: $IMAGE not found; run infra/vm/build.sh first" >&2
    exit 1
fi

MEM="${WEFT_VM_MEM:-4096}"
CPUS="${WEFT_VM_CPUS:-4}"
DISPLAY_OPT="${WEFT_VM_DISPLAY:-gtk,gl=on}"

exec qemu-system-x86_64 \
    -enable-kvm \
    -m "${MEM}M" \
    -smp "${CPUS}" \
    -drive "file=${IMAGE},format=qcow2,if=virtio" \
    -vga virtio \
    -display "${DISPLAY_OPT}" \
    -device virtio-net-pci,netdev=net0 \
    -netdev user,id=net0 \
    -device virtio-rng-pci \
    -serial mon:stdio \
    "$@"
