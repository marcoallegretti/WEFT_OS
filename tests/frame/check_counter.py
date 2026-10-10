"""Check that the Counter demo's UI actions reach its Wasm component and back.

The harness runs weft-appd on a nested desktop (see session.py) with
weft-runtime (built with wasmtime-runtime) and weft-app-shell, launches
`org.weft.demo.counter` as the trusted system client, and drives the page
with real keyboard input routed by weft-compositor:

1. The page must appear once weft-appd reports APP_READY.
2. ArrowUp, ArrowUp, ArrowDown are sent. The page keeps no count of its own;
   it shows the value the component returns over the authenticated session
   bridge. The count area must therefore change after each key (0 -> 1 -> 2)
   and look exactly as it did at 1 after ArrowDown.

Requires Xvfb, xwd, ImageMagick's convert and xdotool.
"""

import argparse
import base64
import json
import os
import shutil
import socket
import struct
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True
from session import ROOT, Desktop, SessionError, bounding_box, missing_tools, parse_ppm  # noqa: E402


APP_ID = "org.weft.demo.counter"
PAGE = (15, 15, 15)  # The Counter page's background, #0f0f0f.
CARD = (26, 26, 26)  # The Counter card's background, #1a1a1a.
STARTUP_TIMEOUT = 120.0


class AppdClient:
    """Minimal WebSocket client for weft-appd's system role."""

    def __init__(self, port, token):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=10)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall((f"GET /appd HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
                           "Upgrade: websocket\r\nConnection: Upgrade\r\n"
                           f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
                          .encode())
        response = b""
        while b"\r\n\r\n" not in response:
            chunk = self.sock.recv(1)
            if not chunk:
                raise SessionError("weft-appd closed the WebSocket handshake")
            response += chunk
        if b" 101 " not in response.split(b"\r\n", 1)[0]:
            raise SessionError("weft-appd refused the WebSocket handshake")
        self.send({"type": "HELLO", "role": "system", "token": token})

    def send(self, message):
        data = json.dumps(message).encode()
        mask = os.urandom(4)
        if len(data) < 126:
            header = bytes([0x81, 0x80 | len(data)])
        else:
            header = bytes([0x81, 0x80 | 126]) + struct.pack(">H", len(data))
        self.sock.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def _exact(self, count):
        data = b""
        while len(data) < count:
            chunk = self.sock.recv(count - len(data))
            if not chunk:
                raise EOFError
            data += chunk
        return data

    def receive(self, timeout):
        """The next JSON message, or None when the connection closes."""
        self.sock.settimeout(timeout)
        try:
            header = self._exact(2)
        except EOFError:
            return None
        length = header[1] & 0x7F
        if length == 126:
            length = struct.unpack(">H", self._exact(2))[0]
        elif length == 127:
            length = struct.unpack(">Q", self._exact(8))[0]
        payload = self._exact(length)
        if header[0] & 0x0F == 0x8:
            return None
        return json.loads(payload)

    def wait_for(self, predicate, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            message = self.receive(max(0.1, deadline - time.monotonic()))
            if message is None:
                raise SessionError("weft-appd closed the connection")
            if predicate(message):
                return message
        raise TimeoutError


def read_endpoint(runtime, timeout):
    """weft-appd's port and system token once it has written both."""
    def endpoint():
        try:
            port = int((runtime / "weft/appd.wsport").read_text().strip())
            token = (runtime / "weft/appd.systoken").read_text().strip()
        except (OSError, ValueError):
            return None
        return port, token

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found = endpoint()
        if found:
            return found
        time.sleep(0.2)
    raise SessionError("weft-appd did not publish its endpoint")


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def find_card(screenshot):
    """The Counter card's bounds, searched inside the page.

    The desktop around the application window has the card's colour too, so
    the search is limited to the area the page background spans.
    """
    width, height, pixels = parse_ppm(screenshot)
    page = bounding_box(width, height, pixels, PAGE)
    if not page:
        return None
    x0, y0, x1, y1 = page
    crop_width = x1 - x0 + 1
    crop = b"".join(pixels[(y * width + x0) * 3:(y * width + x1 + 1) * 3] for y in range(y0, y1 + 1))
    card = bounding_box(crop_width, y1 - y0 + 1, crop, CARD)
    if not card:
        return None
    return card[0] + x0, card[1] + y0, card[2] + x0, card[3] + y0


def count_area(screenshot, card):
    """Pixels of the count, the upper middle of the Counter card."""
    width, _, pixels = parse_ppm(screenshot)
    x0, y0, x1, y1 = card
    cx = (x0 + x1) // 2
    top, bottom = y0 + (y1 - y0) // 5, y0 + (y1 - y0) // 2
    rows = [pixels[(y * width + cx - 80) * 3:(y * width + cx + 80) * 3] for y in range(top, bottom)]
    return b"".join(rows)


def run(args, desktop, xdotool):
    port = free_port()
    target = Path(args.target)
    # A private home, so owner records appd writes stay out of the user's.
    home = args.output / "home"
    shutil.rmtree(home, ignore_errors=True)
    home.mkdir(parents=True)
    desktop.launch_client("appd", [target / "weft-appd"], {
        "WEFT_RUNTIME_BIN": str(target / "weft-runtime"),
        "WEFT_APP_SHELL_BIN": str(target / "weft-app-shell"),
        "WEFT_APP_STORE": str(args.store),
        "WEFT_TRUSTED_KEYS": str(ROOT / "examples/keys"),
        "HOME": str(home),
        "XDG_DATA_HOME": str(home / "share"),
        "WEFT_DISABLE_CGROUP": "1",
        "WEFT_APPD_WS_PORT": str(port),
    })
    port, token = read_endpoint(desktop.runtime, 30)
    appd = AppdClient(port, token)
    appd.send({"type": "LAUNCH_APP", "app_id": APP_ID, "surface_id": 0})
    try:
        ready = appd.wait_for(lambda m: m.get("type") == "APP_READY", STARTUP_TIMEOUT)
    except TimeoutError:
        raise AssertionError("no APP_READY")
    session_id = ready["session_id"]

    # The app's only Wayland connection is the one appd handed the
    # compositor for this session.
    bound = f"client bound to session session_id={session_id} app_id={APP_ID}"
    if bound not in desktop.log_text("compositor"):
        raise AssertionError(f"the compositor did not bind the app's client: {bound!r}")

    def capture_card():
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            shot = desktop.capture()
            card = find_card(shot)
            if card and card[2] - card[0] > 200 and card[3] - card[1] > 200:
                return shot, card
            time.sleep(0.3)
        raise AssertionError("Counter card not on screen")

    shot, card = capture_card()
    states = [count_area(shot, card)]

    def act(*arguments):
        subprocess_run(xdotool, desktop.display, arguments)

    act("search", "--class", "weft-compositor", "windowfocus", "--sync")
    # Click inside the card, away from the buttons, for keyboard focus.
    act("mousemove", "--sync", str((card[0] + card[2]) // 2), str(card[1] + 12))
    act("click", "1")

    def press(key, description, expect_change_from):
        act("key", key)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            area = count_area(desktop.capture(), card)
            if area != expect_change_from:
                time.sleep(0.5)
                return count_area(desktop.capture(), card)
            time.sleep(0.2)
        raise AssertionError(f"{description}: count did not change")

    states.append(press("Up", "first ArrowUp", states[0]))
    states.append(press("Up", "second ArrowUp", states[1]))
    states.append(press("Down", "ArrowDown", states[2]))
    if len({states[0], states[1], states[2]}) != 3:
        raise AssertionError("counts 0, 1 and 2 are not distinct on screen")
    if states[3] != states[1]:
        raise AssertionError("count after ArrowDown does not match the count after one ArrowUp")

    # Ending the session closes its client in the compositor.
    appd.send({"type": "TERMINATE_APP", "session_id": session_id})
    deadline = time.monotonic() + 15
    closed = f"closing the client of an ended session session_id={session_id}"
    while closed not in desktop.log_text("compositor"):
        if time.monotonic() > deadline:
            raise AssertionError(f"the compositor did not close the session's client: {closed!r}")
        time.sleep(0.3)


def subprocess_run(xdotool, display, arguments):
    import subprocess
    subprocess.run([xdotool, *arguments], env={"DISPLAY": display}, check=True, timeout=20)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/debug/weft-compositor")
    parser.add_argument("--target", type=Path, default=ROOT / "target/debug",
                        help="directory with weft-appd, weft-runtime and weft-app-shell")
    parser.add_argument("--store", type=Path, default=ROOT / "examples",
                        help="app store containing org.weft.demo.counter")
    parser.add_argument("--output", type=Path, default=ROOT / "target/counter-check")
    args = parser.parse_args(argv)

    xdotool = shutil.which("xdotool")
    missing = missing_tools() + ([] if xdotool else ["xdotool"])
    missing += [str(p) for p in (args.compositor, args.target / "weft-appd",
                                 args.target / "weft-runtime", args.target / "weft-app-shell")
                if not p.is_file()]
    if missing:
        print("missing: " + ", ".join(missing), file=sys.stderr)
        return 2
    try:
        with Desktop(args.compositor, args.output, outputs=["appd.log", "counter.ppm"]) as desktop:
            try:
                run(args, desktop, xdotool)
            except AssertionError as failure:
                (args.output / "counter.ppm").write_bytes(desktop.capture())
                print(f"counter check failed: {failure}", file=sys.stderr)
                print(f"logs and capture in {args.output}", file=sys.stderr)
                return 1
    except SessionError as error:
        print(error, file=sys.stderr)
        return 1
    print("counter check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
