"""Check that existing Notes data survives the move to the app data store.

Earlier versions kept app data inside the user package store, at
`~/.local/share/weft/apps/<id>/data`. The harness places a notes file there
(with a backslash sequence, a newline and non-ASCII text, which must come
through byte for byte), runs weft-appd on a nested desktop and launches
`org.weft.demo.notes` with the real runtime and app shell. The session must
reach APP_READY, the file must then be at
`$XDG_DATA_HOME/weft/app-data/org.weft.demo.notes/notes.txt` with identical
bytes, the old directory and the emptied package directory must be gone,
and the new one private (0700). Typing at the end of the note and pressing
Ctrl+S must then store the moved text plus the typed text, which shows that
the session reads and writes the moved directory.

A second launch after placing a different file at the old location must be
refused with error 409, leaving both copies untouched.

Requires Xvfb, xwd, ImageMagick's convert and xdotool.
"""

import argparse
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.dont_write_bytecode = True
from check_counter import AppdClient, free_port, read_endpoint  # noqa: E402
from check_notes import wait_for_file  # noqa: E402
from session import ROOT, Desktop, SessionError, missing_tools  # noqa: E402

APP_ID = "org.weft.demo.notes"
NOTES = "first line\nliteral \\n stays\ncafé ☃\n".encode()


def run(args, desktop, home, xdotool):
    legacy = home / ".local/share/weft/apps" / APP_ID / "data"
    legacy.mkdir(parents=True)
    (legacy / "notes.txt").write_bytes(NOTES)
    data_home = home / "data-home"
    target = data_home / "weft/app-data" / APP_ID

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
    if not (target / "notes.txt").is_file() or (target / "notes.txt").read_bytes() != NOTES:
        raise AssertionError("notes did not arrive unchanged")
    if legacy.parent.exists():
        raise AssertionError(f"{legacy.parent} still exists")
    if target.stat().st_mode & 0o777 != 0o700:
        raise AssertionError(f"{target} is not private: {oct(target.stat().st_mode)}")

    def act(*arguments):
        subprocess.run([xdotool, *arguments], env={"DISPLAY": desktop.display},
                       check=True, timeout=30)

    time.sleep(2)
    act("search", "--class", "weft-compositor", "windowfocus", "--sync")
    act("mousemove", "--sync", "100", "300")
    act("click", "1")
    act("key", "ctrl+End")
    act("type", "--delay", "40", "ok")
    act("key", "ctrl+s")
    wait_for_file(target / "notes.txt", NOTES.decode() + "ok")

    appd.send({"type": "TERMINATE_APP", "session_id": reply["session_id"]})
    legacy.mkdir(parents=True)
    (legacy / "notes.txt").write_bytes(b"other copy")
    appd.send({"type": "LAUNCH_APP", "app_id": APP_ID, "surface_id": 0})
    reply = appd.wait_for(lambda m: m.get("type") in ("APP_READY", "ERROR"), 60)
    if reply.get("type") != "ERROR" or reply.get("code") != 409:
        raise AssertionError(f"conflicting data was not refused: {reply}")
    if (legacy / "notes.txt").read_bytes() != b"other copy" or \
            (target / "notes.txt").read_bytes() != NOTES + b"ok":
        raise AssertionError("a refused launch changed app data")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/debug/weft-compositor")
    parser.add_argument("--target", type=Path, default=ROOT / "target/debug",
                        help="directory with weft-appd, weft-runtime and weft-app-shell")
    parser.add_argument("--store", type=Path, default=ROOT / "examples",
                        help="app store containing org.weft.demo.notes")
    parser.add_argument("--output", type=Path, default=ROOT / "target/app-data-check")
    args = parser.parse_args(argv)

    xdotool = shutil.which("xdotool")
    missing = missing_tools() + ([] if xdotool else ["xdotool"])
    missing += [str(p) for p in (args.compositor, args.target / "weft-appd",
                                                  args.target / "weft-runtime",
                                                  args.target / "weft-app-shell")
                                 if not p.is_file()]
    if missing:
        print("missing: " + ", ".join(missing), file=sys.stderr)
        return 2
    home = Path(tempfile.mkdtemp(prefix="weft-home-"))
    try:
        with Desktop(args.compositor, args.output, outputs=["appd.log"]) as desktop:
            run(args, desktop, home, xdotool)
    except (AssertionError, SessionError, TimeoutError) as failure:
        print(f"app data check failed: {failure}", file=sys.stderr)
        print(f"logs in {args.output}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(home, ignore_errors=True)
    print("app data check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
