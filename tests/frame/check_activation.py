#!/usr/bin/env python3
"""Check that the compositor stacks, focuses and activates windows.

The harness runs weft-servo-shell with a solid green page as the system
panel, and weft-appd with two test applications, one red and one blue, on a
nested desktop (see session.py). Each application page switches between its
colour and a lighter shade on every key press. In the presented pixels:

- the panel fills the compositor output and stays beneath the applications;
- application windows fill the work area: the output less the strip the
  shell reserves for its taskbar along the bottom, which stays visible;
- each newly launched application is shown on top and receives keys;
- ACTIVATE_APP, the request the taskbar sends, brings the other session's
  window back to the front with keyboard focus;
- when the focused application ends, the remaining one gets keyboard focus;
- ACTIVATE_APP for a session that is not running is refused.

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
from check_counter import AppdClient, free_port, read_endpoint  # noqa: E402
from session import ROOT, Desktop, SessionError, bounding_box, missing_tools, parse_ppm  # noqa: E402

GREEN = (0, 160, 0)
APPS = {
    "org.weft.test.red": ((200, 0, 0), (255, 120, 120)),
    "org.weft.test.blue": ((0, 0, 200), (120, 120, 255)),
}
STARTUP_TIMEOUT = 120.0
# The strip weft-servo-shell reserves at the bottom (TASKBAR_HEIGHT).
TASKBAR = 48

PANEL_PAGE = ('<!DOCTYPE html><html><body style="margin:0;background:rgb(0,160,0);'
              'width:100vw;height:100vh"></body></html>')


def app_page(color, pressed):
    return ('<!DOCTYPE html><html><body style="margin:0;width:100vw;height:100vh;'
            f'background:rgb{color}"><script>'
            'var pressed = false;'
            'addEventListener("keydown", function () { pressed = !pressed;'
            f'document.body.style.background = pressed ? "rgb{pressed}" : "rgb{color}"; }});'
            '</script></body></html>')


def make_store(store, home):
    for app_id, (color, pressed) in APPS.items():
        package = store / app_id
        (package / "ui").mkdir(parents=True)
        (package / "wapp.toml").write_text(
            f'[package]\nid = "{app_id}"\nname = "Test"\nversion = "0.1.0"\n'
            '[runtime]\nmodule = "app.wasm"\n[ui]\nentry = "ui/index.html"\n')
        shutil.copy(ROOT / "examples/org.weft.demo.counter/app.wasm", package / "app.wasm")
        (package / "ui/index.html").write_text(app_page(color, pressed))
        record = home / "share/weft/owners" / app_id
        record.parent.mkdir(parents=True, exist_ok=True)
        record.write_text("development\n")


def area(screenshot, color):
    width, height, pixels = parse_ppm(screenshot)
    box = bounding_box(width, height, pixels, color)
    return box, width, height


def wait_for_color(desktop, color, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        shot = desktop.capture()
        box, _, _ = area(shot, color)
        if box is not None:
            return shot, box
        time.sleep(0.3)
    return desktop.capture(), None


def wait_until_gone(desktop, color, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        box, _, _ = area(desktop.capture(), color)
        if box is None:
            return True
        time.sleep(0.3)
    return False


def run(args, desktop, store, home):
    panel_page = desktop.runtime / "panel.html"
    panel_page.write_text(PANEL_PAGE)
    desktop.launch_client("shell", [args.shell], {"WEFT_SYSTEM_UI_HTML": str(panel_page)})
    shot, panel = wait_for_color(desktop, GREEN, 60)
    if panel is None:
        raise AssertionError("the panel was not shown")
    # The compositor sizes the panel to its whole output, larger than a
    # toplevel's default size.
    if panel[2] - panel[0] + 1 < 1000 or panel[3] - panel[1] + 1 < 700:
        raise AssertionError(f"the panel does not fill the output: {panel}")

    t = Path(args.target)
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

    def key(name):
        subprocess.run([args.xdotool, "search", "--class", "weft-compositor",
                        "windowfocus", "--sync"], env={"DISPLAY": desktop.display},
                       check=True, timeout=20)
        subprocess.run([args.xdotool, "key", name], env={"DISPLAY": desktop.display},
                       check=True, timeout=20)

    def shown(app_id, pressed=False):
        """Waits until `app_id` is the visible application, alone on top."""
        color = APPS[app_id][1 if pressed else 0]
        others = [c for other, cs in APPS.items() for c in cs if other != app_id]
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            shot = desktop.capture()
            box, _, _ = area(shot, color)
            hidden = all(area(shot, c)[0] is None for c in others)
            if box is not None and hidden:
                # The application fills the work area: the panel's whole
                # width, down to the reserved strip, which stays visible.
                if box[0] != panel[0] or box[2] != panel[2] or box[1] != panel[1]:
                    raise AssertionError(f"{app_id} does not fill the work area: {box}")
                if box[3] != panel[3] - TASKBAR:
                    raise AssertionError(
                        f"{app_id} does not end at the reserved strip: {box}, panel {panel}")
                return
            time.sleep(0.3)
        state = "pressed " if pressed else ""
        raise AssertionError(f"{app_id} is not shown {state}on top")

    sessions = {}
    for app_id in APPS:
        appd.send({"type": "LAUNCH_APP", "app_id": app_id, "surface_id": 0})
        try:
            ready = appd.wait_for(
                lambda m, a=app_id: m.get("type") == "APP_READY" and m.get("app_id") == a,
                STARTUP_TIMEOUT)
        except TimeoutError:
            raise AssertionError(f"{app_id} did not start")
        sessions[app_id] = ready["session_id"]
        # A newly launched application comes to the front.
        shown(app_id)

    red, blue = "org.weft.test.red", "org.weft.test.blue"
    # The last launched application has keyboard focus.
    key("a")
    shown(blue, pressed=True)

    def activate(app_id):
        appd.send({"type": "ACTIVATE_APP", "session_id": sessions[app_id]})
        reply = appd.wait_for(lambda m: m.get("type") in ("APP_STATE", "ERROR")
                              and m.get("session_id", sessions[app_id]) == sessions[app_id],
                              10)
        if reply.get("type") != "APP_STATE":
            raise AssertionError(f"activating {app_id} was refused: {reply}")

    # Activation brings the other session back with keyboard focus.
    activate(red)
    shown(red)
    key("a")
    shown(red, pressed=True)
    activate(blue)
    shown(blue, pressed=True)

    # When the focused application ends, keyboard focus returns to the other.
    appd.send({"type": "TERMINATE_APP", "session_id": sessions[blue]})
    shown(red, pressed=True)
    key("a")
    shown(red)

    appd.send({"type": "ACTIVATE_APP", "session_id": 999})
    reply = appd.wait_for(lambda m: m.get("type") == "ERROR", 10)
    if reply.get("code") != 404:
        raise AssertionError(f"activating a session that does not run: {reply}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/debug/weft-compositor")
    parser.add_argument("--shell", type=Path, default=ROOT / "target/debug/weft-servo-shell")
    parser.add_argument("--target", type=Path, default=ROOT / "target/debug",
                        help="directory with weft-appd, weft-runtime and weft-app-shell")
    parser.add_argument("--output", type=Path, default=ROOT / "target/activation-check")
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
        with Desktop(args.compositor, args.output, outputs=["appd.log", "activation.ppm"]) \
                as desktop:
            try:
                run(args, desktop, store, home)
            except AssertionError:
                (args.output / "activation.ppm").write_bytes(desktop.capture())
                raise
    except (AssertionError, SessionError, TimeoutError) as failure:
        print(f"activation check failed: {failure}", file=sys.stderr)
        print(f"logs and capture in {args.output}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(store, ignore_errors=True)
        shutil.rmtree(home, ignore_errors=True)
    print("activation check passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
