"""Check that a Servo host paints and presents a known page through weft-compositor.

The harness starts Xvfb, runs weft-compositor nested on it with the winit
backend, launches a Servo host as a Wayland client showing `reference.html`,
captures the X screen and checks the presented pixels: four solid quadrants
(red, green, blue, white) in the page's arrangement and dark text pixels
inside the white quadrant. It exits non-zero when the frame never appears or
does not match.

`--host system` runs weft-servo-shell with the page as its system UI.
`--host app` installs the page as a package in a temporary app store and runs
weft-app-shell for it. The host must print READY, and the page must be on
screen within a short grace period after READY, because the compositor shows
a presented frame slightly later. READY printed within that grace before the
page is presented is therefore not detected. `--slow-style SECONDS` makes
early readiness observable: the page stays hidden until a stylesheet that a
local HTTP server delivers after the delay reveals it, so a host that reports
READY before the document has loaded shows no page within the grace period.

Requires Xvfb, xwd and ImageMagick's convert. Processes are started from
argument arrays and are always terminated before the harness exits.
"""

import argparse
import http.server
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SCREEN = (1024, 768)
COLORS = {
    "red": (255, 0, 0),
    "green": (0, 255, 0),
    "blue": (0, 0, 255),
    "white": (255, 255, 255),
}
MIN_SIDE = 40
MIN_TEXT_PIXELS = 30
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*m")


def parse_ppm(data):
    """Decode a binary P6 PPM into (width, height, bytes)."""
    tokens = []
    index = 0
    while len(tokens) < 4:
        while data[index:index + 1].isspace():
            index += 1
        if data[index:index + 1] == b"#":
            index = data.index(b"\n", index) + 1
            continue
        end = index
        while not data[end:end + 1].isspace():
            end += 1
        tokens.append(data[index:end])
        index = end
    magic, width, height, maxval = tokens
    if magic != b"P6" or int(maxval) != 255:
        raise ValueError("expected an 8-bit P6 image")
    width, height = int(width), int(height)
    pixels = data[index + 1:index + 1 + width * height * 3]
    if len(pixels) != width * height * 3:
        raise ValueError("truncated image")
    return width, height, pixels


def bounding_box(width, height, pixels, color):
    """Return (x0, y0, x1, y1) of pixels exactly matching color, or None."""
    target = bytes(color)
    xs, ys = [], []
    for y in range(0, height, 2):
        row = pixels[y * width * 3:(y + 1) * width * 3]
        start = row.find(target)
        while start != -1:
            if start % 3 == 0:
                xs.append(start // 3)
                ys.append(y)
                start = row.find(target, start + 3)
            else:
                # A match spanning two pixels; realign on the next byte.
                start = row.find(target, start + 1)
    if not xs:
        return None
    return min(xs), min(ys), max(xs), max(ys)


def dark_pixels(width, pixels, box):
    x0, y0, x1, y1 = box
    count = 0
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            offset = (y * width + x) * 3
            if max(pixels[offset:offset + 3]) < 96:
                count += 1
    return count


def evaluate(width, height, pixels):
    """Return a list of problems with the captured frame; empty means it matches."""
    boxes = {name: bounding_box(width, height, pixels, color) for name, color in COLORS.items()}
    problems = [f"no {name} region" for name, box in boxes.items() if box is None]
    if problems:
        return problems
    for name, (x0, y0, x1, y1) in boxes.items():
        if x1 - x0 < MIN_SIDE or y1 - y0 < MIN_SIDE:
            problems.append(f"{name} region too small: {x1 - x0 + 1}x{y1 - y0 + 1}")
    red, green, blue, white = (boxes[n] for n in ("red", "green", "blue", "white"))
    if not green[0] > red[2]:
        problems.append("green is not right of red")
    if not blue[1] > red[3]:
        problems.append("blue is not below red")
    if not (white[0] > blue[2] and white[1] > green[3]):
        problems.append("white is not below green and right of blue")
    if abs(red[1] - green[1]) > 4 or abs(red[0] - blue[0]) > 4:
        problems.append("quadrants are not aligned")
    text = dark_pixels(width, pixels, white)
    if text < MIN_TEXT_PIXELS:
        problems.append(f"no text in white region ({text} dark pixels)")
    return problems


APP_ID = "org.weft.test.frame"
COMPOSITE_GRACE = 2.0


def install_app(store, page):
    """Install `page` as the UI entry of a minimal package and return the store root."""
    package = store / APP_ID
    (package / "ui").mkdir(parents=True)
    shutil.copyfile(page, package / "ui" / "index.html")
    (package / "wapp.toml").write_text(
        "[package]\n"
        f'id = "{APP_ID}"\n'
        'name = "Frame reference"\n'
        'version = "0.0.0"\n'
        "\n[runtime]\n"
        'module = "app.wasm"\n'
        "\n[ui]\n"
        'entry = "ui/index.html"\n',
        encoding="utf-8",
    )
    return store


def slow_style_server(delay):
    """Serve a stylesheet that reveals the page after `delay` seconds."""
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            time.sleep(delay)
            body = b"body { visibility: visible; }"
            self.send_response(200)
            self.send_header("Content-Type", "text/css")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def hidden_until_styled(page, destination, port):
    """Copy `page`, hiding its body until the delayed stylesheet loads."""
    html = page.read_text(encoding="utf-8")
    link = (f'<style>body {{ visibility: hidden; }}</style>\n'
            f'<link rel="stylesheet" href="http://127.0.0.1:{port}/reveal.css">\n</head>')
    if "</head>" not in html:
        raise ValueError(f"{page} has no </head>")
    destination.write_text(html.replace("</head>", link, 1), encoding="utf-8")
    return destination


def capture(display):
    xwd = subprocess.run(["xwd", "-root", "-silent", "-display", display],
                         capture_output=True, check=True, timeout=20)
    ppm = subprocess.run(["convert", "xwd:-", "ppm:-"], input=xwd.stdout,
                         capture_output=True, check=True, timeout=20)
    return ppm.stdout


def free_display():
    for number in range(90, 140):
        if not Path(f"/tmp/.X11-unix/X{number}").exists() and \
                not Path(f"/tmp/.X{number}-lock").exists():
            return f":{number}"
    raise RuntimeError("no free X display number")


def start(command, log, env):
    return subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, env=env,
                            start_new_session=True)


def signal_group(process, signum):
    try:
        os.killpg(process.pid, signum)
    except ProcessLookupError:
        pass


def stop(process):
    """Terminate the process and everything left in its process group."""
    if process is None:
        return
    signal_group(process, signal.SIGTERM)
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        signal_group(process, signal.SIGKILL)
        process.wait(timeout=10)
    signal_group(process, signal.SIGKILL)


def wait_for(predicate, timeout, interval=0.5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(interval)
    return None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--compositor", type=Path,
                        default=ROOT / "target/debug/weft-compositor")
    parser.add_argument("--host", choices=("system", "app"), default="system")
    parser.add_argument("--shell", type=Path,
                        help="host binary (default: target/debug/weft-servo-shell or "
                             "target/debug/weft-app-shell)")
    parser.add_argument("--page", type=Path, default=HERE / "reference.html")
    parser.add_argument("--slow-style", type=float, metavar="SECONDS",
                        help="hide the page until a stylesheet delayed by SECONDS loads")
    parser.add_argument("--timeout", type=float, default=120.0,
                        help="seconds to wait for a matching frame")
    parser.add_argument("--output", type=Path, default=ROOT / "target/frame-check",
                        help="directory for logs and the captured screenshot")
    args = parser.parse_args(argv)
    if args.shell is None:
        name = "weft-servo-shell" if args.host == "system" else "weft-app-shell"
        args.shell = ROOT / "target/debug" / name

    for tool in ("Xvfb", "xwd", "convert"):
        if shutil.which(tool) is None:
            print(f"missing required tool: {tool}", file=sys.stderr)
            return 2
    for binary in (args.compositor, args.shell):
        if not binary.is_file():
            print(f"missing binary: {binary}", file=sys.stderr)
            return 2

    args.output.mkdir(parents=True, exist_ok=True)
    runtime = Path(tempfile.mkdtemp(prefix="weft-frame-"))
    runtime.chmod(0o700)
    display = free_display()
    base_env = {key: value for key, value in os.environ.items()
                if key not in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET")}
    base_env["XDG_RUNTIME_DIR"] = str(runtime)
    base_env.setdefault("RUST_LOG", "info")

    page = args.page
    style_server = None
    if args.slow_style:
        style_server = slow_style_server(args.slow_style)
        page = hidden_until_styled(args.page, runtime / "page.html",
                                   style_server.server_address[1])

    processes = []
    logs = {name: open(args.output / f"{name}.log", "w", encoding="utf-8")
            for name in ("xvfb", "compositor", "shell")}
    try:
        xvfb = start(["Xvfb", display, "-screen", "0", f"{SCREEN[0]}x{SCREEN[1]}x24",
                      "-nolisten", "tcp"], logs["xvfb"], base_env)
        processes.append(xvfb)
        if not wait_for(lambda: Path(f"/tmp/.X11-unix/X{display[1:]}").exists(), 15):
            print("Xvfb did not start", file=sys.stderr)
            return 1

        compositor_env = dict(base_env, DISPLAY=display)
        compositor = start([str(args.compositor), "--winit"], logs["compositor"],
                           compositor_env)
        processes.append(compositor)
        compositor_log = args.output / "compositor.log"

        def socket_name():
            if compositor.poll() is not None:
                return "exited"
            text = ANSI_ESCAPE.sub("", compositor_log.read_text(encoding="utf-8",
                                                                errors="replace"))
            match = re.search(r'socket_name="([^"]+)"', text)
            return match and match.group(1)

        socket = wait_for(socket_name, 30)
        if socket in (None, "exited"):
            print(f"compositor did not open a Wayland socket; see {compositor_log}",
                  file=sys.stderr)
            return 1

        shell_env = dict(base_env, WAYLAND_DISPLAY=socket)
        if args.host == "system":
            shell_env["WEFT_SYSTEM_UI_HTML"] = str(page.resolve())
            command = [str(args.shell)]
        else:
            shell_env["WEFT_APP_STORE"] = str(install_app(runtime / "store", page))
            command = [str(args.shell), APP_ID, "1"]
        shell = start(command, logs["shell"], shell_env)
        processes.append(shell)
        shell_log = args.output / "shell.log"

        def ready_printed():
            text = shell_log.read_text(encoding="utf-8", errors="replace")
            return any(line.strip() == "READY" for line in text.splitlines())

        problems = ["no frame captured"]
        screenshot = None
        ready_at = None
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            if shell.poll() is not None:
                problems = [f"shell exited with status {shell.returncode}"]
                break
            if args.host == "app" and ready_at is None and ready_printed():
                ready_at = time.monotonic()
            ready = ready_at is not None
            screenshot = capture(display)
            problems = evaluate(*parse_ppm(screenshot))
            if ready and problems and time.monotonic() - ready_at > COMPOSITE_GRACE:
                problems = [f"page not on screen {COMPOSITE_GRACE:g} s after READY"] + problems
                break
            if not problems and (args.host == "system" or ready):
                break
            if not problems:
                problems = ["frame presented but READY not printed"]
            time.sleep(1)

        if screenshot is not None:
            (args.output / "frame.ppm").write_bytes(screenshot)
        if problems:
            print("frame check failed: " + "; ".join(problems), file=sys.stderr)
            print(f"logs and last capture in {args.output}", file=sys.stderr)
            return 1
        print(f"frame check passed; capture in {args.output / 'frame.ppm'}")
        return 0
    finally:
        for process in reversed(processes):
            stop(process)
        if style_server is not None:
            style_server.shutdown()
        for log in logs.values():
            log.close()
        shutil.rmtree(runtime, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
