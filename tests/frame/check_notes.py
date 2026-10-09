"""Check that the Notes demo stores exactly what the user types.

The harness runs weft-appd on a nested desktop with the real runtime and app
shell, seeds the Notes data with text containing a literal backslash
sequence, quotes, a tab and non-ASCII characters, launches
`org.weft.demo.notes` and edits it with real keyboard input:

1. Typing at the end and pressing Ctrl+S must store the seeded text plus
   the typed text, byte for byte; a load or save that rewrote `\\n`, quotes
   or tabs fails.
2. Text typed while a save is in flight (the runtime is paused with SIGSTOP
   between Ctrl+S and the reply) must still be in the editor afterwards: a
   second Ctrl+S stores it too.
3. After the file is changed on disk (as by another Notes window), a save
   must be refused and the other window's text kept.
4. Discard reloads the stored text. Keys typed while that reload is in
   flight (runtime paused again) must be ignored, so a following save stores
   the reloaded text plus only what was typed afterwards.

Requires Xvfb, xwd, ImageMagick's convert and xdotool.
"""

import argparse
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.dont_write_bytecode = True
from check_counter import AppdClient, free_port, read_endpoint  # noqa: E402
from session import ROOT, Desktop, SessionError, missing_tools  # noqa: E402

APP_ID = "org.weft.demo.notes"
SEED = 'path C:\\new\\table "quoted"\tcaf\u00e9 \u2603'


# The Discard button in the 800x600 Notes window at the desktop's origin.
DISCARD = ("663", "68")


def runtime_pid():
    """The process ID of the Notes session's weft-runtime."""
    found = subprocess.run(["pgrep", "-n", "-f", f"weft-runtime {APP_ID}"],
                           capture_output=True, text=True)
    if found.returncode != 0:
        raise AssertionError("no weft-runtime process for Notes")
    return int(found.stdout.split()[0])


def wait_for_file(path, expected, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists() and path.read_text(encoding="utf-8") == expected:
            return
        time.sleep(0.2)
    actual = path.read_text(encoding="utf-8") if path.exists() else None
    raise AssertionError(f"stored notes {actual!r}, expected {expected!r}")


def run(args, desktop, home, xdotool):
    data_home = home / "share"
    notes = data_home / "weft/app-data" / APP_ID / "notes.txt"
    notes.parent.mkdir(parents=True)
    notes.write_text(SEED, encoding="utf-8")

    t = Path(args.target)
    desktop.launch_client("appd", [t / "weft-appd"], {
        "WEFT_RUNTIME_BIN": str(t / "weft-runtime"),
        "WEFT_APP_SHELL_BIN": str(t / "weft-app-shell"),
        "WEFT_APP_STORE": str(args.store),
        "WEFT_DISABLE_CGROUP": "1",
        "WEFT_APPD_WS_PORT": str(free_port()),
        "HOME": str(home),
        "XDG_DATA_HOME": str(data_home),
    })
    port, token = read_endpoint(desktop.runtime, 30)
    appd = AppdClient(port, token)
    appd.send({"type": "LAUNCH_APP", "app_id": APP_ID, "surface_id": 0})
    reply = appd.wait_for(lambda m: m.get("type") in ("APP_READY", "ERROR"), 120)
    if reply.get("type") != "APP_READY":
        raise AssertionError(f"Notes did not start: {reply}")
    time.sleep(2)

    def act(*arguments):
        subprocess.run([xdotool, *arguments], env={"DISPLAY": desktop.display},
                       check=True, timeout=30)

    act("search", "--class", "weft-compositor", "windowfocus", "--sync")
    # The editor fills the window below the header.
    act("mousemove", "--sync", "100", "300")
    act("click", "1")
    act("key", "ctrl+End")

    runtime = runtime_pid()

    act("type", "--delay", "40", ' +\\t "x"')
    expected = SEED + ' +\\t "x"'
    os.kill(runtime, signal.SIGSTOP)
    try:
        act("key", "ctrl+s")
        act("type", "--delay", "40", " more")
        time.sleep(2)
    finally:
        os.kill(runtime, signal.SIGCONT)
    wait_for_file(notes, expected)
    time.sleep(1)
    act("key", "ctrl+s")
    expected += " more"
    wait_for_file(notes, expected)

    other = "written by another window"
    notes.write_text(other, encoding="utf-8")
    act("type", "--delay", "40", " lost?")
    act("key", "ctrl+s")
    time.sleep(3)
    if notes.read_text(encoding="utf-8") != other:
        raise AssertionError("a save replaced notes changed elsewhere")

    os.kill(runtime, signal.SIGSTOP)
    try:
        act("mousemove", "--sync", *DISCARD)
        act("click", "1")
        act("mousemove", "--sync", "100", "300")
        act("click", "1")
        act("type", "--delay", "40", "zz")
        # Let the page handle the keys while the reload is still unanswered.
        time.sleep(2)
    finally:
        os.kill(runtime, signal.SIGCONT)
    time.sleep(2)
    act("key", "ctrl+End")
    act("type", "--delay", "40", "!")
    act("key", "ctrl+s")
    wait_for_file(notes, other + "!")
    leftovers = [p.name for p in notes.parent.iterdir() if p.name != "notes.txt"]
    if leftovers:
        raise AssertionError(f"files left beside the notes: {leftovers}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/debug/weft-compositor")
    parser.add_argument("--target", type=Path, default=ROOT / "target/debug",
                        help="directory with weft-appd, weft-runtime and weft-app-shell")
    parser.add_argument("--store", type=Path, default=ROOT / "examples",
                        help="app store containing org.weft.demo.notes")
    parser.add_argument("--output", type=Path, default=ROOT / "target/notes-check")
    args = parser.parse_args(argv)

    xdotool = shutil.which("xdotool")
    missing = missing_tools() + ([] if xdotool else ["xdotool"])
    missing += [str(p) for p in (args.compositor, args.target / "weft-appd",
                                 args.target / "weft-runtime", args.target / "weft-app-shell")
                if not p.is_file()]
    if missing:
        print("missing: " + ", ".join(missing), file=sys.stderr)
        return 2
    home = Path(tempfile.mkdtemp(prefix="weft-home-"))
    try:
        with Desktop(args.compositor, args.output, outputs=["appd.log", "notes.ppm"]) as desktop:
            try:
                run(args, desktop, home, xdotool)
            except AssertionError:
                (args.output / "notes.ppm").write_bytes(desktop.capture())
                raise
    except (AssertionError, SessionError, TimeoutError) as failure:
        print(f"notes check failed: {failure}", file=sys.stderr)
        print(f"logs and capture in {args.output}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(home, ignore_errors=True)
    print("notes check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
