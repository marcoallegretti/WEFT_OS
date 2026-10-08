"""Run WEFT OS verification profiles without installing, formatting or publishing anything.

Each profile is a fixed list of argument arrays executed from the repository root.
The runner stops at the first failing command and returns its exit status.
"""

import argparse
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]

# Stop rustup from installing the pinned toolchain or its components on demand.
ENVIRONMENT = {**os.environ, "RUSTUP_AUTO_INSTALL": "0"}

LINUX_CRATES = ["weft-compositor", "weft-servo-shell", "weft-app-shell"]
DEMOS = ["examples/org.weft.demo.counter", "examples/org.weft.demo.notes"]
DEMO_KEY = "examples/keys/weft-sign.pub"
RUNTIME_FEATURES = "wasmtime-runtime,net-fetch"


def _packages(flag, crates):
    return [part for crate in crates for part in (flag, crate)]


PROFILES = {
    "toolkit": (
        "Contributor toolkit links, catalog, metadata and runner tests",
        [
            [sys.executable, "-B", ".agents/scripts/check_toolkit.py"],
            [sys.executable, "-B", "-m", "unittest", "discover", "-s", ".agents/scripts",
             "-p", "test_*.py"],
        ],
    ),
    "portable": (
        "Formatting, Clippy and tests for crates built on every host (CI cross-platform)",
        [
            ["cargo", "fmt", "--all", "--check"],
            ["cargo", "clippy", "--workspace", *_packages("--exclude", LINUX_CRATES),
             "--all-targets", "--locked", "--", "-D", "warnings"],
            ["cargo", "test", "--workspace", *_packages("--exclude", LINUX_CRATES), "--locked"],
        ],
    ),
    "linux": (
        "Clippy and tests for the Wayland compositor and feature-disabled shells (CI linux-only)",
        [
            ["cargo", "clippy", *_packages("-p", LINUX_CRATES), "--all-targets", "--locked",
             "--", "-D", "warnings"],
            ["cargo", "test", *_packages("-p", LINUX_CRATES), "--locked"],
        ],
    ),
    "servo-embed": (
        "Type-check both Servo hosts with the real renderer feature (CI servo-embed-linux)",
        [
            ["cargo", "check", "-p", "weft-servo-shell", "--features", "servo-embed", "--locked"],
            ["cargo", "check", "-p", "weft-app-shell", "--features", "servo-embed", "--locked"],
        ],
    ),
    "runtime": (
        "Clippy and tests for weft-runtime with the Wasmtime engine and fetch enabled",
        [
            ["cargo", "clippy", "-p", "weft-runtime", "--features", RUNTIME_FEATURES,
             "--all-targets", "--locked", "--", "-D", "warnings"],
            ["cargo", "test", "-p", "weft-runtime", "--features", RUNTIME_FEATURES, "--locked"],
        ],
    ),
    "demos": (
        "Check the demo components for wasm32-wasip2 against their own lockfiles",
        [
            ["cargo", "check", "--locked", "--manifest-path", f"{demo}/Cargo.toml",
             "--target", "wasm32-wasip2", "--target-dir", "target/examples"]
            for demo in DEMOS
        ],
    ),
    "packages": (
        "Validate the demo packages and verify their committed signatures with weft-pack",
        [
            command
            for demo in DEMOS
            for command in (
                ["cargo", "run", "--quiet", "--locked", "-p", "weft-pack", "--", "check", demo],
                ["cargo", "run", "--quiet", "--locked", "-p", "weft-pack", "--", "verify", demo,
                 "--key", DEMO_KEY],
            )
        ],
    ),
}


def describe():
    width = max(map(len, PROFILES))
    lines = [f"  {name:<{width}}  {summary}" for name, (summary, _) in PROFILES.items()]
    return "profiles:\n" + "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog=describe(),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("profiles", nargs="+", choices=PROFILES, metavar="profile",
                        help="one or more profiles, run in the given order")
    parser.add_argument("--dry-run", action="store_true",
                        help="print the commands without running them")
    args = parser.parse_args(argv)

    for profile in dict.fromkeys(args.profiles):
        for command in PROFILES[profile][1]:
            print(f"[{profile}] {subprocess.list2cmdline(command)}", flush=True)
            if args.dry_run:
                continue
            try:
                result = subprocess.run(command, cwd=ROOT, env=ENVIRONMENT, check=False)
            except OSError as error:
                print(f"[{profile}] cannot run {command[0]}: {error}", file=sys.stderr)
                return 1
            if result.returncode != 0:
                print(f"[{profile}] failed with exit status {result.returncode}", file=sys.stderr)
                return result.returncode if result.returncode > 0 else 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
