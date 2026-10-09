"""Check that an application page can reach only its own files and its bridge.

The harness installs a test package, using the Counter demo's component and
the page in tests/frame/confinement, in a temporary store and launches it
through weft-appd with the real runtime and app shell on the nested desktop.
The page probes, and paints one band per probe:

- an image from its own UI directory loads;
- the session bridge reaches the component, which answers;
- an image just outside its UI directory (../outside.png) fails to load;
- an image elsewhere on disk (a file: URL) fails to load;
- an image from a local HTTP server fails to load;
- a no-cors fetch from that server is rejected;
- a WebSocket to a local listener fails.

Each refused probe would succeed without the policy: the images are valid,
the fetch accepts an opaque response and the listener accepts connections.

Every band must be green, and the HTTP server and the TCP listener must see
no connection at all, so the refusals happen in the renderer, not after a
request left it.

Requires Xvfb, xwd and ImageMagick's convert.
"""

import argparse
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
PROBES = ["own-image", "bridge", "outside-image", "other-image", "http-image", "http-fetch",
          "websocket"]
GREEN = (0, 192, 0)
# The app window's page area on the 1024x768 nested desktop.
PAGE_TOP, PAGE_HEIGHT, PAGE_X = 35, 600, 400


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
    requests = 0

    def do_GET(self):
        Counting.requests += 1
        body = png()
        self.send_response(200)
        self.send_header("Content-Type", "image/png")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def make_package(store, http_port, tcp_port, other):
    package = store / APP_ID
    (package / "ui").mkdir(parents=True)
    (package / "wapp.toml").write_text(
        f'[package]\nid = "{APP_ID}"\nname = "Confinement"\nversion = "0.1.0"\n'
        '[runtime]\nmodule = "app.wasm"\n[ui]\nentry = "ui/index.html"\n')
    shutil.copy(ROOT / "examples/org.weft.demo.counter/app.wasm", package / "app.wasm")
    page = (Path(__file__).parent / "confinement/index.html").read_text()
    page = (page.replace("@HTTP_PORT@", str(http_port)).replace("@TCP_PORT@", str(tcp_port))
            .replace("@OTHER_URL@", other.as_uri()))
    (package / "ui/index.html").write_text(page)
    (package / "ui/own.png").write_bytes(png())
    (package / "outside.png").write_bytes(png())
    other.write_bytes(png())


def run(args, desktop, store):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Counting)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    listener.settimeout(0.2)
    connections = []

    def accept():
        while not stop.is_set():
            try:
                connection = listener.accept()[0]
            except OSError:
                continue
            connections.append(connection)
            connection.close()

    stop = threading.Event()
    threading.Thread(target=accept, daemon=True).start()
    try:
        make_package(store, server.server_port, listener.getsockname()[1],
                     store.parent / f"{store.name}-other.png")
        t = Path(args.target)
        desktop.launch_client("appd", [t / "weft-appd"], {
            "WEFT_RUNTIME_BIN": str(t / "weft-runtime"),
            "WEFT_APP_SHELL_BIN": str(t / "weft-app-shell"),
            "WEFT_APP_STORE": str(store),
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
            width, _, pixels = parse_ppm(desktop.capture())
            colours = {}
            for i, name in enumerate(PROBES):
                y = PAGE_TOP + int((i + 0.5) * PAGE_HEIGHT / len(PROBES))
                colours[name] = pixel(width, pixels, PAGE_X, y)
            if all(c == GREEN for c in colours.values()) or time.monotonic() > deadline:
                break
            time.sleep(0.5)
        failed = [name for name, c in colours.items() if c != GREEN]
        if failed:
            raise AssertionError(f"probes not as required: {failed} ({colours})")
        if Counting.requests or connections:
            raise AssertionError(f"requests left the renderer: {Counting.requests} HTTP, "
                                 f"{len(connections)} TCP")
    finally:
        stop.set()
        server.shutdown()
        listener.close()


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
    try:
        with Desktop(args.compositor, args.output, outputs=["appd.log", "confinement.ppm"]) \
                as desktop:
            try:
                run(args, desktop, store)
            except AssertionError:
                (args.output / "confinement.ppm").write_bytes(desktop.capture())
                raise
    except (AssertionError, SessionError, TimeoutError) as failure:
        print(f"confinement check failed: {failure}", file=sys.stderr)
        print(f"logs and capture in {args.output}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(store, ignore_errors=True)
        (store.parent / f"{store.name}-other.png").unlink(missing_ok=True)
    print("confinement check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
