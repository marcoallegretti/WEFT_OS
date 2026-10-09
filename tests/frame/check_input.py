"""Check that native pointer and keyboard input reaches a page through weft-compositor.

The harness runs weft-servo-shell with `input.html` on a nested desktop (see
session.py) and injects real X input into the compositor's window with
xdotool (after giving that window X input focus, since Xvfb runs without a
window manager), which weft-compositor's winit backend receives and routes to
the focused Wayland client. Each step must change the page as the page's script
defines, as seen in the presented pixels:

1. A click 6 px inside the left half's right edge turns only the left half
   green; the right half must stay blue.
2. Typing `a` turns the right half yellow.
3. Typing `Shift+a` turns the right half magenta, which checks the modifier.
4. A click 6 px inside the right half's left edge turns the right half white.

The two clicks straddle the boundary between the halves, so a pointer
position that is scaled or offset by more than a few pixels fails.

Requires Xvfb, xwd, ImageMagick's convert and xdotool.
"""

import argparse
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True
from session import ROOT, Desktop, SessionError, bounding_box, missing_tools, parse_ppm  # noqa: E402


HERE = Path(__file__).resolve().parent
RED, GREEN, BLUE = (255, 0, 0), (0, 255, 0), (0, 0, 255)
WHITE, YELLOW, MAGENTA = (255, 255, 255), (255, 255, 0), (255, 0, 255)
MIN_SIDE = 40


def region(screenshot, color):
    """Bounding box of `color` if it covers a page half, else None."""
    box = bounding_box(*parse_ppm(screenshot), color)
    if box and box[2] - box[0] >= MIN_SIDE and box[3] - box[1] >= MIN_SIDE:
        return box
    return None


STARTUP_TIMEOUT = 120.0
EDGE_INSET = 6


class Steps:
    def __init__(self, desktop, shell, timeout):
        self.desktop = desktop
        self.shell = shell
        self.timeout = timeout
        self.screenshot = None
        self.xdotool_path = shutil.which("xdotool")

    def expect(self, description, present, absent=(), timeout=None):
        """Wait until every colour in `present` and none in `absent` is shown."""
        deadline = time.monotonic() + (self.timeout if timeout is None else timeout)
        while time.monotonic() < deadline:
            if self.shell.poll() is not None:
                raise AssertionError(f"shell exited with status {self.shell.returncode}")
            self.screenshot = self.desktop.capture()
            boxes = {color: region(self.screenshot, color) for color in present}
            if all(boxes.values()) and not any(region(self.screenshot, c) for c in absent):
                return boxes
            time.sleep(0.5)
        raise AssertionError(f"{description}: expected page state not shown")

    def xdotool(self, *arguments):
        subprocess.run([self.xdotool_path, *arguments], env={"DISPLAY": self.desktop.display},
                       check=True, timeout=20)

    def click(self, x, y):
        self.xdotool("mousemove", "--sync", str(x), str(y))
        self.xdotool("click", "1")


def run(steps):
    boxes = steps.expect("initial page", [RED, BLUE], timeout=STARTUP_TIMEOUT)
    # Xvfb runs without a window manager, so give the compositor's window the
    # X input focus that a real session's window manager would.
    steps.xdotool("search", "--class", "weft-compositor", "windowfocus", "--sync")
    left, right = boxes[RED], boxes[BLUE]
    middle = (left[1] + left[3]) // 2
    steps.click(left[2] - EDGE_INSET, middle)
    steps.expect("click near the right edge of the left half", [GREEN, BLUE],
                 absent=[RED, WHITE])
    steps.xdotool("key", "a")
    steps.expect("key a", [GREEN, YELLOW], absent=[BLUE])
    steps.xdotool("key", "shift+a")
    steps.expect("key Shift+a", [GREEN, MAGENTA], absent=[YELLOW])
    steps.click(right[0] + EDGE_INSET, middle)
    steps.expect("click near the left edge of the right half", [GREEN, WHITE],
                 absent=[MAGENTA, RED])


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/debug/weft-compositor")
    parser.add_argument("--shell", type=Path, default=ROOT / "target/debug/weft-servo-shell")
    parser.add_argument("--page", type=Path, default=HERE / "input.html")
    parser.add_argument("--timeout", type=float, default=30.0,
                        help="seconds to wait for each expected page state")
    parser.add_argument("--output", type=Path, default=ROOT / "target/input-check",
                        help="directory for logs and the last capture")
    args = parser.parse_args(argv)

    missing = missing_tools() + [tool for tool in ("xdotool",) if shutil.which(tool) is None]
    missing += [str(b) for b in (args.compositor, args.shell) if not b.is_file()]
    if missing:
        print("missing: " + ", ".join(missing), file=sys.stderr)
        return 2

    try:
        with Desktop(args.compositor, args.output, outputs=["input.ppm"]) as desktop:
            shell = desktop.launch_client("shell", [args.shell],
                                          {"WEFT_SYSTEM_UI_HTML": str(args.page.resolve())})
            steps = Steps(desktop, shell, args.timeout)
            try:
                run(steps)
            except AssertionError as failure:
                print(f"input check failed: {failure}", file=sys.stderr)
                print(f"logs and last capture in {args.output}", file=sys.stderr)
                return 1
            finally:
                if steps.screenshot is not None:
                    (args.output / "input.ppm").write_bytes(steps.screenshot)
    except SessionError as error:
        print(error, file=sys.stderr)
        return 1
    print("input check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
