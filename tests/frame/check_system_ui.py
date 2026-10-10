#!/usr/bin/env python3
"""Check that the system UI can be operated with the keyboard alone.

The harness runs weft-servo-shell with the real system UI page
(infra/shell/system-ui.html) and weft-appd with two test applications,
"Blue" and "Red", on a nested desktop (see session.py). Each application
page switches between its colour and a lighter shade on every key press.
Every action below is a key press, apart from one activation request; the
results are observed in weft-appd's lifecycle messages and in the presented
pixels:

- a Super tap brings the shell to the front from an application and opens
  the launcher with focus on its first app, in name order; Super pressed
  with another key leaves the application in front, and that key reaches it
  with the Super modifier, which the application no longer sees once Super
  is released;
- Enter opens the focused app, whose window takes keyboard focus; Tab
  moves through the launcher's apps;
- Escape closes the launcher and returns focus to the Apps button, from
  which Tab reaches each taskbar entry and its close button;
- Enter on a taskbar entry brings its app's window back with keyboard
  focus, and Enter on its close button ends its session;
- the launcher closes when the shell loses focus, so the next Super tap
  opens it, and a second Super tap closes it, leaving focus on the Apps
  button.

The applications are unsigned test packages, recorded as development content
in a private data home, using the Counter demo's component.
"""

import argparse
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_activation import APPS, area  # noqa: E402
from check_counter import AppdClient, free_port, read_endpoint  # noqa: E402
from session import ROOT, Desktop, SessionError, missing_tools  # noqa: E402

BLUE, RED = "org.weft.test.blue", "org.weft.test.red"
NAMES = {BLUE: "Blue", RED: "Red"}
STARTUP_TIMEOUT = 120.0
# The strip weft-servo-shell reserves at the bottom (TASKBAR_HEIGHT).
TASKBAR = 48


def app_page(color, pressed):
    """A page that switches shade on every key press without the Super
    (Meta) modifier, so a modifier left held shows as a missed switch."""
    return ('<!DOCTYPE html><html><body style="margin:0;width:100vw;height:100vh;'
            f'background:rgb{color}"><script>'
            'var pressed = false;'
            'addEventListener("keydown", function (e) { if (e.metaKey) return;'
            'pressed = !pressed;'
            f'document.body.style.background = pressed ? "rgb{pressed}" : "rgb{color}"; }});'
            '</script></body></html>')


def make_store(store, home):
    for app_id, (color, pressed) in APPS.items():
        package = store / app_id
        (package / "ui").mkdir(parents=True)
        (package / "wapp.toml").write_text(
            f'[package]\nid = "{app_id}"\nname = "{NAMES[app_id]}"\nversion = "0.1.0"\n'
            '[runtime]\nmodule = "app.wasm"\n[ui]\nentry = "ui/index.html"\n')
        shutil.copy(ROOT / "examples/org.weft.demo.counter/app.wasm", package / "app.wasm")
        (package / "ui/index.html").write_text(app_page(color, pressed))
        record = home / "share/weft/owners" / app_id
        record.parent.mkdir(parents=True, exist_ok=True)
        record.write_text("development\n")


def run(args, desktop, store, home):
    t = Path(args.target)
    desktop.launch_client("appd", [t / "weft-appd"], {
        "WEFT_RUNTIME_BIN": str(t / "weft-runtime"),
        "WEFT_APP_SHELL_BIN": str(t / "weft-app-shell"),
        "WEFT_APP_STORE": str(store),
        "WEFT_DISABLE_CGROUP": "1",
        "WEFT_APPD_WS_PORT": str(free_port()),
        "HOME": str(home),
        "XDG_DATA_HOME": str(home / "share"),
        "RUST_LOG": "info,weft_appd::ws=debug",
    })
    port, token = read_endpoint(desktop.runtime, 30)
    appd = AppdClient(port, token)
    desktop.launch_client("shell", [args.shell],
                          {"WEFT_SYSTEM_UI_HTML": str(ROOT / "infra/shell/system-ui.html")})
    # This check's client and the system UI page.
    deadline = time.monotonic() + 90
    while desktop.log_text("appd").count("system client connected") < 2:
        if time.monotonic() > deadline:
            raise AssertionError("the system UI did not connect to weft-appd")
        time.sleep(0.5)

    def key(*names):
        subprocess.run([args.xdotool, "search", "--class", "weft-compositor",
                        "windowfocus", "--sync"], env={"DISPLAY": desktop.display},
                       check=True, timeout=20)
        for name in names:
            subprocess.run([args.xdotool, "key", name], env={"DISPLAY": desktop.display},
                           check=True, timeout=20)
            time.sleep(0.3)

    def colors(shot):
        return {c for cs in APPS.values() for c in cs if area(shot, c)[0] is not None}

    def shown(app_id, pressed=False):
        """Waits until `app_id` is the visible application, alone on top."""
        color = APPS[app_id][1 if pressed else 0]
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if colors(desktop.capture()) == {color}:
                return
            time.sleep(0.3)
        state = "pressed " if pressed else ""
        raise AssertionError(f"{NAMES[app_id]} is not shown {state}on top")

    def shell_in_front():
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if not colors(desktop.capture()):
                return
            time.sleep(0.3)
        raise AssertionError("the shell did not come to the front")

    def ready(app_id, timeout=STARTUP_TIMEOUT):
        try:
            message = appd.wait_for(lambda m: m.get("type") == "APP_READY", timeout)
        except TimeoutError:
            raise AssertionError(f"{NAMES[app_id]} was not opened")
        if message.get("app_id") != app_id:
            raise AssertionError(f"expected {NAMES[app_id]} to open, got {message}")
        return message["session_id"]

    def no_launch(seconds=3):
        try:
            message = appd.wait_for(lambda m: m.get("type") == "APP_READY", seconds)
        except TimeoutError:
            return
        raise AssertionError(f"an app opened unexpectedly: {message}")

    # A Super tap opens the launcher on the first app by name; Enter opens
    # it. The page asks for the catalog once connected; until it has
    # arrived, Enter finds no app, so the first attempt may be repeated.
    for attempt in range(10):
        key("Escape", "super", "Return")
        try:
            appd.wait_for(lambda m: m.get("type") == "LAUNCH_ACK", 5)
            break
        except TimeoutError:
            if attempt == 9:
                raise AssertionError("the launcher opened no app")
    blue = ready(BLUE)
    shown(BLUE)
    # The new window has keyboard focus.
    key("a")
    shown(BLUE, pressed=True)

    # Super pressed with another key is not a tap: the key reaches the
    # application with the Super modifier, and the shell stays behind it.
    key("super+a")
    time.sleep(1)
    shown(BLUE, pressed=True)
    # Once Super is released, the application's keys carry no Super
    # modifier, even when another modifier changed while it was held.
    xdo = [args.xdotool]
    for step in (["keydown", "super"], ["key", "shift"], ["keyup", "super"]):
        subprocess.run(xdo + step, env={"DISPLAY": desktop.display}, check=True, timeout=20)
        time.sleep(0.3)
    shown(BLUE, pressed=True)
    key("a")
    shown(BLUE)
    key("a")
    shown(BLUE, pressed=True)

    # A Super tap from an application brings the shell and its launcher to
    # the front; Tab moves to the next app.
    key("super")
    shell_in_front()
    key("Tab", "Return")
    red = ready(RED)
    shown(RED)

    # Escape closes the launcher, so Tab leaves the Apps button for the
    # first taskbar entry rather than an app; Enter activates it.
    key("super")
    shell_in_front()
    key("Escape", "Tab", "Return")
    shown(BLUE, pressed=True)
    no_launch()
    key("a")
    shown(BLUE)

    # The launcher closes when the shell loses focus, here to an
    # activation, so the next Super tap opens it rather than closing it.
    key("super")
    shell_in_front()
    appd.send({"type": "ACTIVATE_APP", "session_id": red})
    shown(RED)
    key("super")
    shell_in_front()
    key("Return")
    extra = ready(BLUE)
    shown(BLUE)

    # Tab passes Blue's entry and close button and Red's entry to reach
    # Red's close button; Enter ends Red's session.
    key("super")
    shell_in_front()
    key("Escape", "Tab", "Tab", "Tab", "Tab", "Return")
    try:
        appd.wait_for(lambda m: m.get("type") == "APP_STATE" and m.get("state") == "stopped"
                      and m.get("session_id") == red, 20)
    except TimeoutError:
        raise AssertionError("Enter on Red's close button did not end its session")
    appd.send({"type": "QUERY_RUNNING"})
    running = appd.wait_for(lambda m: m.get("type") == "RUNNING_APPS", 10)
    if sorted(s["session_id"] for s in running["sessions"]) != sorted([blue, extra]):
        raise AssertionError(f"after closing Red, expected only the Blue sessions: {running}")

    # A second Super tap closes the launcher and leaves focus on the Apps
    # button: Tab then reaches the first taskbar entry, and Enter shows
    # that app instead of opening another.
    key("super", "super")
    no_launch()
    key("Tab", "Return")
    shown(BLUE)
    no_launch()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/debug/weft-compositor")
    parser.add_argument("--shell", type=Path, default=ROOT / "target/debug/weft-servo-shell")
    parser.add_argument("--target", type=Path, default=ROOT / "target/debug",
                        help="directory with weft-appd, weft-runtime and weft-app-shell")
    parser.add_argument("--output", type=Path, default=ROOT / "target/system-ui-check")
    parser.add_argument("--xdotool", default=shutil.which("xdotool"))
    args = parser.parse_args(argv)
    if not args.xdotool:
        print("missing: xdotool", file=sys.stderr)
        return 2

    missing = missing_tools() + [str(p) for p in (args.compositor, args.shell,
                                                  args.target / "weft-appd",
                                                  args.target / "weft-runtime",
                                                  args.target / "weft-app-shell")
                                 if not p.is_file()]
    if missing:
        print("missing: " + ", ".join(missing), file=sys.stderr)
        return 2
    store = Path(tempfile.mkdtemp(prefix="weft-store-"))
    home = Path(tempfile.mkdtemp(prefix="weft-home-"))
    try:
        make_store(store, home)
        with Desktop(args.compositor, args.output,
                     outputs=["appd.log", "system-ui.ppm"]) as desktop:
            try:
                run(args, desktop, store, home)
            except AssertionError:
                (args.output / "system-ui.ppm").write_bytes(desktop.capture())
                raise
    except (AssertionError, SessionError, TimeoutError) as failure:
        print(f"system UI check failed: {failure}", file=sys.stderr)
        print(f"logs and capture in {args.output}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(store, ignore_errors=True)
        shutil.rmtree(home, ignore_errors=True)
    print("system UI check passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
