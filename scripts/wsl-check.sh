#!/usr/bin/env bash
set -e

PROJECT="/mnt/c/Users/might/Desktop/Development/Systems/WEFT OS"

# ── Fake libdisplay-info.pc that reports 0.2.9 ───────────────────────────────
# libdisplay-info-sys 0.2.2 requires >= 0.1.0 < 0.3.0; openSUSE ships 0.3.0.
# cargo check does not link, so reporting 0.2.9 satisfies the version guard
# without requiring a source build.
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
echo "==> libdisplay-info shim: $(pkg-config --modversion libdisplay-info --with-path "$FAKE_PC_DIR" 2>/dev/null || echo written)"

# ── cargo check ───────────────────────────────────────────────────────────────
source "$HOME/.cargo/env"

export PKG_CONFIG_PATH="$FAKE_PC_DIR:/usr/lib64/pkgconfig:/usr/share/pkgconfig"

cd "$PROJECT"

echo ""
echo "==> cargo check -p weft-compositor"
cargo check -p weft-compositor 2>&1
echo ""
echo "==> cargo clippy -p weft-compositor -- -D warnings"
cargo clippy -p weft-compositor -- -D warnings 2>&1
echo ""
echo "==> cargo fmt --check -p weft-compositor"
cargo fmt --check -p weft-compositor 2>&1
echo ""
echo "ALL DONE"
