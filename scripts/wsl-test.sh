#!/usr/bin/env bash
set -e

PROJECT="/mnt/c/Users/might/Desktop/Development/Systems/WEFT OS"

FAKE_PC_DIR="$HOME/.local/fake-pkgconfig"
mkdir -p "$FAKE_PC_DIR"
cat > "$FAKE_PC_DIR/libdisplay-info.pc" << 'EOF'
prefix=/usr
exec_prefix=${prefix}
libdir=/usr/lib64
includedir=/usr/include

Name: libdisplay-info
Description: EDID and DisplayID library (version shim for cargo check)
Version: 0.2.9
Libs: -L${libdir} -ldisplay-info
Cflags: -I${includedir}
EOF

source "$HOME/.cargo/env"
export PKG_CONFIG_PATH="$FAKE_PC_DIR:/usr/lib64/pkgconfig:/usr/share/pkgconfig"

cd "$PROJECT"

echo "==> cargo test -p weft-compositor"
cargo test -p weft-compositor 2>&1

echo ""
echo "==> cargo test -p weft-appd"
cargo test -p weft-appd 2>&1

echo ""
echo "==> cargo test -p weft-runtime"
cargo test -p weft-runtime 2>&1

echo ""
echo "==> cargo test -p weft-pack"
cargo test -p weft-pack 2>&1

echo ""
echo "ALL DONE"
