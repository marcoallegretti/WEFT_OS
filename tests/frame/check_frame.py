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
early readiness observable: the page stays hidden until a stylesheet next to it
reveals it, and the harness holds a file lease that keeps the stylesheet from
being opened until the delay has passed, so a host that reports
READY before the document has loaded shows no page within the grace period.

Requires Xvfb, xwd and ImageMagick's convert; see session.py.
"""

import argparse
import fcntl
import os
import shutil
import signal
import sys
import threading
import time
from pathlib import Path

sys.dont_write_bytecode = True
from session import ROOT, Desktop, SessionError, bounding_box, missing_tools, parse_ppm  # noqa: E402


HERE = Path(__file__).resolve().parent
COLORS = {
    "red": (255, 0, 0),
    "green": (0, 255, 0),
    "blue": (0, 0, 255),
    "white": (255, 255, 255),
}
MIN_SIDE = 40
MIN_TEXT_PIXELS = 30


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


def slow_style(directory, delay):
    """Create `directory/reveal.css`, a stylesheet revealing the page, that
    cannot be opened until `delay` seconds have passed. A write lease makes
    any other process's open() wait until the lease is released. The
    stylesheet is a local file next to the page, so an application host
    confined to its UI directory may load it."""
    sheet = directory / "reveal.css"
    sheet.write_text("body { visibility: visible; }", encoding="utf-8")
    # The kernel signals the lease holder when another open() waits on it.
    signal.signal(signal.SIGIO, signal.SIG_IGN)
    lease = os.open(sheet, os.O_WRONLY)
    fcntl.fcntl(lease, fcntl.F_SETLEASE, fcntl.F_WRLCK)

    def release():
        time.sleep(delay)
        fcntl.fcntl(lease, fcntl.F_SETLEASE, fcntl.F_UNLCK)
        os.close(lease)

    threading.Thread(target=release, daemon=True).start()


def hidden_until_styled(page, destination):
    """Copy `page`, hiding its body until the delayed stylesheet loads."""
    html = page.read_text(encoding="utf-8")
    link = ('<style>body { visibility: hidden; }</style>\n'
            '<link rel="stylesheet" href="reveal.css">\n</head>')
    if "</head>" not in html:
        raise ValueError(f"{page} has no </head>")
    destination.write_text(html.replace("</head>", link, 1), encoding="utf-8")
    return destination


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
    if args.slow_style:
        # The kernel revokes a lease that is held longer than this.
        limit = int(Path("/proc/sys/fs/lease-break-time").read_text())
        if not 0 < args.slow_style < limit:
            parser.error(f"--slow-style must be between 0 and {limit} seconds")
    if args.shell is None:
        name = "weft-servo-shell" if args.host == "system" else "weft-app-shell"
        args.shell = ROOT / "target/debug" / name

    missing = missing_tools() + [str(b) for b in (args.compositor, args.shell) if not b.is_file()]
    if missing:
        print("missing: " + ", ".join(missing), file=sys.stderr)
        return 2

    try:
        with Desktop(args.compositor, args.output, outputs=["frame.ppm"]) as desktop:
            page = args.page
            if args.slow_style:
                page = hidden_until_styled(args.page, desktop.runtime / "page.html")
            if args.host == "system":
                if args.slow_style:
                    slow_style(desktop.runtime, args.slow_style)
                shell = desktop.launch_client(
                    "shell", [args.shell], {"WEFT_SYSTEM_UI_HTML": str(page.resolve())})
            else:
                store = install_app(desktop.runtime / "store", page)
                if args.slow_style:
                    slow_style(store / APP_ID / "ui", args.slow_style)
                shell = desktop.launch_client(
                    "shell", [args.shell, APP_ID, "1"], {"WEFT_APP_STORE": str(store)})
            problems, screenshot = watch(desktop, shell, args)
    except SessionError as error:
        print(error, file=sys.stderr)
        return 1

    if screenshot is not None:
        (args.output / "frame.ppm").write_bytes(screenshot)
    if problems:
        print("frame check failed: " + "; ".join(problems), file=sys.stderr)
        print(f"logs and last capture in {args.output}", file=sys.stderr)
        return 1
    print(f"frame check passed; capture in {args.output / 'frame.ppm'}")
    return 0


def watch(desktop, shell, args):
    """Capture until the frame matches, the host exits or the time runs out."""
    def ready_printed():
        return any(line.strip() == "READY" for line in desktop.log_text("shell").splitlines())

    problems = ["no frame captured"]
    screenshot = None
    ready_at = None
    deadline = time.monotonic() + args.timeout
    while time.monotonic() < deadline:
        if shell.poll() is not None:
            return [f"shell exited with status {shell.returncode}"], screenshot
        if args.host == "app" and ready_at is None and ready_printed():
            ready_at = time.monotonic()
        screenshot = desktop.capture()
        problems = evaluate(*parse_ppm(screenshot))
        if ready_at is not None and problems and time.monotonic() - ready_at > COMPOSITE_GRACE:
            return [f"page not on screen {COMPOSITE_GRACE:g} s after READY"] + problems, screenshot
        if not problems and (args.host == "system" or ready_at is not None):
            return [], screenshot
        if not problems:
            problems = ["frame presented but READY not printed"]
        time.sleep(1)
    return problems, screenshot


if __name__ == "__main__":
    raise SystemExit(main())
