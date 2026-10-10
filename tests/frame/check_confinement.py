"""Check that an application page can reach only its own files and its bridge.

The harness installs a test package, using the Counter demo's component and
the page in tests/frame/confinement, in a temporary store and launches it
through weft-appd with the real runtime and app shell on the nested desktop.
The page probes, and paints one band per probe:

- an image from its own UI directory loads;
- the session bridge reaches the component, which answers;
- an image just outside its UI directory (../outside.png) fails to load;
- an image elsewhere on disk (a file: URL) fails to load;
- an image reached through a symbolic link in the UI directory fails to load;
- an image from a local HTTP server fails to load;
- a no-cors fetch from that server is rejected;
- a WebSocket to a local server that completes the handshake fails;
- a worker does not fetch from that server (Servo does not start workers
  for file: pages, which also keeps them from fetching);
- an about:srcdoc frame has no navigator.servo, which could change engine
  preferences.

The page also imports a stylesheet, loads a font and sends a beacon to the
HTTP server, and frames an about:srcdoc document showing an image from it.

Each refused probe would succeed without the policy: the images are valid,
the fetches accept an opaque response and the WebSocket server accepts the
upgrade. Every band must be green, and the HTTP server and the WebSocket
server must see no connection at all, so the refusals happen in the
renderer, not after a request left it.

Requires Xvfb, xwd and ImageMagick's convert.
"""

import argparse
import base64
import hashlib
import http.server
import shutil
import socket
import struct
import sys
import tempfile
import threading
import time
import zlib
from pathlib import Path

sys.dont_write_bytecode = True
from check_counter import AppdClient, free_port, read_endpoint  # noqa: E402
from session import ROOT, Desktop, SessionError, missing_tools, parse_ppm, pixel  # noqa: E402

APP_ID = "org.weft.test.confinement"
PROBES = ["own-image", "bridge", "outside-image", "other-image", "linked-image", "http-image",
          "http-fetch", "websocket", "worker-fetch", "srcdoc-internals"]
GREEN = (0, 192, 0)
# Where the probe bands are sampled across the app window's page.
PAGE_X = 400
# The X root behind the nested compositor window.
ROOT_COLOUR = (0, 0, 0)


def page_rows(width, height, pixels):
    """The first and last rows the app window's page covers at PAGE_X: the
    compositor gives the application its whole work area. No shell runs in
    this check, so every row of the output that is not the X root is page."""
    rows = [y for y in range(height) if pixel(width, pixels, PAGE_X, y) != ROOT_COLOUR]
    return (rows[0], rows[-1]) if rows else None


def png():
    """A valid 1x1 PNG image."""
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))
    header = struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
    pixels = zlib.compress(b"\x00\x00\x00\xff")
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", pixels)
            + chunk(b"IEND", b""))


class Counting(http.server.BaseHTTPRequestHandler):
    requests = []

    def do_GET(self):
        Counting.requests.append(f"{self.command} {self.path}")
        body = png()
        self.send_response(200)
        self.send_header("Content-Type", "image/png")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_POST = do_GET

    def log_message(self, *args):
        pass


def websocket_server(connections, stop):
    """A listener that completes WebSocket handshakes, so a WebSocket that
    reaches it opens. Returns its port."""
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    listener.settimeout(0.2)

    def serve():
        while not stop.is_set():
            try:
                connection = listener.accept()[0]
            except OSError:
                continue
            connections.append(connection)
            connection.settimeout(5)
            try:
                request = b""
                while b"\r\n\r\n" not in request:
                    chunk = connection.recv(4096)
                    if not chunk:
                        break
                    request += chunk
                key = next((line.split(b":", 1)[1].strip() for line in request.split(b"\r\n")
                            if line.lower().startswith(b"sec-websocket-key:")), b"")
                accept = base64.b64encode(hashlib.sha1(
                    key + b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest())
                connection.sendall(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n"
                                   b"Connection: Upgrade\r\nSec-WebSocket-Accept: " + accept
                                   + b"\r\n\r\n")
            except OSError:
                pass
        listener.close()

    threading.Thread(target=serve, daemon=True).start()
    return listener.getsockname()[1]


def make_package(store, http_port, ws_port, other_dir):
    package = store / APP_ID
    (package / "ui").mkdir(parents=True)
    (package / "wapp.toml").write_text(
        f'[package]\nid = "{APP_ID}"\nname = "Confinement"\nversion = "0.1.0"\n'
        '[runtime]\nmodule = "app.wasm"\n[ui]\nentry = "ui/index.html"\n')
    shutil.copy(ROOT / "examples/org.weft.demo.counter/app.wasm", package / "app.wasm")
    fixture = Path(__file__).parent / "confinement"
    other = other_dir / "other.png"
    page = (fixture / "index.html").read_text()
    page = (page.replace("@HTTP@", f"http://127.0.0.1:{http_port}/")
            .replace("@WS@", f"ws://127.0.0.1:{ws_port}/")
            .replace("@OTHER_URL@", other.as_uri()))
    (package / "ui/index.html").write_text(page)
    (package / "ui/own.png").write_bytes(png())
    (package / "outside.png").write_bytes(png())
    other.write_bytes(png())
    (package / "ui/linked").symlink_to(other_dir)
    # The package holds a link, which weft-pack refuses to install, so it is
    # planted directly and recorded as development content, which runs
    # unverified: the renderer's own confinement is what this check probes.
    record = store / "home/share/weft/owners" / APP_ID
    record.parent.mkdir(parents=True, exist_ok=True)
    record.write_text("development\n")


def run(args, desktop, store, other_dir):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Counting)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    stop = threading.Event()
    connections = []
    ws_port = websocket_server(connections, stop)
    try:
        make_package(store, server.server_port, ws_port, other_dir)
        t = Path(args.target)
        desktop.launch_client("appd", [t / "weft-appd"], {
            "WEFT_RUNTIME_BIN": str(t / "weft-runtime"),
            "WEFT_APP_SHELL_BIN": str(t / "weft-app-shell"),
            "WEFT_APP_STORE": str(store),
            "HOME": str(store / "home"),
            "XDG_DATA_HOME": str(store / "home/share"),
            "WEFT_DISABLE_CGROUP": "1",
            "WEFT_APPD_WS_PORT": str(free_port()),
        })
        port, token = read_endpoint(desktop.runtime, 30)
        appd = AppdClient(port, token)
        appd.send({"type": "LAUNCH_APP", "app_id": APP_ID, "surface_id": 0})
        reply = appd.wait_for(lambda m: m.get("type") in ("APP_READY", "ERROR"), 120)
        if reply.get("type") != "APP_READY":
            raise AssertionError(f"the test app did not start: {reply}")

        deadline = time.monotonic() + 15
        while True:
            width, height, pixels = parse_ppm(desktop.capture())
            rows = page_rows(width, height, pixels)
            if rows is None:
                raise AssertionError("the test app's page is not on screen")
            top, bottom = rows
            colours = {}
            for i, name in enumerate(PROBES):
                y = top + int((i + 0.5) * (bottom - top + 1) / len(PROBES))
                colours[name] = pixel(width, pixels, PAGE_X, y)
            if all(c == GREEN for c in colours.values()) or time.monotonic() > deadline:
                break
            time.sleep(0.5)
        time.sleep(1)  # let requests the page sent after the last band arrive
        problems = []
        failed = [name for name, c in colours.items() if c != GREEN]
        if failed:
            problems.append(f"probes not as required: {failed}")
        if Counting.requests or connections:
            problems.append(f"requests left the renderer: HTTP {sorted(Counting.requests)}, "
                            f"{len(connections)} WebSocket")
        if problems:
            raise AssertionError("; ".join(problems))
    finally:
        stop.set()
        server.shutdown()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/debug/weft-compositor")
    parser.add_argument("--target", type=Path, default=ROOT / "target/debug",
                        help="directory with weft-appd, weft-runtime and weft-app-shell")
    parser.add_argument("--output", type=Path, default=ROOT / "target/confinement-check")
    args = parser.parse_args(argv)

    missing = missing_tools() + [str(p) for p in (args.compositor, args.target / "weft-appd",
                                                  args.target / "weft-runtime",
                                                  args.target / "weft-app-shell")
                                 if not p.is_file()]
    if missing:
        print("missing: " + ", ".join(missing), file=sys.stderr)
        return 2
    store = Path(tempfile.mkdtemp(prefix="weft-store-"))
    other_dir = Path(tempfile.mkdtemp(prefix="weft-other-"))
    try:
        with Desktop(args.compositor, args.output, outputs=["appd.log", "confinement.ppm"]) \
                as desktop:
            try:
                run(args, desktop, store, other_dir)
            except AssertionError:
                (args.output / "confinement.ppm").write_bytes(desktop.capture())
                raise
    except (AssertionError, SessionError, TimeoutError) as failure:
        print(f"confinement check failed: {failure}", file=sys.stderr)
        print(f"logs and capture in {args.output}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(store, ignore_errors=True)
        shutil.rmtree(other_dir, ignore_errors=True)
    print("confinement check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
