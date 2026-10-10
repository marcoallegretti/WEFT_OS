"""Nested WEFT desktop for graphical tests: Xvfb, weft-compositor and Wayland clients.

`Desktop` starts Xvfb and weft-compositor with its winit backend, launches
clients on the compositor's Wayland socket, captures the X screen and
terminates every process it started when the `with` block ends. Processes are
started from argument arrays in their own process groups.
"""

import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
# Large enough to show the nested compositor's whole 1280x800 output.
SCREEN = (1400, 900)
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*m")
REQUIRED_TOOLS = ("Xvfb", "xwd", "convert")


class SessionError(Exception):
    """The nested desktop could not be started."""


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


def pixel(width, pixels, x, y):
    offset = (y * width + x) * 3
    return tuple(pixels[offset:offset + 3])


def missing_tools():
    return [tool for tool in REQUIRED_TOOLS if shutil.which(tool) is None]


def wait_for(predicate, timeout, interval=0.5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(interval)
    return None


def _signal_group(process, signum):
    try:
        os.killpg(process.pid, signum)
    except ProcessLookupError:
        pass


def _stop(process):
    """Terminate the process and everything left in its process group."""
    _signal_group(process, signal.SIGTERM)
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        _signal_group(process, signal.SIGKILL)
        process.wait(timeout=10)
    _signal_group(process, signal.SIGKILL)


class Desktop:
    """Xvfb plus weft-compositor; logs are written to `output/<name>.log`."""

    def __init__(self, compositor, output, outputs=(), screen=SCREEN):
        """`outputs` names further files in `output` that this run will write;
        `screen` is the X screen size."""
        self.compositor = Path(compositor)
        self.screen = screen
        self.output = Path(output)
        self.own_outputs = ["xvfb.log", "compositor.log", "shell.log", *outputs]
        self.processes = []
        self.logs = {}

    def __enter__(self):
        self.output.mkdir(parents=True, exist_ok=True)
        for name in self.own_outputs:
            (self.output / name).unlink(missing_ok=True)
        self.runtime = Path(tempfile.mkdtemp(prefix="weft-desktop-"))
        self.runtime.chmod(0o700)
        self.env = {key: value for key, value in os.environ.items()
                    if key not in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET")}
        self.env["XDG_RUNTIME_DIR"] = str(self.runtime)
        self.env.setdefault("RUST_LOG", "info")
        try:
            self._start_display()
            self._start_compositor()
        except BaseException:
            self.__exit__(None, None, None)
            raise
        return self

    def __exit__(self, *exc):
        for process in reversed(self.processes):
            _stop(process)
        for log in self.logs.values():
            log.close()
        shutil.rmtree(self.runtime, ignore_errors=True)
        return False

    def launch(self, name, command, env=None):
        """Start a process with its output in `<name>.log`."""
        log = open(self.output / f"{name}.log", "w", encoding="utf-8")
        self.logs[name] = log
        process = subprocess.Popen([str(part) for part in command], stdout=log,
                                   stderr=subprocess.STDOUT, env=dict(self.env, **(env or {})),
                                   start_new_session=True)
        self.processes.append(process)
        return process

    def launch_client(self, name, command, env=None):
        """Start a Wayland client of the compositor."""
        return self.launch(name, command, dict(env or {}, WAYLAND_DISPLAY=self.socket))

    def log_text(self, name):
        text = (self.output / f"{name}.log").read_text(encoding="utf-8", errors="replace")
        return ANSI_ESCAPE.sub("", text)

    def capture(self):
        """Return the X screen as a P6 PPM."""
        xwd = subprocess.run(["xwd", "-root", "-silent", "-display", self.display],
                             capture_output=True, check=True, timeout=20)
        ppm = subprocess.run(["convert", "xwd:-", "ppm:-"], input=xwd.stdout,
                             capture_output=True, check=True, timeout=20)
        return ppm.stdout

    def _start_display(self):
        # Xvfb picks a free display itself and writes its number to the pipe
        # once it accepts connections, so concurrent runs cannot share one.
        read_end, write_end = os.pipe()
        log = open(self.output / "xvfb.log", "w", encoding="utf-8")
        self.logs["xvfb"] = log
        try:
            xvfb = subprocess.Popen(
                ["Xvfb", "-displayfd", str(write_end), "-screen", "0",
                 f"{self.screen[0]}x{self.screen[1]}x24", "-nolisten", "tcp"],
                stdout=log, stderr=subprocess.STDOUT, env=self.env,
                start_new_session=True, pass_fds=(write_end,))
        finally:
            os.close(write_end)
        self.processes.append(xvfb)
        with os.fdopen(read_end) as reported:
            number = reported.readline().strip()
        if not number.isdigit() or xvfb.poll() is not None:
            raise SessionError("Xvfb did not start")
        self.display = f":{number}"

    def _start_compositor(self):
        compositor = self.launch("compositor", [self.compositor, "--winit"],
                                 {"DISPLAY": self.display})

        def socket_name():
            if compositor.poll() is not None:
                return "exited"
            match = re.search(r'socket_name="([^"]+)"', self.log_text("compositor"))
            return match and match.group(1)

        socket = wait_for(socket_name, 30)
        if socket in (None, "exited"):
            raise SessionError("compositor did not open a Wayland socket; see "
                               f"{self.output / 'compositor.log'}")
        self.socket = socket
