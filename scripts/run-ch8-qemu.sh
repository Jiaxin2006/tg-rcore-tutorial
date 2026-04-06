#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CH8_DIR="$ROOT/tg-rcore-tutorial-ch8"
SMP_COUNT="2"
DISPLAY_BACKEND="${TG_QEMU_DISPLAY:-cocoa}"
MEM_SIZE="${TG_QEMU_MEM:-128M}"
KERNEL_PATH=""
FS_IMG_PATH=""

usage() {
    cat <<'EOF'
usage: bash scripts/run-ch8-qemu.sh [--smp N] [--display BACKEND] [--kernel PATH] [--fs PATH]

Examples:
  bash scripts/run-ch8-qemu.sh --smp 1
  bash scripts/run-ch8-qemu.sh --smp 2 --display sdl
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --smp)
            SMP_COUNT="$2"
            shift 2
            ;;
        --display)
            DISPLAY_BACKEND="$2"
            shift 2
            ;;
        --kernel)
            KERNEL_PATH="$2"
            shift 2
            ;;
        --fs)
            FS_IMG_PATH="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            usage
            exit 1
            ;;
    esac
done

KERNEL_PATH="${KERNEL_PATH:-$CH8_DIR/target/riscv64gc-unknown-none-elf/debug/jiaxin2006-tg-rcore-tutorial-t4}"
FS_IMG_PATH="${FS_IMG_PATH:-$CH8_DIR/target/riscv64gc-unknown-none-elf/debug/fs.img}"

exec qemu-system-riscv64 \
    -machine virt \
    -serial mon:stdio \
    -display "$DISPLAY_BACKEND" \
    -bios none \
    -smp "$SMP_COUNT" \
    -m "$MEM_SIZE" \
    -drive "file=$FS_IMG_PATH,if=none,format=raw,id=x0" \
    -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
    -device virtio-gpu-device,bus=virtio-mmio-bus.1 \
    -kernel "$KERNEL_PATH"
