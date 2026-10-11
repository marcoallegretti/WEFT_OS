"""Lay the vendored Servo crates out in their repository tree.

Nix vendors each git crate into a directory of its own, but Servo's crates
read files elsewhere in their repository by relative path: the WebIDL parser
under `third_party/`, the pages and lists under `resources/`. This copies
those parts of the pinned Servo tree to TREE, moves each vendored Servo
crate (with the manifest and checksum file the vendoring wrote) to its place
in that tree, and leaves a symlink in the vendor directory, so cargo sees
the same crates and their relative paths resolve.

Usage: servo-tree.py SERVO_SOURCE CARGO_LOCK VENDOR_DIR TREE
"""

import shutil
import sys
import tomllib
from pathlib import Path

SERVO_GIT = "git+https://github.com/marcoallegretti/servo"
# The parts of the tree the crates read; `tests/` alone is most of its size.
PARTS = ("components", "resources", "third_party", "Cargo.toml")


def main() -> None:
    source, lock, vendor, tree = (Path(a) for a in sys.argv[1:])
    crates = {
        p["name"]: p["version"]
        for p in tomllib.loads(lock.read_text())["package"]
        if p.get("source", "").startswith(SERVO_GIT)
    }
    for part in PARTS:
        src = source / part
        if src.is_dir():
            shutil.copytree(src, tree / part, symlinks=True)
        else:
            shutil.copy(src, tree / part)
    tree.chmod(0o755)
    for path in tree.rglob("*"):
        if not path.is_symlink():
            path.chmod(0o755 if path.is_dir() else 0o644)

    homes = {}
    for manifest in (tree / "components").rglob("Cargo.toml"):
        name = tomllib.loads(manifest.read_text()).get("package", {}).get("name")
        if name in crates:
            homes[name] = manifest.parent
    missing = crates.keys() - homes.keys()
    if missing:
        sys.exit(f"Servo crates not found in {source}: {sorted(missing)}")

    for name, version in crates.items():
        vendored = vendor / f"{name}-{version}"
        shutil.copytree(vendored, homes[name], dirs_exist_ok=True)
        shutil.rmtree(vendored)
        vendored.symlink_to(homes[name])


main()
