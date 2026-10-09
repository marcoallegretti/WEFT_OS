#!/usr/bin/env python3
"""Check that package updates are transactional and leave running sessions alone.

The harness installs a test application with weft-pack into a private store
and runs weft-appd with the real runtime and app shell on a nested desktop
(see session.py). Each revision of the application's page shows its own
colour. In the presented pixels and the store:

1. A session started from revision one keeps showing it after the package is
   updated to revision two, and its app shell runs from revision one's own
   directory, which the update keeps while the session runs.
2. An update that fails (an invalid component) changes nothing: revision two
   stays active.
3. A session launched after the update shows revision two.
4. Once the first session has ended, the next update removes revision one,
   and keeps revision two for rollback; `weft-pack rollback` makes revision
   two active again.
5. Uninstalling while a session runs stops new launches (404) but not the
   session; the next uninstall, after it has ended, removes the revisions.

The application is unsigned development content using the Counter demo's
component.
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_counter import AppdClient, free_port, read_endpoint  # noqa: E402
from session import ROOT, Desktop, SessionError, bounding_box, missing_tools, parse_ppm  # noqa: E402

APP_ID = "org.weft.test.update"
COLOURS = {"one": (200, 0, 0), "two": (0, 0, 200), "three": (0, 160, 0)}
VERSIONS = {"one": 1, "two": 2, "bad": 3, "three": 4}
STARTUP_TIMEOUT = 120.0


def write_revision(src, name, wasm=None):
    shutil.rmtree(src, ignore_errors=True)
    (src / "ui").mkdir(parents=True)
    (src / "wapp.toml").write_text(
        f'[package]\nid = "{APP_ID}"\nname = "Update"\nversion = "0.0.{VERSIONS[name]}"\n'
        '[runtime]\nmodule = "app.wasm"\n[ui]\nentry = "ui/index.html"\n')
    if wasm is None:
        shutil.copy(ROOT / "examples/org.weft.demo.counter/app.wasm", src / "app.wasm")
    else:
        (src / "app.wasm").write_bytes(wasm)
    colour = COLOURS.get(name, (0, 0, 0))
    (src / "ui/index.html").write_text(
        '<!DOCTYPE html><html><body style="margin:0;width:100vw;height:100vh;'
        f'background:rgb{colour}"></body></html>')


def shows(desktop, colour, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        width, height, pixels = parse_ppm(desktop.capture())
        if bounding_box(width, height, pixels, colour) is not None:
            return True
        time.sleep(0.3)
    return False


def run(args, desktop, store, home):
    t = Path(args.target)
    env = dict(os.environ, WEFT_APP_STORE=str(store), HOME=str(home),
               XDG_DATA_HOME=str(home / "share"))

    def pack(*arguments, ok=True):
        done = subprocess.run([t / "weft-pack", *arguments], env=env,
                              capture_output=True, text=True, timeout=60)
        if ok and done.returncode != 0:
            raise AssertionError(f"weft-pack {' '.join(arguments)} failed: {done.stderr}")
        if not ok and done.returncode == 0:
            raise AssertionError(f"weft-pack {' '.join(arguments)} succeeded: {done.stdout}")
        return done.stdout + done.stderr

    revisions = store / ".revisions" / APP_ID

    def active():
        return os.readlink(store / APP_ID)

    src = home / "src"
    write_revision(src, "one")
    pack("install", str(src), "--dev")
    one = store / active()

    desktop.launch_client("appd", [t / "weft-appd"], {
        "WEFT_RUNTIME_BIN": str(t / "weft-runtime"),
        "WEFT_APP_SHELL_BIN": str(t / "weft-app-shell"),
        "WEFT_APP_STORE": str(store),
        "WEFT_DISABLE_CGROUP": "1",
        "WEFT_APPD_WS_PORT": str(free_port()),
        "HOME": str(home),
        "XDG_DATA_HOME": str(home / "share"),
    })
    port, token = read_endpoint(desktop.runtime, 30)
    appd = AppdClient(port, token)

    def launch():
        appd.send({"type": "LAUNCH_APP", "app_id": APP_ID, "surface_id": 0})
        reply = appd.wait_for(lambda m: m.get("type") in ("APP_READY", "ERROR"), STARTUP_TIMEOUT)
        if reply.get("type") != "APP_READY":
            raise AssertionError(f"the app did not start: {reply}")
        return reply["session_id"]

    def stop(session_id):
        appd.send({"type": "TERMINATE_APP", "session_id": session_id, "force": True})
        appd.wait_for(lambda m: m.get("type") == "APP_STATE" and m.get("state") == "stopped"
                      and m.get("session_id") == session_id, 20)

    first = launch()
    if not shows(desktop, COLOURS["one"]):
        raise AssertionError("revision one is not shown")
    shells = subprocess.run(["pgrep", "-af", "weft-app-shell"], capture_output=True,
                            text=True).stdout
    if str(one / "ui/index.html") not in shells:
        raise AssertionError(f"the app shell does not run from revision one: {shells}")

    # 1. An update leaves the running session on its revision.
    write_revision(src, "two")
    pack("install", str(src), "--dev")
    two = store / active()
    if two == one:
        raise AssertionError("the update did not activate a new revision")
    time.sleep(2)
    if not shows(desktop, COLOURS["one"], 5) or shows(desktop, COLOURS["two"], 1):
        raise AssertionError("the update changed what the running session shows")
    if (one / "ui/index.html").read_text().find("200, 0, 0") < 0:
        raise AssertionError("the running session's revision changed")

    # 2. A failed update changes nothing.
    write_revision(src, "bad", wasm=b"not a component")
    pack("install", str(src), "--dev", ok=False)
    if store / active() != two:
        raise AssertionError("a failed update changed the active revision")

    # 3. A new launch gets the update.
    second = launch()
    if not shows(desktop, COLOURS["two"]):
        raise AssertionError("a launch after the update does not show revision two")
    stop(second)

    # 4. With the first session ended, the next update collects revision one.
    if not one.is_dir():
        raise AssertionError("revision one was removed while its session ran")
    stop(first)
    write_revision(src, "three")
    pack("install", str(src), "--dev")
    if one.exists():
        raise AssertionError("revision one was kept after its session ended")
    if not two.is_dir():
        raise AssertionError("the previous revision was not kept for rollback")
    pack("rollback", APP_ID)
    if store / active() != two:
        raise AssertionError("rollback did not make revision two active")
    third = launch()
    if not shows(desktop, COLOURS["two"]):
        raise AssertionError("a launch after rollback does not show revision two")

    # 5. Uninstall stops new launches, not the running session.
    pack("uninstall", APP_ID)
    appd.send({"type": "LAUNCH_APP", "app_id": APP_ID, "surface_id": 0})
    refused = appd.wait_for(lambda m: m.get("type") in ("APP_READY", "ERROR"), 30)
    if refused.get("code") != 404:
        raise AssertionError(f"a launch after uninstall was not refused: {refused}")
    if not two.is_dir() or not shows(desktop, COLOURS["two"], 5):
        raise AssertionError("uninstall disturbed the running session")
    stop(third)
    pack("uninstall", APP_ID, ok=False)
    if revisions.exists():
        raise AssertionError(f"revisions left after uninstall: {list(revisions.iterdir())}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/debug/weft-compositor")
    parser.add_argument("--target", type=Path, default=ROOT / "target/debug",
                        help="directory with weft-appd, weft-pack, weft-runtime and weft-app-shell")
    parser.add_argument("--output", type=Path, default=ROOT / "target/update-check")
    args = parser.parse_args(argv)

    missing = missing_tools() + [str(p) for p in (args.compositor, args.target / "weft-appd",
                                                  args.target / "weft-pack",
                                                  args.target / "weft-runtime",
                                                  args.target / "weft-app-shell")
                                 if not p.is_file()]
    if missing:
        print("missing: " + ", ".join(missing), file=sys.stderr)
        return 2
    store = Path(tempfile.mkdtemp(prefix="weft-store-"))
    home = Path(tempfile.mkdtemp(prefix="weft-home-"))
    try:
        with Desktop(args.compositor, args.output, outputs=["appd.log", "update.ppm"]) as desktop:
            try:
                run(args, desktop, store, home)
            except AssertionError:
                (args.output / "update.ppm").write_bytes(desktop.capture())
                raise
    except (AssertionError, SessionError, TimeoutError) as failure:
        print(f"update check failed: {failure}", file=sys.stderr)
        print(f"logs and capture in {args.output}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(store, ignore_errors=True)
        shutil.rmtree(home, ignore_errors=True)
    print("update check passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
